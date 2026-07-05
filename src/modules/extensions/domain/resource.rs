use std::collections::HashSet;

use crate::modules::extensions::domain::frontmatter::Frontmatter;
use crate::modules::extensions::domain::scope::Layer;

/// A discovered extension resource: its parsed frontmatter, its Markdown body, the layer it came from,
/// and the filesystem path it was loaded from (carried for display, like `instruction_paths`). Pure
/// data — the loader (infrastructure) constructs it; the domain never reads files.
#[derive(Debug, Clone)]
pub struct Resource {
    /// Stable id from frontmatter (`id:`), falling back to the file stem when absent.
    pub id: String,
    pub frontmatter: Frontmatter,
    /// The Markdown body after the frontmatter block, trimmed.
    pub body: String,
    pub layer: Layer,
    /// The file path it was loaded from, for the `/rules` display and boot diagnostics.
    pub path: String,
}

impl Resource {
    pub fn new(
        id: String,
        frontmatter: Frontmatter,
        body: String,
        layer: Layer,
        path: String,
    ) -> Self {
        Self {
            id,
            frontmatter,
            body,
            layer,
            path,
        }
    }
}

/// A behavioural **rule**: always-on text injected into the system prompt, or on-demand text addressable
/// by skills/commands. Built from a `Resource`; the frontmatter's `always:` decides injection.
#[derive(Debug, Clone)]
pub struct Rule {
    pub id: String,
    /// Whether this rule's body is injected into every turn's system prompt (`always: true`)
    /// or stays indexed for on-demand recall (`false`/absent).
    pub always: bool,
    pub body: String,
    pub layer: Layer,
    pub path: String,
    // ponytail: on-demand rule addressing by tag (skills/commands looking up a rule by topic) has no
    // caller yet — the only routing built so far is the always-on injection. Upgrade path: a
    // `rules_by_tag` lookup once a skill/command needs one.
    #[allow(dead_code)]
    pub tags: HashSet<String>,
}

impl Rule {
    /// Build a rule from a frontmatter-parsed resource. `always` reads the `always` scalar as a
    /// truthy string (`true`/`on`/`yes`/`1`), defaulting to `false` on absence.
    pub fn from_resource(res: &Resource) -> Self {
        let always = res
            .frontmatter
            .get("always")
            .map(is_truthy)
            .unwrap_or(false);
        let tags = res
            .frontmatter
            .list("tags")
            .map(|items| items.iter().cloned().collect())
            .unwrap_or_default();
        Self {
            id: res.id.clone(),
            always,
            body: res.body.clone(),
            layer: res.layer,
            path: res.path.clone(),
            tags,
        }
    }
}

/// A custom slash **command**: a Markdown template expanded into a prompt on invocation, optionally bound
/// to an agent profile, a model override, and a tool allow-list. Built from a `Resource`.
#[derive(Debug, Clone)]
pub struct CommandSpec {
    /// The invocation token with leading slash, e.g. `/test`. From frontmatter `name:` (the loader
    /// prepends the slash), falling back to the file stem.
    pub name: String,
    pub aliases: Vec<String>,
    pub description: String,
    pub body: String,
    pub layer: Layer,
    pub path: String,
    /// An agent profile id to run the command under: its system-prompt is prepended to the expanded body
    /// (`ExtensionCatalog::command_bodies`).
    pub agent: Option<String>,
    // ponytail: model/tool-scoping per turn has no engine mechanism yet (AgentLoop/ToolRegistry are
    // session-wide, not per-turn) — upgrade path: thread an optional override through AgentLoop::run.
    #[allow(dead_code)]
    pub model: Option<String>,
    #[allow(dead_code)]
    pub allowed_tools: Vec<String>,
}

impl CommandSpec {
    /// Build a command spec from a frontmatter-parsed resource. The `name` scalar is the bare token
    /// (the loader ensures the leading `/`); aliases are read as a list and normalized the same way.
    pub fn from_resource(res: &Resource) -> Self {
        let bare_name = res
            .frontmatter
            .get("name")
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| res.id.clone());
        let name = ensure_slash(&bare_name);
        let aliases = res
            .frontmatter
            .list("aliases")
            .map(|items| items.iter().map(|a| ensure_slash(a)).collect())
            .unwrap_or_default();
        let description = res
            .frontmatter
            .get("description")
            .map(|s| s.to_string())
            .unwrap_or_default();
        let agent = res
            .frontmatter
            .get("agent")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let model = res
            .frontmatter
            .get("model")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let allowed_tools = res
            .frontmatter
            .list("allowed-tools")
            .map(|items| items.to_vec())
            .unwrap_or_default();
        Self {
            name,
            aliases,
            description,
            body: res.body.clone(),
            layer: res.layer,
            path: res.path.clone(),
            agent,
            model,
            allowed_tools,
        }
    }
}

