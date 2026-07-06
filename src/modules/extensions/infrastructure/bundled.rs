//! Default extension resources shipped inside the binary (ADR 0028): skills and agent profiles that make
//! Kiri useful from the first prompt, with no filesystem setup required. Content lives as plain Markdown
//! files under `bundled/`, compiled in via `include_str!` — the same pattern already used for the config
//! source characterization test (`shared/infra/config.rs`). Parsing reuses `Frontmatter::parse` and the
//! exact id-resolution rule `file_loader::load_one` uses for a disk file, so a bundled `Resource` is
//! indistinguishable in shape from one loaded off disk; every downstream consumer (`skills_index`,
//! `command_bodies`, the `/skills`/`/agents` displays) treats it uniformly.

use crate::modules::extensions::domain::frontmatter::Frontmatter;
use crate::modules::extensions::domain::resource::Resource;
use crate::modules::extensions::domain::scope::Layer;

/// One bundled resource: its resource-type subdirectory name (matching `file_loader::RESOURCE_TYPES`),
/// its file stem (the id fallback, same rule as a disk file), and its Markdown content.
const BUNDLED: &[(&str, &str, &str)] = &[
    ("skills", "plano", include_str!("bundled/skills/plano.md")),
    ("skills", "gh", include_str!("bundled/skills/gh.md")),
    ("skills", "commit", include_str!("bundled/skills/commit.md")),
    (
        "skills",
        "ponytail",
        include_str!("bundled/skills/ponytail.md"),
    ),
    ("agents", "search", include_str!("bundled/agents/search.md")),
    (
        "agents",
        "planning",
        include_str!("bundled/agents/planning.md"),
    ),
];

/// Parse one bundled entry into a `Resource`, mirroring `file_loader::load_one` minus the I/O: the id
/// comes from frontmatter `id:`, falling back to the file stem; the body is trimmed; the layer is
/// `Bundled`; the display path is a synthetic `<bundled>/{type}/{stem}.md` (there is no real filesystem
/// path to show).
fn parse_bundled(type_name: &str, stem: &str, content: &str) -> Resource {
    let (frontmatter, body) = Frontmatter::parse(content);
    let id = frontmatter
        .get("id")
        .map(|s| s.to_string())
        .unwrap_or_else(|| stem.to_string());
    let path = format!("<bundled>/{type_name}/{stem}.md");
    Resource::new(
        id,
        frontmatter,
        body.trim().to_string(),
        Layer::Bundled,
        path,
    )
}

/// Every bundled resource of `type_name`, in declaration order.
pub fn bundled_for(type_name: &str) -> impl Iterator<Item = Resource> + '_ {
    BUNDLED
        .iter()
        .filter(move |(t, _, _)| *t == type_name)
        .map(|(t, stem, content)| parse_bundled(t, stem, content))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_resource_parses_to_its_expected_id() {
        for (type_name, stem, content) in BUNDLED {
            let res = parse_bundled(type_name, stem, content);
            assert_eq!(
                res.id, *stem,
                "bundled {type_name}/{stem}.md resolved to unexpected id '{}'",
                res.id
            );
            assert_eq!(res.layer, Layer::Bundled);
        }
    }

    #[test]
    fn bundled_skills_have_a_nonempty_description() {
        // `skills_index` injects `id — description` into the system prompt; an empty description would
        // ship a broken index line.
        for res in bundled_for("skills") {
            let description = res.frontmatter.get("description").unwrap_or_default();
            assert!(
                !description.is_empty(),
                "bundled skill '{}' has no description",
                res.id
            );
        }
    }

    #[test]
    fn bundled_agents_are_read_only() {
        const READ_ONLY: [&str; 3] = ["read_file", "list_dir", "search"];
        for res in bundled_for("agents") {
            let allowed = res.frontmatter.list("allowed-tools").unwrap_or_default();
            assert!(
                !allowed.is_empty(),
                "bundled agent '{}' declares no allowed-tools",
                res.id
            );
            for tool in allowed {
                assert!(
                    READ_ONLY.contains(&tool.as_str()),
                    "bundled agent '{}' allows non-read-only tool '{tool}'",
                    res.id
                );
            }
        }
    }

    #[test]
    fn bundled_for_filters_by_type() {
        let skills: Vec<_> = bundled_for("skills").map(|r| r.id).collect();
        assert!(skills.contains(&"plano".to_string()));
        assert!(!skills.contains(&"search".to_string()));

        let agents: Vec<_> = bundled_for("agents").map(|r| r.id).collect();
        assert!(agents.contains(&"search".to_string()));
        assert!(agents.contains(&"planning".to_string()));

        assert_eq!(bundled_for("rules").count(), 0);
    }
}
