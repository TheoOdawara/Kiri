use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;

use crate::modules::memory::application::memory_port::Memory;
use crate::modules::memory::domain::entry::{MemoryEntry, MemoryKind};
use crate::modules::memory::domain::scope::Scope;
use crate::modules::memory::domain::similarity::is_near_duplicate;
use crate::modules::provider::application::completion_provider::{
    CompletionProvider, NullSink, TurnRequest,
};
use crate::shared::kernel::error::{AgentError, AgentResult};
use crate::shared::kernel::message::Message;
use crate::shared::kernel::role::Role;

/// One entry the model proposes to remember, parsed from its JSON output.
#[derive(Deserialize)]
struct DistilledEntry {
    kind: String,
    content: String,
    #[serde(default)]
    tags: Vec<String>,
    scope: String,
}

/// What a distillation pass wrote, skipped, and failed to write, for a user-facing summary. `failed`
/// counts entries whose durable write itself errored (DB locked, disk full) — kept distinct from
/// `skipped` (a legitimate dedup/validation/unavailable skip) so a real persistence failure is surfaced,
/// not silently swallowed (ERR-01).
pub struct DistillReport {
    pub written: usize,
    pub skipped: usize,
    pub failed: usize,
}

impl DistillReport {
    fn empty() -> Self {
        Self {
            written: 0,
            skipped: 0,
            failed: 0,
        }
    }
}

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_MAX_ENTRIES: usize = 12;
const DEFAULT_MAX_TRANSCRIPT_BYTES: usize = 16 * 1024;

/// The first N words of a candidate's content used as the keyword recall query when checking for a
/// duplicate — enough to find an equivalent entry without over-narrowing the search.
const DEDUP_QUERY_WORDS: usize = 6;
/// How many existing entries to recall as duplicate candidates per write.
const DEDUP_RECALL_LIMIT: usize = 5;

const DISTILL_SYSTEM_PROMPT: &str = concat!(
    "You distill durable knowledge from a coding session for a long-term memory. Read the transcript ",
    "and extract only knowledge that is reusable in future, unrelated sessions: architectural or design ",
    "decisions, recommended patterns, anti-patterns to avoid, reusable snippets, learned heuristics, ",
    "verifiable technical facts, and explicit user preferences ('always use X', 'I prefer Y', 'never ",
    "do Z'). Ignore everything ephemeral or task-specific (file paths edited this turn, one-off bug ",
    "fixes, transient state, pleasantries).\n\n",
    "Output ONLY a JSON array (no prose, no code fences) of objects with exactly these fields:\n",
    "  - kind: one of \"decision\", \"pattern\", \"anti-pattern\", \"snippet\", \"heuristic\", \"fact\", ",
    "\"preference\"\n",
    "  - content: a concise, self-contained statement of the knowledge (no transcript references)\n",
    "  - tags: an array of short lowercase tags (may be empty)\n",
    "  - scope: \"shared\" for cross-project truths and ALL user preferences, else \"project\"\n\n",
    "Return [] when nothing is worth keeping. Never invent knowledge that the transcript does not ",
    "support. Output the JSON array and nothing else."
);

/// The end-of-session learning pass: feed the conversation to the model, ask it to extract durable
/// knowledge, and persist what it returns to memory. Depends on the `Memory` capability (to write) and is
/// handed a `CompletionProvider` at call time (so it always uses the live adapter after a `/provider` swap).
pub struct Distiller {
    memory: Arc<dyn Memory>,
    project_id: String,
    timeout: Duration,
    max_entries: usize,
    max_transcript_bytes: usize,
}

impl Distiller {
    pub fn new(memory: Arc<dyn Memory>, project_id: String) -> Self {
        Self {
            memory,
            project_id,
            timeout: DEFAULT_TIMEOUT,
            max_entries: DEFAULT_MAX_ENTRIES,
            max_transcript_bytes: DEFAULT_MAX_TRANSCRIPT_BYTES,
        }
    }

