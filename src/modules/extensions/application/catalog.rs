use std::collections::HashMap;

use crate::modules::extensions::domain::resource::{
    AgentProfile, CommandSpec as ExtCommandSpec, Hook, HookEvent, Resource, Rule, Skill,
};
use crate::shared::kernel::error::AgentResult;

/// The assembled extension catalogs, agnostic of the filesystem source. The loader builds it; the
/// composition root (`app::wire`) owns the loader adapter, calls `load`, and injects the result.
#[derive(Debug, Clone, Default)]
pub struct ExtensionCatalog {
    // ponytail: kept for future `/extensions` debug display (raw resources, both types, one place);
    // nothing reads it yet — `/rules`/`/commands`/`/agents`/`/skills` each already display their own kind.
    #[allow(dead_code)]
    pub resources: HashMap<String, Resource>,
    /// Rules, keyed by id. Always-on rules (`Rule::always == true`) are injected into the system prompt;
    /// on-demand rules are addressable by skills/commands (future).
    pub rules: Vec<Rule>,
    /// Custom slash commands, keyed by their slash-prefixed name (e.g. `/test`). An alias entry is
    /// resolved to the owning command via an auxiliary map (built at integration time).
    pub commands: HashMap<String, ExtCommandSpec>,
    /// One alias → owning command name map, built alongside `commands`.
    pub command_aliases: HashMap<String, String>,
    /// Agent profiles, keyed by id. A command's `agent:` field binds it to one.
    pub agents: HashMap<String, AgentProfile>,
    /// Skills, keyed by id. Their descriptions render into the system prompt's skill index; bodies are
    /// fetched on demand by the `use_skill` tool.
    pub skills: HashMap<String, Skill>,
    /// Hooks, keyed by id. Global hooks auto-approve; project ones need the trust gate.
    pub hooks: HashMap<String, Hook>,
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

    /// Every command token (canonical name + aliases) mapped straight to its expanded prompt body — the
    /// command's own body, with its bound agent's system-prompt prepended when it has one (`agent:`
    /// names an unknown id is treated as unbound). Submit-time lookup is a single hit regardless of which
    /// alias the user typed.
    pub fn command_bodies(&self) -> HashMap<String, String> {
        let mut bodies = HashMap::with_capacity(self.commands.len());
        for command in self.commands.values() {
            bodies.insert(command.name.clone(), self.expand_command_body(command));
        }
        for (alias, owner) in &self.command_aliases {
            if let Some(command) = self.commands.get(owner) {
                bodies.insert(alias.clone(), self.expand_command_body(command));
            }
        }
        bodies
    }

    fn expand_command_body(&self, command: &ExtCommandSpec) -> String {
        match command.agent.as_ref().and_then(|id| self.agents.get(id)) {
            Some(agent) => format!("{}\n\n{}", agent.system_prompt, command.body),
            None => command.body.clone(),
        }
    }

    /// The `/agents` display text: one line per loaded agent profile (id, layer, source path), sorted by
    /// id. `None` when no agents were loaded.
    pub fn agents_display(&self) -> Option<String> {
        if self.agents.is_empty() {
            return None;
        }
        let mut agents: Vec<&AgentProfile> = self.agents.values().collect();
        agents.sort_by(|a, b| a.id.cmp(&b.id));
        let lines: Vec<String> = agents
            .iter()
            .map(|agent| format!("- {} [{}] {}", agent.id, agent.layer.label(), agent.path))
            .collect();
        Some(lines.join("\n"))
    }

    /// The `/skills` display text: one line per loaded skill (id, layer, tags, source path), sorted by
    /// id. `None` when no skills were loaded.
    pub fn skills_display(&self) -> Option<String> {
        if self.skills.is_empty() {
            return None;
        }
        let mut skills: Vec<&Skill> = self.skills.values().collect();
        skills.sort_by(|a, b| a.id.cmp(&b.id));
        let lines: Vec<String> = skills
            .iter()
            .map(|skill| {
                let tags = if skill.tags.is_empty() {
                    String::new()
                } else {
                    let mut sorted: Vec<&str> = skill.tags.iter().map(String::as_str).collect();
                    sorted.sort_unstable();
                    format!(" (tags: {})", sorted.join(", "))
                };
                format!(
                    "- {}{} [{}] {}",
                    skill.id,
                    tags,
                    skill.layer.label(),
                    skill.path
                )
            })
            .collect();
        Some(lines.join("\n"))
    }