/// An **agent** profile (ADR 0021): a named system-prompt a custom command binds to via its `agent:`
/// field. Built from a `Resource`. Not an isolated sub-agent — the harness runs a single turn loop, so
/// binding a command to an agent means "prepend this system-prompt to the turn", not "spawn a concurrent
/// agent" (see `ExtensionCatalog::command_bodies`).
#[derive(Debug, Clone)]
pub struct AgentProfile {
    pub id: String,
    /// The agent's system-prompt text (the resource body), prepended to a bound command's expanded body.
    pub system_prompt: String,
    pub layer: Layer,
    pub path: String,
    // ponytail: same ceiling as `CommandSpec::model`/`allowed_tools` — no per-turn override mechanism yet.
    #[allow(dead_code)]
    pub model: Option<String>,
    #[allow(dead_code)]
    pub allowed_tools: Vec<String>,
}

impl AgentProfile {
    /// Build an agent profile from a frontmatter-parsed resource.
    pub fn from_resource(res: &Resource) -> Self {
        let model = res
            .frontmatter
            .get("model")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let allowed_tools = res
            .frontmatter
            .list("allowed-tools")
            .map(|items| items.to_vec())
            .unwrap_or_default();
        Self {
            id: res.id.clone(),
            system_prompt: res.body.clone(),
            layer: res.layer,
            path: res.path.clone(),
            model,
            allowed_tools,
        }
    }
}

/// A **skill**: on-demand instructions the model pulls via the `use_skill(name)` tool rather than always
/// carrying the full body in the system prompt. `description` is the one-line entry shown in the always-on
/// skill index; `body` is returned only when invoked. Built from a `Resource`.
#[derive(Debug, Clone)]
pub struct Skill {
    pub id: String,
    pub description: String,
    pub body: String,
    pub layer: Layer,
    pub path: String,
    pub tags: HashSet<String>,
    // ponytail: a skill script is an active capability (shell execution) — no hooks/gate machinery exists
    // yet to run it safely. Upgrade path: the hooks context (Fase 4) executes it behind the trust gate.
    #[allow(dead_code)]
    pub script: Option<String>,
}

impl Skill {
    /// Build a skill from a frontmatter-parsed resource.
    pub fn from_resource(res: &Resource) -> Self {
        let description = res
            .frontmatter
            .get("description")
            .map(|s| s.to_string())
            .unwrap_or_default();
        let tags = res
            .frontmatter
            .list("tags")
            .map(|items| items.iter().cloned().collect())
            .unwrap_or_default();
        let script = res
            .frontmatter
            .get("script")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        Self {
            id: res.id.clone(),
            description,
            body: res.body.clone(),
            layer: res.layer,
            path: res.path.clone(),
            tags,
            script,
        }
    }
}

/// The lifecycle point a hook fires on. `SessionStart`/`SessionEnd`/`TurnEnd` dispatch from existing
/// async funnels (`app::wire`/`Tui::run` boot and teardown, `on_turn_end`).
// ponytail: PreToolUse/PostToolUse parse and gate correctly but have no dispatcher yet — ToolObserver's
// callbacks (`tool_started`/`tool_finished`) are synchronous, so firing a hook there needs new
// spawn+channel plumbing into Bridge, a follow-up rather than folded into this pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    SessionStart,
    SessionEnd,
    TurnEnd,
    #[allow(dead_code)]
    PreToolUse,
    #[allow(dead_code)]
    PostToolUse,
}

impl HookEvent {
    fn from_str(value: &str) -> Option<Self> {
        match value.trim() {
            "SessionStart" => Some(Self::SessionStart),
            "SessionEnd" => Some(Self::SessionEnd),
            "TurnEnd" => Some(Self::TurnEnd),
            "PreToolUse" => Some(Self::PreToolUse),
            "PostToolUse" => Some(Self::PostToolUse),
            _ => None,
        }
    }
}

/// A **hook** (ADR 0021): a shell command run at a lifecycle event — an active capability, gated by
/// `domain::gate` for project-layer hooks (global ones auto-approve). Built from a `Resource`; the
/// command is the resource body (like `AgentProfile::system_prompt`/`Skill::body`), the event and
/// optional tool-name matcher (`PreToolUse`/`PostToolUse` only) come from frontmatter.
#[derive(Debug, Clone)]
pub struct Hook {
    pub id: String,
    pub event: HookEvent,
    #[allow(dead_code)] // consumed once PreToolUse/PostToolUse dispatch lands
    pub matcher: Option<String>,
    pub command: String,
    pub layer: Layer,
    pub path: String,
}

