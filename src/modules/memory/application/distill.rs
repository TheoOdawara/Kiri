use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;

use crate::modules::memory::application::memory_port::MemoryPort;
use crate::modules::memory::domain::entry::{MemoryEntry, MemoryKind};
use crate::modules::memory::domain::scope::Scope;
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

/// What a distillation pass wrote and skipped, for a user-facing summary.
pub struct DistillReport {
    pub written: usize,
    pub skipped: usize,
}

impl DistillReport {
    fn empty() -> Self {
        Self {
            written: 0,
            skipped: 0,
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
/// Jaccard token-overlap at/above which two entries are treated as the same fact reworded (a strict
/// superset scores lower and survives).
const NEAR_DUPLICATE_JACCARD: f32 = 0.8;

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
/// knowledge, and persist what it returns to memory. Depends on the `MemoryPort` (to write) and is handed
/// a `CompletionProvider` at call time (so it always uses the live adapter after a `/provider` swap).
pub struct Distiller {
    memory: Arc<dyn MemoryPort>,
    project_id: String,
    timeout: Duration,
    max_entries: usize,
    max_transcript_bytes: usize,
}

impl Distiller {
    pub fn new(memory: Arc<dyn MemoryPort>, project_id: String) -> Self {
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
        for raw in entries.into_iter().take(self.max_entries) {
            if self.persist(raw).await {
                report.written += 1;
            } else {
                report.skipped += 1;
            }
        }
        Ok(report)
    }

    /// Validate, dedup, and persist one proposed entry. Returns whether it was written.
    async fn persist(&self, raw: DistilledEntry) -> bool {
        let Some(kind) = MemoryKind::from_str(&raw.kind) else {
            return false;
        };
        let Some(scope) = Scope::from_wire(&raw.scope) else {
            return false;
        };
        let available = match scope {
            Scope::Shared => self.memory.shared_memory_available(),
            Scope::Project => self.memory.project_memory_available(),
        };
        if !available {
            return false;
        }
        if self.is_duplicate(&raw.content, scope).await {
            return false;
        }
        let entry = MemoryEntry::new(
            kind,
            raw.content,
            raw.tags.into_iter().collect(),
            scope.project_id_for(&self.project_id),
        );
        let result = match scope {
            Scope::Shared => self.memory.remember_shared(entry).await,
            Scope::Project => self.memory.remember_project(entry).await,
        };
        result.is_ok()
    }

    /// Whether an equivalent entry already exists in the target scope, so re-learning the same fact each
    /// session does not balloon the store. Recalls candidates by the content's leading words (keyword
    /// search), then compares normalized text for equality or containment.
    async fn is_duplicate(&self, content: &str, scope: Scope) -> bool {
        let query = leading_words(content, DEDUP_QUERY_WORDS);
        if query.is_empty() {
            return false;
        }
        let hits = match scope {
            Scope::Shared => self.memory.recall_shared(&query, DEDUP_RECALL_LIMIT).await,
            Scope::Project => self.memory.recall_project(&query, DEDUP_RECALL_LIMIT).await,
        };
        let Ok(hits) = hits else {
            // A recall failure must not block learning — treat as "not a duplicate" and let the write
            // proceed (the worst case is one redundant entry, never lost knowledge).
            return false;
        };
        hits.iter()
            .any(|hit| is_near_duplicate(&hit.content, content))
    }
}

/// Whether two entries are the same fact (normalized equality or a high token-overlap reword). Crucially
/// NOT plain substring containment: a terse older entry that is a substring of a richer new one is a
/// strict superset, not a duplicate, so the more-informative entry is kept rather than dropped.
fn is_near_duplicate(a: &str, b: &str) -> bool {
    let na = normalize(a);
    let nb = normalize(b);
    if na == nb {
        return true;
    }
    let ta: HashSet<&str> = na.split_whitespace().collect();
    let tb: HashSet<&str> = nb.split_whitespace().collect();
    if ta.is_empty() || tb.is_empty() {
        return false;
    }
    let intersection = ta.intersection(&tb).count();
    let union = ta.union(&tb).count();
    // Jaccard ≥ NEAR_DUPLICATE_JACCARD ⇒ essentially the same fact reworded; a strict superset scores
    // lower and survives.
    (intersection as f32 / union as f32) >= NEAR_DUPLICATE_JACCARD
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

/// Lowercase and collapse all whitespace, for order-insensitive duplicate comparison.
fn normalize(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
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

    #[test]
    fn near_duplicate_catches_rewords_but_keeps_a_richer_superset() {
        // Case-only difference normalizes equal → duplicate.
        assert!(is_near_duplicate(
            "Always use tabs for indentation",
            "always use tabs for indentation"
        ));
        // A high-overlap reword (one extra token) → duplicate.
        assert!(is_near_duplicate(
            "always use tabs for indentation",
            "always use tabs for indentation here"
        ));
        // A terse fact that is a substring of a much richer one is a SUPERSET, not a duplicate — the
        // richer entry must be kept (the regression: plain containment dropped it).
        assert!(!is_near_duplicate(
            "use tabs",
            "always use tabs for indentation in rust source files"
        ));
    }

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
