use std::collections::HashMap;

use crate::modules::extensions::domain::resource::{
    AgentProfile, CommandSpec as ExtCommandSpec, Hook, HookEvent, McpServer, Resource, Rule, Skill,
};
use crate::shared::kernel::error::AgentResult;

/// The assembled extension catalogs, agnostic of the filesystem source.
#[derive(Debug, Clone, Default)]
pub struct ExtensionCatalog {
    // ponytail: kept for future `/extensions` debug display (raw resources, both types, one place);
    // nothing reads it yet — `/rules`/`/commands`/`/agents`/`/skills` each already display their own kind.
    #[allow(dead_code)]
    pub resources: HashMap<String, Resource>,
    /// Always-on rules are injected into the system prompt.
    pub rules: Vec<Rule>,
    /// Keyed by slash-prefixed name, e.g. `/test`.
    pub commands: HashMap<String, ExtCommandSpec>,
    /// Alias → owning command name, built alongside `commands`.
    pub command_aliases: HashMap<String, String>,
    /// Keyed by id. A command's `agent:` field binds it to one.
    pub agents: HashMap<String, AgentProfile>,
    /// Keyed by id. Only descriptions reach the system prompt; bodies are fetched by `use_skill`.
    pub skills: HashMap<String, Skill>,
    /// Keyed by id. Global hooks auto-approve; project ones need the trust gate.
    pub hooks: HashMap<String, Hook>,
    /// Keyed by id. Global servers auto-approve; project ones need the trust gate.
    pub mcp_servers: HashMap<String, McpServer>,
}

impl ExtensionCatalog {
    /// Sorted here because the rules are HashMap-sourced upstream: without it the injected prompt order —
    /// and thus the prompt itself — would vary between boots.
    pub fn render_rules(&self) -> String {
        let mut always_on: Vec<&Rule> = self.rules.iter().filter(|rule| rule.always).collect();
        always_on.sort_by(|a, b| (a.layer.precedence(), &a.id).cmp(&(b.layer.precedence(), &b.id)));
        always_on
            .iter()
            .map(|rule| rule.body.as_str())
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// The `/rules` display text, sorted by id. `None` when none were loaded.
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

    /// The `/commands` display text, sorted by name. `None` when none were loaded.
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

    /// Every command token (canonical name + aliases) mapped straight to its expanded body, so submit-time
    /// lookup is one hit whichever alias the user typed. An `agent:` naming an unknown id is unbound.
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

    /// The `/agents` display text, sorted by id. `None` when none were loaded.
    pub fn agents_display(&self) -> Option<String> {
        if self.agents.is_empty() {
            return None;
        }
        let mut agents: Vec<&AgentProfile> = self.agents.values().collect();
        agents.sort_by(|a, b| a.id.cmp(&b.id));
        let lines: Vec<String> = agents
            .iter()
            .map(|agent| {
                let label = if agent.name != agent.id {
                    format!("{} ({})", agent.id, agent.name)
                } else {
                    agent.id.clone()
                };
                let desc = if agent.description.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", agent.description)
                };
                format!("- {label}{desc} [{}] {}", agent.layer.label(), agent.path)
            })
            .collect();
        Some(lines.join("\n"))
    }

    /// The always-on `# Agents` index, so the model can pick a `task` target by id rather than guess one
    /// (ADR 0029). Mirrors `skills_index`.
    pub fn agents_index(&self) -> Option<String> {
        if self.agents.is_empty() {
            return None;
        }
        let mut agents: Vec<&AgentProfile> = self.agents.values().collect();
        agents.sort_by(|a, b| a.id.cmp(&b.id));
        let lines: Vec<String> = agents
            .iter()
            .map(|agent| format!("- {} — {}", agent.id, agent.description))
            .collect();
        Some(lines.join("\n"))
    }

    /// The `/skills` display text, sorted by id. `None` when none were loaded.
    pub fn skills_display(&self) -> Option<String> {
        if self.skills.is_empty() {
            return None;
        }
        let mut skills: Vec<&Skill> = self.skills.values().collect();
        skills.sort_by(|a, b| a.id.cmp(&b.id));
        let lines: Vec<String> = skills
            .iter()
            .map(|skill| {
                let label = if skill.name != skill.id {
                    format!("{} ({})", skill.id, skill.name)
                } else {
                    skill.id.clone()
                };
                let tags = if skill.tags.is_empty() {
                    String::new()
                } else {
                    let mut sorted: Vec<&str> = skill.tags.iter().map(String::as_str).collect();
                    sorted.sort_unstable();
                    format!(" (tags: {})", sorted.join(", "))
                };
                format!("- {label}{tags} [{}] {}", skill.layer.label(), skill.path)
            })
            .collect();
        Some(lines.join("\n"))
    }

    /// The always-on skill index, so the model knows what `use_skill` can fetch without carrying every
    /// body up front. Keyed by `id` — never `name`, which is display-only and may differ.
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

    /// The `/hooks` display text, sorted by id. `None` when none were loaded.
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