    /// Distill `conversation` and write the extracted entries. Bounded by an internal timeout so a slow
    /// provider never hangs the caller; a provider failure, a timeout, or invalid model output all return
    /// `Err` (the caller surfaces it as a Notice). The conversation is read-only and persisted
    /// independently, so a failed distillation never loses data.
    pub async fn distill(
        &self,
        provider: &dyn CompletionProvider,
        model: &str,
        conversation: &[Message],
    ) -> AgentResult<DistillReport> {
        let transcript = render_transcript(conversation, self.max_transcript_bytes);
        if transcript.trim().is_empty() {
            return Ok(DistillReport::empty());
        }

        let messages = vec![
            Message::system(DISTILL_SYSTEM_PROMPT),
            Message::user(transcript),
        ];
        let request = TurnRequest {
            messages: &messages,
            model,
            tools: &[],
        };
        let mut sink = NullSink;
        let completed = tokio::time::timeout(self.timeout, provider.complete(request, &mut sink))
            .await
            .map_err(|_| AgentError::Memory("distillation timed out".to_string()))??;

        let entries = parse_entries(&completed.content)?;
        let mut report = DistillReport::empty();

        // 1. Validate each proposal into a ready-to-write candidate; an invalid kind/scope or an
        //    unavailable scope is a legitimate skip.
        let mut candidates = Vec::new();
        for raw in entries.into_iter().take(self.max_entries) {
            match self.validate(raw) {
                Some(candidate) => candidates.push(candidate),
                None => report.skipped += 1,
            }
        }

        // 2. Intra-batch dedup (no I/O): drop a candidate that near-duplicates an earlier accepted one in
        //    the same scope. Replaces the old persist-order dependency, where entry N only saw entries
        //    1..N-1 because each was written before the next was checked.
        let mut accepted: Vec<Candidate> = Vec::new();
        for candidate in candidates {
            let duplicate = accepted.iter().any(|prior| {
                prior.scope == candidate.scope
                    && is_near_duplicate(&prior.content, &candidate.content)
            });
            if duplicate {
                report.skipped += 1;
            } else {
                accepted.push(candidate);
            }
        }

        // 3. Dedup against the existing store and persist, per scope, batching the embed: one
        //    `recall_batch` covers every candidate's dedup query in a single embed round-trip.
        let (project, shared) = accepted
            .into_iter()
            .partition::<Vec<_>, _>(|c| c.scope == Scope::Project);
        self.persist_scope(Scope::Project, project, &mut report)
            .await;
        self.persist_scope(Scope::Shared, shared, &mut report).await;
        Ok(report)
    }

    /// Validate a proposal into a writable candidate. `None` for an invalid kind/scope or a scope whose
    /// store is unavailable — all legitimate skips.
    fn validate(&self, raw: DistilledEntry) -> Option<Candidate> {
        let kind = raw.kind.parse::<MemoryKind>().ok()?;
        let scope = Scope::from_wire(&raw.scope)?;
        let available = match scope {
            Scope::Shared => self.memory.shared_memory_available(),
            Scope::Project => self.memory.project_memory_available(),
        };
        available.then_some(Candidate {
            kind,
            content: raw.content,
            tags: raw.tags,
            scope,
        })
    }

    /// Dedup `items` (all in `scope`) against the existing store with a single batched recall, then persist
    /// the survivors. A recall failure degrades to "not a duplicate" so a transient store error never
    /// blocks learning (worst case one redundant entry, never lost knowledge); a durable-write failure is
    /// counted as `failed` (ERR-01), distinct from a dedup skip, and the pass continues.
    async fn persist_scope(&self, scope: Scope, items: Vec<Candidate>, report: &mut DistillReport) {
        if items.is_empty() {
            return;
        }
        let queries: Vec<String> = items
            .iter()
            .map(|item| leading_words(&item.content, DEDUP_QUERY_WORDS))
            .collect();
        let hits = self
            .memory
            .recall_batch(scope, &queries, DEDUP_RECALL_LIMIT)
            .await
            .unwrap_or_else(|_| vec![Vec::new(); queries.len()]);
        for (i, item) in items.into_iter().enumerate() {
            let query = &queries[i];
            let entry_hits = hits.get(i).map(Vec::as_slice).unwrap_or(&[]);
            let is_duplicate = !query.is_empty()
                && entry_hits
                    .iter()
                    .any(|hit| is_near_duplicate(&hit.content, &item.content));
            if is_duplicate {
                report.skipped += 1;
                continue;
            }
            let entry = MemoryEntry::new(
                item.kind,
                item.content,
                item.tags.into_iter().collect(),
                scope.project_id_for(&self.project_id),
            );
            let written = match scope {
                Scope::Shared => self.memory.remember_shared(entry).await,
                Scope::Project => self.memory.remember_project(entry).await,
            };
            match written {
                Ok(()) => report.written += 1,
                Err(_) => report.failed += 1,
            }
        }
    }
}