    /// The always-on skill index for the system prompt: one `name — description` line per skill, sorted
    /// by id, so the model knows what `use_skill` can fetch without carrying every body up front. `None`
    /// when no skills were loaded.
    pub fn skills_index(&self) -> Option<String> {
        if self.skills.is_empty() {
            return None;
        }
        let mut skills: Vec<&Skill> = self.skills.values().collect();
        skills.sort_by(|a, b| a.id.cmp(&b.id));
        let lines: Vec<String> = skills
            .iter()
            .map(|skill| format!("- {} — {}", skill.id, skill.description))
            .collect();
        Some(lines.join("\n"))
    }

    /// The hooks bound to `event`, sorted by id for a stable, deterministic firing order.
    pub fn hooks_for_event(&self, event: HookEvent) -> Vec<&Hook> {
        let mut hooks: Vec<&Hook> = self.hooks.values().filter(|h| h.event == event).collect();
        hooks.sort_by(|a, b| a.id.cmp(&b.id));
        hooks
    }

    /// The `/hooks` display text: one line per loaded hook (id, event, layer, source path), sorted by id.
    /// `None` when no hooks were loaded.
    pub fn hooks_display(&self) -> Option<String> {
        if self.hooks.is_empty() {
            return None;
        }
        let mut hooks: Vec<&Hook> = self.hooks.values().collect();
        hooks.sort_by(|a, b| a.id.cmp(&b.id));
        let lines: Vec<String> = hooks
            .iter()
            .map(|hook| {
                format!(
                    "- {} [{:?}, {}] {}",
                    hook.id,
                    hook.event,
                    hook.layer.label(),
                    hook.path
                )
            })
            .collect();
        Some(lines.join("\n"))
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

    fn agent(id: &str, system_prompt: &str) -> AgentProfile {
        AgentProfile {
            id: id.to_string(),
            system_prompt: system_prompt.to_string(),
            layer: Layer::Global,
            path: format!("/fake/{id}.md"),
            model: None,
            allowed_tools: Vec::new(),
        }
    }

    fn command(name: &str, body: &str, agent: Option<&str>) -> ExtCommandSpec {
        ExtCommandSpec {
            name: name.to_string(),
            aliases: Vec::new(),
            description: String::new(),
            body: body.to_string(),
            layer: Layer::Project,
            path: format!("/fake{name}.md"),
            agent: agent.map(str::to_string),
            model: None,
            allowed_tools: Vec::new(),
        }
    }

    #[test]
    fn command_bodies_prepends_the_bound_agent_system_prompt() {
        let mut agents = HashMap::new();
        agents.insert(
            "researcher".to_string(),
            agent("researcher", "You are a deep-research agent."),
        );
        let mut commands = HashMap::new();
        commands.insert(
            "/deep".to_string(),
            command("/deep", "Investigate thoroughly.", Some("researcher")),
        );
        let catalog = ExtensionCatalog {
            agents,
            commands,
            ..ExtensionCatalog::default()
        };
        let bodies = catalog.command_bodies();
        assert_eq!(
            bodies.get("/deep").map(String::as_str),
            Some("You are a deep-research agent.\n\nInvestigate thoroughly.")
        );
    }

    #[test]
    fn command_bodies_ignores_an_unknown_agent_id() {
        let mut commands = HashMap::new();
        commands.insert(
            "/deep".to_string(),
            command("/deep", "Investigate thoroughly.", Some("ghost")),
        );
        let catalog = ExtensionCatalog {
            commands,
            ..ExtensionCatalog::default()
        };
        let bodies = catalog.command_bodies();
        assert_eq!(
            bodies.get("/deep").map(String::as_str),
            Some("Investigate thoroughly.")
        );
    }

    #[test]
    fn empty_catalog_yields_no_agents_display() {
        assert!(ExtensionCatalog::default().agents_display().is_none());
    }

    #[test]
    fn agents_display_lists_id_layer_and_path() {
        let mut agents = HashMap::new();
        agents.insert("researcher".to_string(), agent("researcher", "..."));
        let catalog = ExtensionCatalog {
            agents,
            ..ExtensionCatalog::default()
        };
        let display = catalog.agents_display().unwrap();
        assert!(display.contains("- researcher [global] /fake/researcher.md"));
    }

    fn skill(id: &str, description: &str, tags: &[&str]) -> Skill {
        Skill {
            id: id.to_string(),
            description: description.to_string(),
            body: format!("{id} body"),
            layer: Layer::Project,
            path: format!("/fake/{id}.md"),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            script: None,
        }
    }

    #[test]
    fn empty_catalog_yields_no_skills_display_or_index() {
        let catalog = ExtensionCatalog::default();
        assert!(catalog.skills_display().is_none());
        assert!(catalog.skills_index().is_none());
    }

    #[test]
    fn skills_display_lists_id_tags_layer_and_path() {
        let mut skills = HashMap::new();
        skills.insert(
            "pdf-extract".to_string(),
            skill("pdf-extract", "Extract text from PDFs", &["pdf", "docs"]),
        );
        let catalog = ExtensionCatalog {
            skills,
            ..ExtensionCatalog::default()
        };
        let display = catalog.skills_display().unwrap();
        assert!(display.contains("- pdf-extract (tags: docs, pdf) [project] /fake/pdf-extract.md"));
    }

    #[test]
    fn skills_index_lists_id_and_description() {
        let mut skills = HashMap::new();
        skills.insert(
            "pdf-extract".to_string(),
            skill("pdf-extract", "Extract text from PDFs", &[]),
        );
        let catalog = ExtensionCatalog {
            skills,
            ..ExtensionCatalog::default()
        };
        let index = catalog.skills_index().unwrap();
        assert_eq!(index, "- pdf-extract — Extract text from PDFs");
    }

    fn hook(id: &str, event: HookEvent, layer: Layer) -> Hook {
        Hook {
            id: id.to_string(),
            event,
            matcher: None,
            command: format!("echo {id}"),
            layer,
            path: format!("/fake/{id}.md"),
        }
    }

    #[test]
    fn empty_catalog_yields_no_hooks_display_and_no_hooks_for_any_event() {
        let catalog = ExtensionCatalog::default();
        assert!(catalog.hooks_display().is_none());
        assert!(catalog.hooks_for_event(HookEvent::SessionStart).is_empty());
    }

    #[test]
    fn hooks_for_event_filters_and_sorts_by_id() {
        let mut hooks = HashMap::new();
        hooks.insert(
            "b-notify".to_string(),
            hook("b-notify", HookEvent::SessionStart, Layer::Global),
        );
        hooks.insert(
            "a-notify".to_string(),
            hook("a-notify", HookEvent::SessionStart, Layer::Global),
        );
        hooks.insert(
            "end-notify".to_string(),
            hook("end-notify", HookEvent::SessionEnd, Layer::Global),
        );
        let catalog = ExtensionCatalog {
            hooks,
            ..ExtensionCatalog::default()
        };
        let start_hooks = catalog.hooks_for_event(HookEvent::SessionStart);
        assert_eq!(
            start_hooks
                .iter()
                .map(|h| h.id.as_str())
                .collect::<Vec<_>>(),
            ["a-notify", "b-notify"]
        );
        assert_eq!(catalog.hooks_for_event(HookEvent::TurnEnd).len(), 0);
    }

    #[test]
    fn hooks_display_lists_id_event_layer_and_path() {
        let mut hooks = HashMap::new();
        hooks.insert(
            "notify".to_string(),
            hook("notify", HookEvent::SessionStart, Layer::Project),
        );
        let catalog = ExtensionCatalog {
            hooks,
            ..ExtensionCatalog::default()
        };
        let display = catalog.hooks_display().unwrap();
        assert!(display.contains("- notify [SessionStart, project] /fake/notify.md"));
    }
}
