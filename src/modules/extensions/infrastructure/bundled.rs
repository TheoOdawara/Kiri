//! Default extension resources shipped inside the binary (ADR 0028): rules, skills, and agent profiles
//! that make Kiri useful from the first prompt, with no filesystem setup required. Content lives as plain
//! Markdown files under `bundled/`, compiled in via `include_str!` — the same pattern already used for the
//! config source characterization test (`shared/infra/config.rs`). Parsing reuses `Frontmatter::parse` and
//! the exact id-resolution rule `file_loader::load_one` uses for a disk file, so a bundled `Resource` is
//! indistinguishable in shape from one loaded off disk; every downstream consumer (`skills_index`,
//! `agents_index`, `command_bodies`, the `/rules`/`/skills`/`/agents` displays) treats it uniformly.
//!
//! The `ponytail` rule and the `ponytail`/`ponytail-review`/`ponytail-audit`/`ponytail-debt`/`ponytail-gain`
//! skills are third-party content, MIT-licensed, from <https://github.com/DietrichGebert/ponytail> by
//! DietrichGebert — embedded verbatim (each carries `license`/`source`/`credit` frontmatter). See `NOTICE`
//! at the repo root for the full license text. Kiri has no per-skill argument mechanism (the upstream
//! `argument-hint: [lite|full|ultra]` selects an intensity via a slash-command argument); the skill and
//! rule ship the upstream body verbatim, which already documents all three levels inline, defaulting to
//! `full`.
//! // ponytail: no skill-argument plumbing exists to switch `lite`/`ultra` at runtime; add a
//! // per-invocation argument to `use_skill`/`task` if that granularity is ever needed.

use crate::modules::extensions::domain::frontmatter::Frontmatter;
use crate::modules::extensions::domain::resource::Resource;
use crate::modules::extensions::domain::scope::Layer;

/// One bundled resource: its resource-type subdirectory name (matching `file_loader::RESOURCE_TYPES`),
/// its file stem (the id fallback, same rule as a disk file), and its Markdown content.
const BUNDLED: &[(&str, &str, &str)] = &[
    (
        "rules",
        "ponytail",
        include_str!("bundled/rules/ponytail.md"),
    ),
    ("skills", "plano", include_str!("bundled/skills/plano.md")),
    ("skills", "gh", include_str!("bundled/skills/gh.md")),
    ("skills", "commit", include_str!("bundled/skills/commit.md")),
    (
        "skills",
        "ponytail",
        include_str!("bundled/skills/ponytail.md"),
    ),
    (
        "skills",
        "ponytail-review",
        include_str!("bundled/skills/ponytail-review.md"),
    ),
    (
        "skills",
        "ponytail-audit",
        include_str!("bundled/skills/ponytail-audit.md"),
    ),
    (
        "skills",
        "ponytail-debt",
        include_str!("bundled/skills/ponytail-debt.md"),
    ),
    (
        "skills",
        "ponytail-gain",
        include_str!("bundled/skills/ponytail-gain.md"),
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
    fn bundled_agents_have_a_nonempty_description() {
        // `agents_index` injects `id — description` into the `# Agents` system-prompt block; an empty
        // description would ship a broken index line and leave the agent undiscoverable (ADR 0029).
        for res in bundled_for("agents") {
            let description = res.frontmatter.get("description").unwrap_or_default();
            assert!(
                !description.is_empty(),
                "bundled agent '{}' has no description",
                res.id
            );
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

        let rules: Vec<_> = bundled_for("rules").map(|r| r.id).collect();
        assert_eq!(rules, vec!["ponytail".to_string()]);
    }

    #[test]
    fn ponytail_rule_is_always_on() {
        let rule = bundled_for("rules")
            .find(|r| r.id == "ponytail")
            .expect("bundled ponytail rule must exist");
        assert_eq!(rule.frontmatter.get("always"), Some("true"));
        assert!(rule.body.contains("lazy senior developer"));
    }

    #[test]
    fn ponytail_suite_is_fully_bundled() {
        let skills: Vec<_> = bundled_for("skills").map(|r| r.id).collect();
        for id in [
            "ponytail",
            "ponytail-review",
            "ponytail-audit",
            "ponytail-debt",
            "ponytail-gain",
        ] {
            assert!(skills.contains(&id.to_string()), "missing {id}");
        }
    }

    #[test]
    fn every_ponytail_resource_carries_attribution() {
        let ponytail_ids = [
            ("rules", "ponytail"),
            ("skills", "ponytail"),
            ("skills", "ponytail-review"),
            ("skills", "ponytail-audit"),
            ("skills", "ponytail-debt"),
            ("skills", "ponytail-gain"),
        ];
        for (type_name, id) in ponytail_ids {
            let res = bundled_for(type_name)
                .find(|r| r.id == id)
                .unwrap_or_else(|| panic!("missing bundled {type_name}/{id}"));
            assert_eq!(res.frontmatter.get("license"), Some("MIT"));
            assert_eq!(
                res.frontmatter.get("source"),
                Some("https://github.com/DietrichGebert/ponytail")
            );
            assert_eq!(res.frontmatter.get("credit"), Some("DietrichGebert"));
        }
    }
}