/// A validated, ready-to-write proposal: the parsed kind/scope plus the content and tags to persist.
struct Candidate {
    kind: MemoryKind,
    content: String,
    tags: Vec<String>,
    scope: Scope,
}

/// Render a bounded transcript: user and assistant text only (system and tool noise dropped), keeping the
/// most recent tail within `max_bytes` so a long session still fits the distiller's context.
fn render_transcript(messages: &[Message], max_bytes: usize) -> String {
    let mut lines = Vec::new();
    for message in messages {
        let label = match message.role {
            Role::User => "User",
            Role::Assistant => "Assistant",
            Role::System | Role::Tool => continue,
        };
        if let Some(content) = message.content.as_deref().filter(|c| !c.trim().is_empty()) {
            lines.push(format!("{label}: {}", content.trim()));
        }
    }
    let joined = lines.join("\n\n");
    if joined.len() <= max_bytes {
        return joined;
    }
    let mut start = joined.len() - max_bytes;
    while !joined.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &joined[start..])
}

/// Extract the JSON array from the model's output (tolerating code fences or stray prose around it) and
/// parse it. No array at all means "nothing to learn" (an empty result); a malformed array is an error.
fn parse_entries(content: &str) -> AgentResult<Vec<DistilledEntry>> {
    let (Some(start), Some(end)) = (content.find('['), content.rfind(']')) else {
        return Ok(Vec::new());
    };
    if end < start {
        return Ok(Vec::new());
    }
    serde_json::from_str(&content[start..=end])
        .map_err(|error| AgentError::Memory(format!("distillation: invalid model output: {error}")))
}

