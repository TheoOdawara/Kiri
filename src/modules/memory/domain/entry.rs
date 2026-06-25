use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use time::OffsetDateTime;
use uuid::Uuid;

/// Tipo de entrada de memória — categoriza o conteúdo para facilitar busca e uso.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemoryKind {
    /// Decisão arquitetural ou de design (equivalente a ADR).
    Decision,
    /// Padrão de código ou arquitetura recomendado.
    Pattern,
    /// Anti-pattern: o que evitar e por quê.
    AntiPattern,
    /// Trecho de código reutilizável (template, boilerplate, snippet).
    Snippet,
    /// Heurística ou regra prática aprendida.
    Heuristic,
    /// Fato técnico verificável (versão, limite, comportamento de API).
    Fact,
}

impl MemoryKind {
    /// All kinds, for enumeration (e.g. a kind picker in the planned memory-management UI).
    #[allow(dead_code)]
    pub fn all() -> &'static [MemoryKind] {
        &[
            MemoryKind::Decision,
            MemoryKind::Pattern,
            MemoryKind::AntiPattern,
            MemoryKind::Snippet,
            MemoryKind::Heuristic,
            MemoryKind::Fact,
        ]
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryKind::Decision => "decision",
            MemoryKind::Pattern => "pattern",
            MemoryKind::AntiPattern => "anti-pattern",
            MemoryKind::Snippet => "snippet",
            MemoryKind::Heuristic => "heuristic",
            MemoryKind::Fact => "fact",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "decision" => Some(MemoryKind::Decision),
            "pattern" => Some(MemoryKind::Pattern),
            "anti-pattern" => Some(MemoryKind::AntiPattern),
            "snippet" => Some(MemoryKind::Snippet),
            "heuristic" => Some(MemoryKind::Heuristic),
            "fact" => Some(MemoryKind::Fact),
            _ => None,
        }
    }
}

impl std::fmt::Display for MemoryKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Entrada única de memória.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// Identificador único (UUID v7 para ordenação temporal).
    pub id: String,
    /// Tipo da entrada.
    pub kind: MemoryKind,
    /// Conteúdo principal (Markdown suportado).
    pub content: String,
    /// Tags para busca e organização.
    #[serde(default)]
    pub tags: HashSet<String>,
    /// Identificador do projeto (hash do path) — None = memória global compartilhada.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// Timestamp de criação (ISO 8601).
    pub created_at: String,
    /// Timestamp de última atualização (ISO 8601).
    pub updated_at: String,
}

/// RFC3339 timestamp for "now". Formatting a valid UTC instant cannot fail in practice; the empty
/// fallback keeps this runtime path total without an `unwrap` (forbidden outside tests).
fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

impl MemoryEntry {
    /// Cria uma nova entrada com timestamps atuais e UUID v7.
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

    /// Atualiza o conteúdo e o timestamp de atualização. Usada por testes e reservada para a futura UI
    /// de edição de memória.
    #[allow(dead_code)]
    pub fn update_content(&mut self, content: String) {
        self.content = content;
        self.updated_at = now_rfc3339();
    }

    /// Adiciona tags. Reservada para a futura UI de gestão de memória.
    #[allow(dead_code)]
    pub fn add_tags(&mut self, tags: impl IntoIterator<Item = String>) {
        self.tags.extend(tags);
        self.updated_at = now_rfc3339();
    }

    /// Verifica se a entry corresponde a uma query textual simples.
    pub fn matches_query(&self, query: &str) -> bool {
        let q = query.to_lowercase();
        self.content.to_lowercase().contains(&q)
            || self.tags.iter().any(|t| t.to_lowercase().contains(&q))
            || self.kind.as_str().contains(&q)
    }

    /// Formata para exibição no contexto do agente.
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
    fn memory_kind_roundtrip() {
        for kind in MemoryKind::all() {
            let s = kind.as_str();
            assert_eq!(MemoryKind::from_str(s), Some(*kind));
        }
        assert_eq!(MemoryKind::from_str("invalid"), None);
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
