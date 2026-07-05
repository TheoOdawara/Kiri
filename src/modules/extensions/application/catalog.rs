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

    /// The `/rules` display text: one line per loaded rule (id, layer, always-on marker, source path),
    /// sorted by id for a stable listing. `None` when no rules were loaded.
    pub fn rules_display(&self) -> Option<String> {
        if self.rules.is_empty() {
            return None;
        }
        let mut rules: Vec<&Rule> = self.rules.iter().collect();
        rules.sort_by(|a, b| a.id.cmp(&b.id));
        let lines: Vec<String> = rules
            .iter()
            .map(|rule| {
                let mode = if rule.always {
                    "always-on"
                } else {
                    "on-demand"
                };
                format!(
                    "- {} [{}, {}] {}",
                    rule.id,
                    rule.layer.label(),
                    mode,
                    rule.path
                )
            })
            .collect();
        Some(lines.join("\n"))
    }

    /// The `/commands` display text: one line per loaded custom command (name, aliases, layer, source
    /// path), sorted by name for a stable listing. `None` when no commands were loaded.
    pub fn commands_display(&self) -> Option<String> {
        if self.commands.is_empty() {
            return None;
        }
        let mut commands: Vec<&ExtCommandSpec> = self.commands.values().collect();
        commands.sort_by(|a, b| a.name.cmp(&b.name));
        let lines: Vec<String> = commands
            .iter()
            .map(|command| {
                let aliases = if command.aliases.is_empty() {
                    String::new()
                } else {
                    format!(" (aliases: {})", command.aliases.join(", "))
                };
                format!(
                    "- {}{} [{}] {}",
                    command.name,
                    aliases,
                    command.layer.label(),
                    command.path
                )
            })
            .collect();
        Some(lines.join("\n"))
    }

    /// Every command token (canonical name + aliases) mapped straight to its expanded prompt body, so
    /// submit-time lookup is a single hit regardless of which alias the user typed.
    pub fn command_bodies(&self) -> HashMap<String, String> {
        let mut bodies = HashMap::with_capacity(self.commands.len());
        for command in self.commands.values() {
            bodies.insert(command.name.clone(), command.body.clone());
        }
        for (alias, owner) in &self.command_aliases {
            if let Some(command) = self.commands.get(owner) {
                bodies.insert(alias.clone(), command.body.clone());
            }
        }
        bodies
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

    #[test]
    fn empty_catalog_yields_no_rules_display() {
        assert!(ExtensionCatalog::default().rules_display().is_none());
    }

    #[test]
    fn rules_display_lists_every_rule_with_layer_and_mode() {
        let catalog = ExtensionCatalog {
            rules: vec![
                Rule {
                    id: "style".into(),
                    always: true,
                    body: "Use Rust fmt.".into(),
                    layer: Layer::Global,
                    path: "/fake/style.md".into(),
                    tags: Default::default(),
                },
                Rule {
                    id: "team".into(),
                    always: false,
                    body: "Prefer async.".into(),
                    layer: Layer::Project,
                    path: "/fake/team.md".into(),
                    tags: Default::default(),
                },
            ],
            ..ExtensionCatalog::default()
        };
        let display = catalog.rules_display().unwrap();
        assert!(display.contains("style [global, always-on]"));
        assert!(display.contains("team [project, on-demand]"));
    }

    #[test]
    fn command_bodies_resolves_aliases_to_the_owning_body() {
        let mut commands = HashMap::new();
        commands.insert(
            "/test".to_string(),
            ExtCommandSpec {
                name: "/test".into(),
                aliases: vec!["/t".into()],
                description: "Run tests".into(),
                body: "Run the suite.".into(),
                layer: Layer::Global,
                path: "/fake/test.md".into(),
                agent: None,
                model: None,
                allowed_tools: Vec::new(),
            },
        );
        let mut command_aliases = HashMap::new();
        command_aliases.insert("/t".to_string(), "/test".to_string());
        let catalog = ExtensionCatalog {
            commands,
            command_aliases,
            ..ExtensionCatalog::default()
        };
        let bodies = catalog.command_bodies();
        assert_eq!(
            bodies.get("/test").map(String::as_str),
            Some("Run the suite.")
        );
        assert_eq!(bodies.get("/t").map(String::as_str), Some("Run the suite."));
    }

    #[test]
    fn empty_catalog_yields_no_commands_display() {
        assert!(ExtensionCatalog::default().commands_display().is_none());
    }

    #[test]
    fn commands_display_lists_name_aliases_layer_and_path() {
        let mut commands = HashMap::new();
        commands.insert(
            "/test".to_string(),
            ExtCommandSpec {
                name: "/test".into(),
                aliases: vec!["/t".into()],
                description: "Run tests".into(),
                body: "Run the suite.".into(),
                layer: Layer::Project,
                path: "/fake/test.md".into(),
                agent: None,
                model: None,
                allowed_tools: Vec::new(),
            },
        );
        let catalog = ExtensionCatalog {
            commands,
            ..ExtensionCatalog::default()
        };
        let display = catalog.commands_display().unwrap();
        assert!(display.contains("/test (aliases: /t) [project] /fake/test.md"));
    }
}
