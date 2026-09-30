use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::time::now_rfc3339;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::str::FromStr;
use uuid::Uuid;

/// Memory entry kind — categorizes the content to ease search and use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemoryKind {
    /// Architectural or design decision (equivalent to an ADR).
    Decision,
    /// A recommended code or architecture pattern.
    Pattern,
    /// Anti-pattern: what to avoid and why.
    AntiPattern,
    /// A reusable piece of code (template, boilerplate, snippet).
    Snippet,
    /// A learned heuristic or rule of thumb.
    Heuristic,
    /// A verifiable technical fact (version, limit, API behavior).
    Fact,
    /// A durable user preference ("always use X", "I prefer Y") that should shape future work.
    Preference,
}

impl MemoryKind {
    /// Reserved for the memory-management UI's kind picker.
    #[cfg(test)]
    pub fn all() -> &'static [MemoryKind] {
        &[
            MemoryKind::Decision,
            MemoryKind::Pattern,
            MemoryKind::AntiPattern,
            MemoryKind::Snippet,
            MemoryKind::Heuristic,
            MemoryKind::Fact,
            MemoryKind::Preference,
        ]
    }

    /// Paired with the `FromStr` impl, so the enum has one round-trippable wire shape.
    pub fn as_wire(&self) -> &'static str {
        match self {
            MemoryKind::Decision => "decision",
            MemoryKind::Pattern => "pattern",
            MemoryKind::AntiPattern => "anti-pattern",
            MemoryKind::Snippet => "snippet",
            MemoryKind::Heuristic => "heuristic",
            MemoryKind::Fact => "fact",
            MemoryKind::Preference => "preference",
        }
    }
}

impl FromStr for MemoryKind {
    type Err = AgentError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "decision" => Ok(MemoryKind::Decision),
            "pattern" => Ok(MemoryKind::Pattern),
            "anti-pattern" => Ok(MemoryKind::AntiPattern),
            "snippet" => Ok(MemoryKind::Snippet),
            "heuristic" => Ok(MemoryKind::Heuristic),
            "fact" => Ok(MemoryKind::Fact),
            "preference" => Ok(MemoryKind::Preference),
            other => Err(AgentError::Memory(format!("unknown memory kind '{other}'"))),
        }
    }
}

impl std::fmt::Display for MemoryKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_wire())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// UUID v7, so the id sorts by creation time.
    pub id: String,
    pub kind: MemoryKind,
    pub content: String,
    #[serde(default)]
    pub tags: HashSet<String>,
    /// A hash of the workspace path; `None` means global shared memory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// ISO 8601.
    pub created_at: String,
    /// ISO 8601.
    pub updated_at: String,
}

impl MemoryEntry {
    /// Reading the wall clock and the RNG here is the ADR-0010-sanctioned domain exception.
    pub fn new(
        kind: MemoryKind,
        content: String,
        tags: HashSet<String>,
        project_id: Option<String>,
    ) -> Self {
        let id = Uuid::now_v7().to_string();
        let timestamp = now_rfc3339();
        Self {
            id,
            kind,
            content,
            tags,
            project_id,
            created_at: timestamp.clone(),
            updated_at: timestamp,
        }
    }

    #[cfg(test)]
    pub fn update_content(&mut self, content: String) {
        self.content = content;
        self.updated_at = now_rfc3339();
    }

    pub fn matches_query(&self, query: &str) -> bool {
        let q = query.to_lowercase();
        self.content.to_lowercase().contains(&q)
            || self.tags.iter().any(|t| t.to_lowercase().contains(&q))
            || self.kind.as_wire().contains(&q)
    }

    pub fn format_for_context(&self) -> String {
        let tags = if self.tags.is_empty() {
            String::new()
        } else {
            format!(
                " [tags: {}]",
                self.tags.iter().cloned().collect::<Vec<_>>().join(", ")
            )
        };
        let project = self.project_id.as_deref().unwrap_or("global");
        format!(
            "--- MemoryEntry ({}) {}{} ---\n{}\n--- End ---",
            self.kind, project, tags, self.content
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_kind_round_trips_through_wire() {
        for kind in MemoryKind::all() {
            assert_eq!(kind.as_wire().parse::<MemoryKind>().unwrap(), *kind);
        }
    }

    #[test]
    fn memory_kind_parse_rejects_unknown() {
        assert!("invalid".parse::<MemoryKind>().is_err());
    }

    #[test]
    fn fact_parses_via_fromstr() {
        assert_eq!("fact".parse::<MemoryKind>().unwrap(), MemoryKind::Fact);
    }

    #[test]
    fn entry_new_has_id_and_timestamps() {
        let entry = MemoryEntry::new(MemoryKind::Pattern, "content".into(), HashSet::new(), None);
        assert!(!entry.id.is_empty());
        assert!(!entry.created_at.is_empty());
        assert_eq!(entry.created_at, entry.updated_at);
    }

    #[test]
    fn entry_update_content_changes_updated_at() {
        let mut entry = MemoryEntry::new(MemoryKind::Fact, "old".into(), HashSet::new(), None);
        let created = entry.created_at.clone();
        std::thread::sleep(std::time::Duration::from_millis(10));
        entry.update_content("new".into());
        assert_eq!(entry.content, "new");
        assert_ne!(entry.updated_at, created);
    }

    #[test]
    fn entry_matches_query() {
        let entry = MemoryEntry::new(
            MemoryKind::Pattern,
            "Use Option<T> instead of unwrap".into(),
            ["rust", "error-handling"]
                .into_iter()
                .map(String::from)
                .collect(),
            None,
        );
        assert!(entry.matches_query("option"));
        assert!(entry.matches_query("unwrap"));
        assert!(entry.matches_query("rust"));
        assert!(entry.matches_query("error"));
        assert!(!entry.matches_query("python"));
    }
}