/// The first `n` whitespace-separated words of `text`, used as a recall query for dedup.
fn leading_words(text: &str, n: usize) -> String {
    text.split_whitespace()
        .take(n)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::memory::infrastructure::test_support::temp_port;
    use crate::modules::provider::application::completion_provider::EventSink;
    use crate::shared::kernel::completed_turn::CompletedTurn;
    use tempfile::TempDir;

    /// A provider that returns a fixed `content` once, ignoring the request.
    struct ScriptedProvider {
        content: String,
    }

    #[async_trait::async_trait(?Send)]
    impl CompletionProvider for ScriptedProvider {
        async fn complete(
            &self,
            _request: TurnRequest<'_>,
            _sink: &mut dyn EventSink,
        ) -> AgentResult<CompletedTurn> {
            Ok(CompletedTurn {
                content: self.content.clone(),
                tool_calls: Vec::new(),
                thinking: None,
            })
        }
    }

    fn conversation() -> Vec<Message> {
        vec![
            Message::system("sys"),
            Message::user("sempre use tabs"),
            Message::assistant_text("entendido, vou usar tabs"),
        ]
    }

    #[tokio::test]
    async fn writes_entries_from_a_json_array() {
        let dir = TempDir::new().unwrap();
        let memory = temp_port(&dir).await;
        let distiller = Distiller::new(memory.clone(), "proj-a".into());
        let provider = ScriptedProvider {
            content: r#"[
                {"kind":"preference","content":"always use tabs","tags":["style"],"scope":"shared"},
                {"kind":"fact","content":"edition 2024 ships in 1.85","tags":[],"scope":"project"}
            ]"#
            .into(),
        };

        let report = distiller
            .distill(&provider, "m", &conversation())
            .await
            .unwrap();
        assert_eq!(report.written, 2);
        assert_eq!(memory.recall_shared("tabs", 10).await.unwrap().len(), 1);
        assert_eq!(memory.recall_project("edition", 10).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn empty_array_writes_nothing() {
        let dir = TempDir::new().unwrap();
        let memory = temp_port(&dir).await;
        let distiller = Distiller::new(memory, "proj-a".into());
        let provider = ScriptedProvider {
            content: "[]".into(),
        };
        let report = distiller
            .distill(&provider, "m", &conversation())
            .await
            .unwrap();
        assert_eq!(report.written, 0);
        assert_eq!(report.skipped, 0);
    }

    #[tokio::test]
    async fn tolerates_fenced_json() {
        let dir = TempDir::new().unwrap();
        let memory = temp_port(&dir).await;
        let distiller = Distiller::new(memory.clone(), "proj-a".into());
        let provider = ScriptedProvider {
            content: "Here is what I learned:\n```json\n[{\"kind\":\"heuristic\",\"content\":\"fail fast\",\"scope\":\"shared\"}]\n```"
                .into(),
        };
        let report = distiller
            .distill(&provider, "m", &conversation())
            .await
            .unwrap();
        assert_eq!(report.written, 1);
    }

    #[tokio::test]
    async fn invalid_json_is_an_error() {
        let dir = TempDir::new().unwrap();
        let memory = temp_port(&dir).await;
        let distiller = Distiller::new(memory, "proj-a".into());
        let provider = ScriptedProvider {
            content: r#"[{"kind": broken, "content"}]"#.into(),
        };
        assert!(
            distiller
                .distill(&provider, "m", &conversation())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn skips_invalid_kind_and_scope() {
        let dir = TempDir::new().unwrap();
        let memory = temp_port(&dir).await;
        let distiller = Distiller::new(memory, "proj-a".into());
        let provider = ScriptedProvider {
            content: r#"[
                {"kind":"bogus","content":"x","scope":"shared"},
                {"kind":"fact","content":"y","scope":"galaxy"}
            ]"#
            .into(),
        };
        let report = distiller
            .distill(&provider, "m", &conversation())
            .await
            .unwrap();
        assert_eq!(report.written, 0);
        assert_eq!(report.skipped, 2);
    }

    /// A `Memory` whose scopes are available but every durable write errors — to exercise ERR-01.
    struct FailingMemory;

    #[async_trait::async_trait]
    impl Memory for FailingMemory {
        async fn recall_project(&self, _: &str, _: usize) -> AgentResult<Vec<MemoryEntry>> {
            Ok(Vec::new())
        }
        async fn recall_shared(&self, _: &str, _: usize) -> AgentResult<Vec<MemoryEntry>> {
            Ok(Vec::new())
        }
        async fn recall_batch(
            &self,
            _: Scope,
            queries: &[String],
            _: usize,
        ) -> AgentResult<Vec<Vec<MemoryEntry>>> {
            Ok(vec![Vec::new(); queries.len()])
        }
        async fn remember_project(&self, _: MemoryEntry) -> AgentResult<()> {
            Err(AgentError::Memory("disk full".into()))
        }
        async fn remember_shared(&self, _: MemoryEntry) -> AgentResult<()> {
            Err(AgentError::Memory("disk full".into()))
        }
        fn project_memory_available(&self) -> bool {
            true
        }
        fn shared_memory_available(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn a_write_failure_is_counted_as_failed_not_skipped() {
        // ERR-01: a durable-write error must surface as `failed`, distinct from a dedup/validation skip.
        let distiller = Distiller::new(Arc::new(FailingMemory), "proj-a".into());
        let provider = ScriptedProvider {
            content: r#"[{"kind":"fact","content":"the sky is blue","scope":"shared"}]"#.into(),
        };
        let report = distiller
            .distill(&provider, "m", &conversation())
            .await
            .unwrap();
        assert_eq!(report.written, 0);
        assert_eq!(report.skipped, 0);
        assert_eq!(report.failed, 1, "a write error must be counted as failed");
    }

    #[tokio::test]
    async fn dedups_near_duplicates_within_one_pass() {
        // Two near-identical entries in the SAME pass: the intra-batch dedup keeps the first and skips the
        // second, without relying on persist order (the batched flow recalls the store once, so the second
        // would not see the first via the store — the local self-dedup is what catches it).
        let dir = TempDir::new().unwrap();
        let memory = temp_port(&dir).await;
        let distiller = Distiller::new(memory.clone(), "proj-a".into());
        let provider = ScriptedProvider {
            content: r#"[
                {"kind":"fact","content":"the sky is blue","scope":"shared"},
                {"kind":"fact","content":"the sky is blue","scope":"shared"}
            ]"#
            .into(),
        };
        let report = distiller
            .distill(&provider, "m", &conversation())
            .await
            .unwrap();
        assert_eq!(report.written, 1);
        assert_eq!(report.skipped, 1);
        assert_eq!(memory.recall_shared("sky", 10).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn dedups_an_already_known_entry() {
        let dir = TempDir::new().unwrap();
        let memory = temp_port(&dir).await;
        let distiller = Distiller::new(memory.clone(), "proj-a".into());
        let provider = ScriptedProvider {
            content: r#"[{"kind":"fact","content":"the sky is blue","scope":"shared"}]"#.into(),
        };
        // First pass writes it; second pass with the same content must skip it.
        let first = distiller
            .distill(&provider, "m", &conversation())
            .await
            .unwrap();
        assert_eq!(first.written, 1);
        let second = distiller
            .distill(&provider, "m", &conversation())
            .await
            .unwrap();
        assert_eq!(second.written, 0);
        assert_eq!(second.skipped, 1);
    }
}
