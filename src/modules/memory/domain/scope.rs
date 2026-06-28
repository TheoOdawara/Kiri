/// Where a memory entry is stored. A shared entry is global (no project id); a project entry is stamped
/// with the current project. Parsing and the project-id rule live here so the `remember` tool and the
/// distiller share one source instead of each re-deriving the `"shared" => global` rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Project,
    Shared,
}

impl Scope {
    /// Parse the wire string used by the `remember`/distill schemas. `None` for any other value.
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "project" => Some(Scope::Project),
            "shared" => Some(Scope::Shared),
            _ => None,
        }
    }

    /// The `project_id` to stamp on an entry saved in this scope: `None` for shared (global by
    /// definition, so it is reachable from every project), the current project for project scope.
    pub fn project_id_for(self, current: &str) -> Option<String> {
        match self {
            Scope::Project => Some(current.to_string()),
            Scope::Shared => None,
        }
    }
}

/// Which memory a recall query reads. Unlike `Scope`, recall can union both stores (`Both`), so it is a
/// distinct query option rather than a storage scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecallScope {
    Project,
    Shared,
    Both,
}

impl RecallScope {
    /// Parse the wire string used by the `recall_memory` schema. `None` for any other value.
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "project" => Some(RecallScope::Project),
            "shared" => Some(RecallScope::Shared),
            "both" => Some(RecallScope::Both),
            _ => None,
        }
    }

    pub fn includes_project(self) -> bool {
        matches!(self, RecallScope::Project | RecallScope::Both)
    }

    pub fn includes_shared(self) -> bool {
        matches!(self, RecallScope::Shared | RecallScope::Both)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_parses_project_shared_and_rejects_unknown() {
        assert_eq!(Scope::from_wire("project"), Some(Scope::Project));
        assert_eq!(Scope::from_wire("shared"), Some(Scope::Shared));
        // "both" is a recall-only option; it is not a storage scope.
        assert_eq!(Scope::from_wire("both"), None);
        assert_eq!(Scope::from_wire("galaxy"), None);
    }

    #[test]
    fn project_id_for_shared_is_none_else_current() {
        assert_eq!(
            Scope::Project.project_id_for("proj-a"),
            Some("proj-a".into())
        );
        assert_eq!(Scope::Shared.project_id_for("proj-a"), None);
    }

    #[test]
    fn recall_scope_parses_project_shared_both_and_rejects_unknown() {
        assert_eq!(
            RecallScope::from_wire("project"),
            Some(RecallScope::Project)
        );
        assert_eq!(RecallScope::from_wire("shared"), Some(RecallScope::Shared));
        assert_eq!(RecallScope::from_wire("both"), Some(RecallScope::Both));
        assert!(RecallScope::from_wire("nope").is_none());
    }

    #[test]
    fn recall_scope_inclusion() {
        assert!(RecallScope::Both.includes_project());
        assert!(RecallScope::Both.includes_shared());
        assert!(RecallScope::Project.includes_project());
        assert!(!RecallScope::Project.includes_shared());
        assert!(RecallScope::Shared.includes_shared());
        assert!(!RecallScope::Shared.includes_project());
    }
}
