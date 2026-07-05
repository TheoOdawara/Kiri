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
    /// Reserved for Fase 3/5 binding: an agent profile id or MCP server to run the command under.
    pub agent: Option<String>,
    pub model: Option<String>,
    /// Reserved for Fase 3: a tool allow-list the invocation restricts the turn to.
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
}