    /// The `/mcp` display text, sorted by id. `None` when none were loaded.
    pub fn mcp_display(&self) -> Option<String> {
        if self.mcp_servers.is_empty() {
            return None;
        }
        let mut servers: Vec<&McpServer> = self.mcp_servers.values().collect();
        servers.sort_by(|a, b| a.id.cmp(&b.id));
        let lines: Vec<String> = servers
            .iter()
            .map(|server| {
                format!(
                    "- {} `{}` [{}] {}",
                    server.id,
                    server.command_line(),
                    server.layer.label(),
                    server.path
                )
            })
            .collect();
        Some(lines.join("\n"))
    }
}

/// Loads extension resources — MCP entries are *specs*, not live connections; the handshake is a separate
/// step run only for gate-approved servers. Async so the filesystem walk never blocks the runtime.
#[async_trait::async_trait]
pub trait ExtensionsLoader: Send + Sync {
    /// Scans global (trusted) then project (untrusted), merging by id. A project entry of the same id
    /// extends a passive resource, or is held behind the gate for an active capability.
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
        assert!(rendered.contains("\n\n"));
    }

    #[test]
    fn render_rules_orders_by_layer_then_id_regardless_of_vec_order() {
        // Inserted in reverse-precedence order, so the sort — not the Vec order — must decide the output.
        let catalog = ExtensionCatalog {
            rules: vec![
                Rule {
                    id: "z-bundled".into(),
                    always: true,
                    body: "BUNDLED".into(),
                    layer: Layer::Bundled,
                    path: "<bundled>/rules/z.md".into(),
                    tags: Default::default(),
                },
                Rule {
                    id: "m-project".into(),
                    always: true,
                    body: "PROJECT".into(),
                    layer: Layer::Project,
                    path: "/fake/m.md".into(),
                    tags: Default::default(),
                },
                Rule {
                    id: "a-global".into(),
                    always: true,
                    body: "GLOBAL".into(),
                    layer: Layer::Global,
                    path: "/fake/a.md".into(),
                    tags: Default::default(),
                },
            ],
            ..ExtensionCatalog::default()
        };
        assert_eq!(catalog.render_rules(), "GLOBAL\n\nPROJECT\n\nBUNDLED");
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
            name: id.to_string(),
            description: format!("{id} description"),
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
    fn agents_display_lists_id_description_layer_and_path() {
        let mut agents = HashMap::new();
        agents.insert("researcher".to_string(), agent("researcher", "..."));
        let catalog = ExtensionCatalog {
            agents,
            ..ExtensionCatalog::default()
        };
        let display = catalog.agents_display().unwrap();
        assert!(
            display.contains("- researcher — researcher description [global] /fake/researcher.md")
        );
    }

    #[test]
    fn agents_display_shows_the_name_when_it_differs_from_the_id() {
        let mut profile = agent("researcher", "...");
        profile.name = "Deep Researcher".to_string();
        let mut agents = HashMap::new();
        agents.insert("researcher".to_string(), profile);
        let catalog = ExtensionCatalog {
            agents,
            ..ExtensionCatalog::default()
        };
        let display = catalog.agents_display().unwrap();
        assert!(display.contains("- researcher (Deep Researcher) — researcher description"));
    }

    #[test]
    fn empty_catalog_yields_no_agents_index() {
        assert!(ExtensionCatalog::default().agents_index().is_none());
    }

    #[test]
    fn agents_index_lists_id_and_description() {
        let mut agents = HashMap::new();
        agents.insert("researcher".to_string(), agent("researcher", "..."));
        let catalog = ExtensionCatalog {
            agents,
            ..ExtensionCatalog::default()
        };
        let index = catalog.agents_index().unwrap();
        assert_eq!(index, "- researcher — researcher description");
    }

    fn skill(id: &str, description: &str, tags: &[&str]) -> Skill {
        Skill {
            id: id.to_string(),
            name: id.to_string(),
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
    fn skills_display_shows_the_name_when_it_differs_from_the_id() {
        let mut s = skill("pdf-extract", "Extract text from PDFs", &["pdf"]);
        s.name = "PDF Extractor".to_string();
        let mut skills = HashMap::new();
        skills.insert("pdf-extract".to_string(), s);
        let catalog = ExtensionCatalog {
            skills,
            ..ExtensionCatalog::default()
        };
        let display = catalog.skills_display().unwrap();
        assert!(display.contains("- pdf-extract (PDF Extractor) (tags: pdf) [project]"));
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

    fn mcp_server(id: &str, command: &str, args: &[&str], layer: Layer) -> McpServer {
        McpServer {
            id: id.to_string(),
            command: command.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
            layer,
            path: format!("/fake/{id}.md"),
        }
    }

    #[test]
    fn empty_catalog_yields_no_mcp_display() {
        assert!(ExtensionCatalog::default().mcp_display().is_none());
    }

    #[test]
    fn mcp_display_lists_id_command_layer_and_path() {
        let mut mcp_servers = HashMap::new();
        mcp_servers.insert(
            "filesystem".to_string(),
            mcp_server("filesystem", "npx", &["-y", "server-fs"], Layer::Global),
        );
        let catalog = ExtensionCatalog {
            mcp_servers,
            ..ExtensionCatalog::default()
        };
        let display = catalog.mcp_display().unwrap();
        assert!(display.contains("- filesystem `npx -y server-fs` [global] /fake/filesystem.md"));
    }
}