impl Hook {
    /// Build a hook from a frontmatter-parsed resource. `None` when `event:` is missing/unrecognized or
    /// the body (the command to run) is blank — a malformed hook file is dropped, never aborts boot.
    pub fn from_resource(res: &Resource) -> Option<Self> {
        let event = res.frontmatter.get("event").and_then(HookEvent::from_str)?;
        if res.body.is_empty() {
            return None;
        }
        let matcher = res
            .frontmatter
            .get("matcher")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        Some(Self {
            id: res.id.clone(),
            event,
            matcher,
            command: res.body.clone(),
            layer: res.layer,
            path: res.path.clone(),
        })
    }
}

/// An **MCP server** (ADR 0021): a subprocess speaking the Model Context Protocol over stdio, whose
/// tools extend the model's toolset — an active capability, gated like hooks. Built from a `Resource`;
/// `command`/`args` come from frontmatter (no shell involved — spawned as `command` with `args` directly,
/// like `hooks` shells its command, but MCP servers are long-lived processes, not one-shot scripts).
// ponytail: stdio transport only — HTTP/SSE MCP servers are unstarted (ADR 0021's `Http(url)` variant).
// Upgrade path: a second `McpTransport` variant + the matching rmcp streamable-http client feature.
#[derive(Debug, Clone)]
pub struct McpServer {
    pub id: String,
    pub command: String,
    pub args: Vec<String>,
    pub layer: Layer,
    pub path: String,
}

impl McpServer {
    /// Build an MCP server spec from a frontmatter-parsed resource. `None` when `command:` is missing or
    /// blank — a malformed server file is dropped rather than aborting boot.
    pub fn from_resource(res: &Resource) -> Option<Self> {
        let command = res
            .frontmatter
            .get("command")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())?;
        let args = res
            .frontmatter
            .list("args")
            .map(|items| items.to_vec())
            .unwrap_or_default();
        Some(Self {
            id: res.id.clone(),
            command,
            args,
            layer: res.layer,
            path: res.path.clone(),
        })
    }

    /// The effective command line (`command` plus any `args`, space-joined) — the single source for
    /// display (`ExtensionCatalog::mcp_display`) and for the TOFU content hash (`domain::gate::
    /// content_hash`), so both always agree on what "this server's content" means.
    pub fn command_line(&self) -> String {
        if self.args.is_empty() {
            self.command.clone()
        } else {
            format!("{} {}", self.command, self.args.join(" "))
        }
    }
}

/// Truthy-string check shared with the frontmatter `always:` reader.
fn is_truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "true" | "on" | "yes" | "1"
    )
}

