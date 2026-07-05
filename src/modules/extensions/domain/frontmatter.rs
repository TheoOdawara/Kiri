use std::collections::HashSet;

/// A parsed YAML(ish) frontmatter block — a flat map of `key: value` pairs plus a `lists` map for the
/// hyphenated-array form. Intentionally a tiny hand-rolled parser: the project forbids adding a YAML
/// dependency just for optional metadata headers, and the subset we need (flat scalar keys + simple string
/// lists) is small and well-defined. Pure, so it is unit-testable without touching the filesystem.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Frontmatter {
    scalars: Vec<(String, String)>,
    lists: Vec<(String, Vec<String>)>,
}

impl Frontmatter {
    /// Parse an optional frontmatter block from a Markdown source. A block begins at the very first line
    /// with `---` and ends at the next line that is exactly `---` (or `...`); everything between is parsed.
    /// Returns `Ok(parsed, body)` — the body is the Markdown that remains after the block (or the whole
    /// source when there is none). A malformed block surfaces as an empty `Frontmatter` rather than an
    /// error, so a bad header never aborts boot (it degrades: the resource loads with no metadata).
    pub fn parse(source: &str) -> (Self, &str) {
        let (block, body) = split_block(source);
        let front = Self::parse_block(block);
        (front, body)
    }

    /// The scalar value for `key`, if present. A `--`-duplicated key is the same key.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.scalars
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// The list value for `key`, if present (an empty slice maps to `None` only when the key is absent).
    pub fn list(&self, key: &str) -> Option<&[String]> {
        self.lists
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_slice())
    }

    /// Whether `key` is present as a scalar or a list.
    pub fn has(&self, key: &str) -> bool {
        self.get(key).is_some() || self.lists.iter().any(|(k, _)| k == key)
    }

    /// The parsed scalar keys as a tag set, for callers that index by tag/metadata.
    pub fn scalar_keys(&self) -> HashSet<&str> {
        self.scalars.iter().map(|(k, _)| k.as_str()).collect()
    }

    fn parse_block(block: &str) -> Self {
        let mut scalars = Vec::new();
        let mut lists = Vec::new();
        let mut current_list: Option<(String, Vec<String>)> = None;
        for raw in block.lines() {
            let line = raw.trim();
            if line.is_empty() {
                continue;
            }
            // A list item: a line starting with `- ` while we are mid-list OR a bare `- x` continuation.
            if let Some(rest) = line.strip_prefix("- ") {
                let item = rest.trim().trim_matches(|c: char| c == '"' || c == '\'');
                if !item.is_empty() {
                    if let Some((_, items)) = current_list.as_mut() {
                        items.push(item.to_string());
                    } else {
                        // A list item with no preceding key: tolerate it as a one-entry list under an
                        // empty key so a malformed header still parses without aborting.
                        current_list = Some((String::new(), vec![item.to_string()]));
                    }
                }
                continue;
            }
            // Flush any open list before starting a new scalar key.
            if let Some(list) = current_list.take() {
                lists.push(list);
            }
            if let Some((key, value)) = line.split_once(':') {
                let key = key.trim().to_string();
                let value = value.trim();
                if value.is_empty() {
                    // A key with no value on its line begins a list block.
                    current_list = Some((key, Vec::new()));
                } else {
                    let v = value
                        .trim_matches(|c: char| c == '"' || c == '\'')
                        .to_string();
                    scalars.push((key, v));
                }
            }
        }
        if let Some(list) = current_list {
            lists.push(list);
        }
        Self { scalars, lists }
    }
}

/// Split an optional frontmatter block from a Markdown source, returning `(block, body)`. The block is
/// the text between the opening and closing `---` lines; the body is everything after the closing line.
/// When the source does not start with a `---` fence, the block is empty and the body is the whole source.
fn split_block(source: &str) -> (&str, &str) {
    let first_line_end = source.find('\n').unwrap_or(source.len());
    let first = source[..first_line_end].trim();
    if first != "---" {
        return ("", source);
    }
    // Scan lines after the opening fence, tracking absolute byte offsets, until the line that is
    // exactly `---` (or `...`) closes the block.
    let mut line_start = first_line_end + 1;
    while line_start <= source.len() {
        let rest = &source[line_start..];
        let line_end_rel = rest.find('\n').unwrap_or(rest.len());
        let line = rest[..line_end_rel].trim_end_matches('\r');
        if matches!(line.trim(), "---" | "...") {
            let block = &source[first_line_end + 1..line_start];
            let body_start = (line_start + line_end_rel + 1).min(source.len());
            return (block, &source[body_start..]);
        }
        if line_end_rel == rest.len() {
            break; // ran out of lines with no closing fence
        }
        line_start += line_end_rel + 1;
    }
    // No closing fence: treat the whole post-`---` text as the body with no usable block.
    ("", source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_fence_returns_the_whole_source_as_body() {
        let (front, body) = Frontmatter::parse("# A rule\nbody text");
        assert!(front.get("anything").is_none());
        assert_eq!(body, "# A rule\nbody text");
    }

    #[test]
    fn parses_scalar_keys_and_strips_the_body() {
        let source = "---\nid: rust-style\nalways: true\ntags:\n  - rust\n  - lint\n---\n# Use clippy\nBody.\n";
        let (front, body) = Frontmatter::parse(source);
        assert_eq!(front.get("id"), Some("rust-style"));
        assert_eq!(front.get("always"), Some("true"));
        assert_eq!(
            front.list("tags"),
            Some(&["rust".to_string(), "lint".to_string()][..])
        );
        assert_eq!(body, "# Use clippy\nBody.\n");
    }

    #[test]
    fn quoted_scalar_values_are_unwrapped() {
        let (front, _) = Frontmatter::parse("---\nname: \"my command\"\n---\n");
        assert_eq!(front.get("name"), Some("my command"));
    }

    #[test]
    fn empty_block_is_tolerated() {
        // An opening fence with no closing fence must not panic; the body is the whole source.
        let (front, body) = Frontmatter::parse("---\nid: x\n");
        assert!(front.get("id").is_none());
        assert_eq!(body, "---\nid: x\n");
    }

    #[test]
    fn list_under_a_missing_key_is_tolerated() {
        // A `- item` with no preceding key: tolerated as a one-entry list under an empty key.
        let (front, _) = Frontmatter::parse("---\n- stray\n---\n");
        assert!(front.list("").is_some_and(|l| l == ["stray"]));
    }

    #[test]
    fn dotdotdot_closes_a_block() {
        let (front, body) = Frontmatter::parse("---\nid: yaml\n...\nbody");
        assert_eq!(front.get("id"), Some("yaml"));
        assert_eq!(body, "body");
    }

    #[test]
    fn has_and_scalar_keys_report_presence() {
        let (front, _) = Frontmatter::parse("---\na: 1\nb:\n  - x\n---\n");
        assert!(front.has("a"));
        assert!(front.has("b"));
        assert!(!front.has("z"));
        assert!(front.scalar_keys().contains("a"));
        assert!(!front.scalar_keys().contains("b"));
    }
}
