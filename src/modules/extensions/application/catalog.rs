use std::collections::HashMap;

use crate::modules::extensions::domain::resource::{CommandSpec as ExtCommandSpec, Resource, Rule};
use crate::shared::kernel::error::AgentResult;

/// The assembled extension catalogs, agnostic of the filesystem source. The loader builds it; the
/// composition root (`app::wire`) owns the loader adapter, calls `load`, and injects the result.
#[derive(Debug, Clone, Default)]
pub struct ExtensionCatalog {
    /// The raw resources (for display / debugging), keyed by id.
    pub resources: HashMap<String, Resource>,
    /// Rules, keyed by id. Always-on rules (`Rule::always == true`) are injected into the system prompt;
    /// on-demand rules are addressable by skills/commands (future).
    pub rules: Vec<Rule>,
    /// Custom slash commands, keyed by their slash-prefixed name (e.g. `/test`). An alias entry is
    /// resolved to the owning command via an auxiliary map (built at integration time).
    pub commands: HashMap<String, ExtCommandSpec>,
    /// One alias → owning command name map, built alongside `commands`.
    pub command_aliases: HashMap<String, String>,
}

impl ExtensionCatalog {
    /// The merged text of every always-on rule, in layer order (global first, project after), for
    /// injection into the system prompt. An empty string when there are no always-on rules.
    pub fn render_rules(&self) -> String {
        let mut lines: Vec<&str> = Vec::new();
        for rule in &self.rules {
            if rule.always {
                lines.push(rule.body.as_str());
            }
        }
        if lines.is_empty() {
            return String::new();
        }
        // The bodies are already trimmed at load time; join with a newline separator.
        lines.join("\n\n")
    }
}

/// The capability port: discover and load extension resources from global and project layers.
/// Async so a future MCP-discovery handshake (Fase 5) is supported; the current filesystem loader
/// blocks once during boot and is async-compatible.
#[async_trait::async_trait]
pub trait ExtensionsLoader: Send + Sync {
    /// Discover and load all extension resources, returning an assembled catalog. The loader scans
    /// `~/.kiri/{rules,commands,...}` (global, trusted) then `<workspace>/.kiri/{rules,commands,...}`
    /// (project, untrusted), merging by id. Global rules/commands load first; a project entry with the
    /// same id extends (passive) or is held behind the gate (active capability).
    async fn load(&self) -> AgentResult<ExtensionCatalog>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::extensions::domain::scope::Layer;

    #[test]
    fn empty_catalog_yields_no_rules_text() {
        let catalog = ExtensionCatalog::default();
        assert!(catalog.render_rules().is_empty());
    }

    #[test]
    fn render_rules_joins_always_on_bodies() {
        let catalog = ExtensionCatalog {
            rules: vec![
                Rule {
                    id: "r1".into(),
                    always: true,
                    body: "Always use Rust.".into(),
                    layer: Layer::Global,
                    path: "/fake/r1.md".into(),
                    tags: Default::default(),
                },
                Rule {
                    id: "r2".into(),
                    always: false,
                    body: "On demand only.".into(),
                    layer: Layer::Project,
                    path: "/fake/r2.md".into(),
                    tags: Default::default(),
                },
                Rule {
                    id: "r3".into(),
                    always: true,
                    body: "Write senior-level code.".into(),
                    layer: Layer::Project,
                    path: "/fake/r3.md".into(),
                    tags: Default::default(),
                },
            ],
            ..ExtensionCatalog::default()
        };
        let rendered = catalog.render_rules();
        assert!(rendered.contains("Always use Rust."));
        assert!(rendered.contains("Write senior-level code."));
        assert!(!rendered.contains("On demand only."));
        // Two always-on bodies are joined by `\n\n`.
        assert!(rendered.contains("\n\n"));
    }
}