/// Ensure a command token starts with a single leading `/`. A token already so-prefixed is unchanged;
/// one prefixed with multiple slashes is normalized to one.
fn ensure_slash(name: &str) -> String {
    let trimmed = name.trim();
    match trimmed.strip_prefix('/') {
        Some(rest) => format!("/{}", rest.trim_start_matches('/')),
        None => format!("/{trimmed}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resource(id: &str, front: &str, body: &str, layer: Layer) -> Resource {
        let (frontmatter, _) = Frontmatter::parse(front);
        Resource {
            id: id.to_string(),
            frontmatter,
            body: body.to_string(),
            layer,
            path: format!("/fake/{id}.md"),
        }
    }

    #[test]
    fn rule_defaults_to_on_demand_and_reads_tags() {
        let res = resource(
            "clippy",
            "---\nalways: true\ntags:\n  - rust\n  - lint\n---\n",
            "Use clippy.",
            Layer::Global,
        );
        let rule = Rule::from_resource(&res);
        assert!(rule.always);
        assert!(rule.tags.contains("rust"));
        assert!(rule.tags.contains("lint"));
    }

    #[test]
    fn rule_absent_always_is_on_demand() {
        let res = resource("doc", "---\n---\n", "Write docs.", Layer::Project);
        let rule = Rule::from_resource(&res);
        assert!(!rule.always);
    }

    #[test]
    fn command_normalizes_name_and_aliases() {
        let res = resource(
            "test",
            "---\nname: test\naliases:\n  - t\n  - run\ndescription: Run tests\n---\n",
            "Run the suite.",
            Layer::Global,
        );
        let cmd = CommandSpec::from_resource(&res);
        assert_eq!(cmd.name, "/test");
        assert_eq!(cmd.aliases, ["/t", "/run"]);
        assert_eq!(cmd.description, "Run tests");
    }

    #[test]
    fn command_strips_redundant_slashes() {
        let res = resource("x", "---\nname: //x\n---\n", "body", Layer::Project);
        let cmd = CommandSpec::from_resource(&res);
        assert_eq!(cmd.name, "/x");
    }

    #[test]
    fn command_reads_reserved_bindings() {
        let res = resource(
            "deep",
            "---\nname: deep\nagent: researcher\nmodel: gpt-pro\nallowed-tools:\n  - read_file\n  - search\n---\n",
            "Do deep work.",
            Layer::Global,
        );
        let cmd = CommandSpec::from_resource(&res);
        assert_eq!(cmd.agent.as_deref(), Some("researcher"));
        assert_eq!(cmd.model.as_deref(), Some("gpt-pro"));
        assert_eq!(cmd.allowed_tools, ["read_file", "search"]);
    }

    #[test]
    fn command_blank_bindings_are_none_or_empty() {
        let res = resource(
            "thin",
            "---\nname: thin\nagent: \"\"\nmodel:   \n---\n",
            "body",
            Layer::Global,
        );
        let cmd = CommandSpec::from_resource(&res);
        assert!(cmd.agent.is_none());
        assert!(cmd.model.is_none());
        assert!(cmd.allowed_tools.is_empty());
    }

    #[test]
    fn agent_profile_reads_system_prompt_and_bindings() {
        let res = resource(
            "researcher",
            "---\nmodel: gpt-pro\nallowed-tools:\n  - search\n---\n",
            "You are a deep-research agent.",
            Layer::Global,
        );
        let agent = AgentProfile::from_resource(&res);
        assert_eq!(agent.id, "researcher");
        assert_eq!(agent.system_prompt, "You are a deep-research agent.");
        assert_eq!(agent.model.as_deref(), Some("gpt-pro"));
        assert_eq!(agent.allowed_tools, ["search"]);
    }

    #[test]
    fn skill_reads_description_tags_and_script() {
        let res = resource(
            "pdf-extract",
            "---\ndescription: Extract text from PDFs\ntags:\n  - pdf\n  - docs\nscript: extract.py\n---\n",
            "Use pdftotext for extraction.",
            Layer::Project,
        );
        let skill = Skill::from_resource(&res);
        assert_eq!(skill.description, "Extract text from PDFs");
        assert!(skill.tags.contains("pdf"));
        assert!(skill.tags.contains("docs"));
        assert_eq!(skill.script.as_deref(), Some("extract.py"));
    }

    #[test]
    fn skill_absent_script_is_none() {
        let res = resource("doc", "---\n---\n", "Write docs.", Layer::Project);
        let skill = Skill::from_resource(&res);
        assert!(skill.script.is_none());
    }

    #[test]
    fn hook_reads_event_matcher_and_command_body() {
        let res = resource(
            "notify",
            "---\nevent: SessionStart\n---\n",
            "echo hello",
            Layer::Global,
        );
        let hook = Hook::from_resource(&res).unwrap();
        assert_eq!(hook.event, HookEvent::SessionStart);
        assert_eq!(hook.command, "echo hello");
        assert!(hook.matcher.is_none());
    }

    #[test]
    fn hook_reads_the_tool_matcher() {
        let res = resource(
            "audit-writes",
            "---\nevent: PreToolUse\nmatcher: write_file\n---\n",
            "echo about to write",
            Layer::Project,
        );
        let hook = Hook::from_resource(&res).unwrap();
        assert_eq!(hook.event, HookEvent::PreToolUse);
        assert_eq!(hook.matcher.as_deref(), Some("write_file"));
    }

    #[test]
    fn hook_missing_event_is_none() {
        let res = resource("x", "---\n---\n", "echo hi", Layer::Global);
        assert!(Hook::from_resource(&res).is_none());
    }

    #[test]
    fn hook_unrecognized_event_is_none() {
        let res = resource(
            "x",
            "---\nevent: SomethingElse\n---\n",
            "echo hi",
            Layer::Global,
        );
        assert!(Hook::from_resource(&res).is_none());
    }

    #[test]
    fn hook_blank_command_is_none() {
        let res = resource("x", "---\nevent: SessionStart\n---\n", "", Layer::Global);
        assert!(Hook::from_resource(&res).is_none());
    }

    #[test]
    fn mcp_server_reads_command_and_args() {
        let res = resource(
            "filesystem",
            "---\ncommand: npx\nargs:\n  - -y\n  - \"@modelcontextprotocol/server-filesystem\"\n---\n",
            "",
            Layer::Global,
        );
        let server = McpServer::from_resource(&res).unwrap();
        assert_eq!(server.command, "npx");
        assert_eq!(
            server.args,
            ["-y", "@modelcontextprotocol/server-filesystem"]
        );
    }

    #[test]
    fn mcp_server_missing_command_is_none() {
        let res = resource("x", "---\n---\n", "", Layer::Project);
        assert!(McpServer::from_resource(&res).is_none());
    }

    #[test]
    fn mcp_server_blank_command_is_none() {
        let res = resource("x", "---\ncommand: \"\"\n---\n", "", Layer::Project);
        assert!(McpServer::from_resource(&res).is_none());
    }
}
