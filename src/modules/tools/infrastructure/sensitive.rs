//! The sensitive-file matcher and its env-driven loader. `SensitiveMatcher::new` and
//! `load_sensitive_matcher` are edge-only wiring constructors — invoked solely from the composition
//! root (`app::wire`) — so they keep `anyhow::Result`; the matcher is then injected into `FsSandbox`,
//! whose port methods return the typed `AgentError`.

use std::sync::Arc;

use anyhow::{Result, anyhow};
use regex::Regex;

/// File-name globs that mark a file as potentially sensitive (secrets, keys, credentials).
/// CRUD and move operations on files whose name matches one of these are refused at the sandbox
/// level, before any filesystem touch. Override via `KIRI_SENSITIVE_PATTERNS` (newline-separated,
/// `#` comments, replaces this default). Match is on the last path component only.
const DEFAULT_SENSITIVE_PATTERNS: &[&str] = &[
    ".env",
    ".env.*",
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    "*.pem",
    "*.key",
    "*.crt",
    "*.p12",
    "*.pfx",
    "*.keystore",
    "*.jks",
    "credentials",
    "credentials.*",
    "secrets",
    "secrets.*",
    ".netrc",
    "*.netrc",
    ".npmrc",
    ".pypirc",
    ".pgpass",
    "*.bak",
    "*.swp",
    "*~",
    "service-account*.json",
    "*-credentials.json",
    "application_default_credentials.json",
    "authorized_keys",
    "known_hosts",
];

/// A compiled set of sensitive-file globs. Stored as `(original_glob, compiled_regex)` pairs so
/// error messages can show the human-readable glob, not the regex.
#[derive(Debug, Clone)]
pub struct SensitiveMatcher {
    patterns: Arc<[(String, Regex)]>,
}

impl SensitiveMatcher {
    /// Compile a set of glob patterns into a matcher. Each glob is converted to an anchored regex
    /// (`*` → `.*`, `?` → `.`, other regex metacharacters escaped). Fails fast on an invalid pattern.
    pub fn new(globs: &[&str]) -> Result<Self> {
        let compiled: Result<Vec<(String, Regex)>, regex::Error> = globs
            .iter()
            .map(|g| {
                let regex_str = glob_to_regex(g);
                Regex::new(&regex_str).map(|r| (g.to_string(), r))
            })
            .collect();
        let patterns = compiled.map_err(|e| anyhow!("invalid sensitive pattern: {e}"))?;
        Ok(Self {
            patterns: Arc::from(patterns),
        })
    }

    /// A matcher that matches nothing — for tests that don't exercise the sensitive guard. Every
    /// caller is a `#[cfg(test)]` fixture, so it is gated out of the release binary.
    #[cfg(test)]
    pub fn empty() -> Self {
        Self {
            patterns: Arc::from(Vec::<(String, Regex)>::new()),
        }
    }

    /// The original glob patterns this matcher was built from (the `(glob, regex)` pairs keep them), so
    /// the system prompt can render the *live* sensitive list rather than a hardcoded copy that an
    /// override could make lie (SEC-06).
    pub fn globs(&self) -> Vec<&str> {
        self.patterns
            .iter()
            .map(|(glob, _)| glob.as_str())
            .collect()
    }

    /// Check whether a file name matches any sensitive pattern. Returns the matched glob (for
    /// error messages) or `None`.
    pub fn matches(&self, file_name: &str) -> Option<&str> {
        for (glob, regex) in self.patterns.iter() {
            if regex.is_match(file_name) {
                return Some(glob.as_str());
            }
        }
        None
    }
}

/// Convert a simple glob (`*`, `?`, literal) to an anchored, case-insensitive regex string
/// (`(?i)^…$`). Case-insensitive because the macOS v1 target's filesystem is case-insensitive — a
/// `.ENV` / `ID_RSA` resolves to the same file as `.env` / `id_rsa` and must be guarded the same.
fn glob_to_regex(glob: &str) -> String {
    let mut regex = String::from("(?i)^");
    for ch in glob.chars() {
        match ch {
            '*' => regex.push_str(".*"),
            '?' => regex.push('.'),
            '.' | '\\' | '+' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|' | '/' => {
                regex.push('\\');
                regex.push(ch);
            }
            c => regex.push(c),
        }
    }
    regex.push('$');
    regex
}

/// The active sensitive-file globs: `KIRI_SENSITIVE_PATTERNS` (newline-separated, `#` comments, replaces
/// the default) or the hardcoded default. Owned so the OS sandbox profile can deny the same set the file
/// tools refuse without re-reading the env or borrowing a local — the single source for both layers.
///
/// Fail-closed (#87): a non-empty env value that parses to zero globs (only blanks/comments) must not
/// disable every name guard — fall back to [`DEFAULT_SENSITIVE_PATTERNS`].
pub fn sensitive_globs() -> Vec<String> {
    match std::env::var("KIRI_SENSITIVE_PATTERNS") {
        Ok(value) if !value.is_empty() => {
            let parsed = parse_sensitive_pattern_lines(&value);
            if parsed.is_empty() {
                default_sensitive_globs()
            } else {
                parsed
            }
        }
        _ => default_sensitive_globs(),
    }
}

fn default_sensitive_globs() -> Vec<String> {
    DEFAULT_SENSITIVE_PATTERNS
        .iter()
        .map(|g| g.to_string())
        .collect()
}

/// Parse newline-separated globs: trim, drop blanks and `#` comments.
fn parse_sensitive_pattern_lines(value: &str) -> Vec<String> {
    value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// Load the sensitive-file globs (via [`sensitive_globs`]) and compile them into a matcher, failing fast
/// on an invalid pattern.
pub fn load_sensitive_matcher() -> Result<SensitiveMatcher> {
    let globs = sensitive_globs();
    let refs: Vec<&str> = globs.iter().map(String::as_str).collect();
    SensitiveMatcher::new(&refs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_env_files() {
        let m = SensitiveMatcher::new(DEFAULT_SENSITIVE_PATTERNS).unwrap();
        assert_eq!(m.matches(".env"), Some(".env"));
        assert_eq!(m.matches(".env.local"), Some(".env.*"));
        assert_eq!(m.matches(".env.production"), Some(".env.*"));
    }

    #[test]
    fn matches_ssh_keys() {
        let m = SensitiveMatcher::new(DEFAULT_SENSITIVE_PATTERNS).unwrap();
        assert_eq!(m.matches("id_rsa"), Some("id_rsa"));
        assert_eq!(m.matches("id_ed25519"), Some("id_ed25519"));
    }

    #[test]
    fn matches_cert_extensions() {
        let m = SensitiveMatcher::new(DEFAULT_SENSITIVE_PATTERNS).unwrap();
        assert_eq!(m.matches("server.pem"), Some("*.pem"));
        assert_eq!(m.matches("ca.crt"), Some("*.crt"));
        assert_eq!(m.matches("keystore.jks"), Some("*.jks"));
    }

    #[test]
    fn matches_backup_files() {
        let m = SensitiveMatcher::new(DEFAULT_SENSITIVE_PATTERNS).unwrap();
        assert_eq!(m.matches("config.bak"), Some("*.bak"));
        assert_eq!(m.matches("notes.txt~"), Some("*~"));
        assert_eq!(m.matches(".vim.swp"), Some("*.swp"));
    }

    #[test]
    fn matches_cloud_credentials() {
        let m = SensitiveMatcher::new(DEFAULT_SENSITIVE_PATTERNS).unwrap();
        assert_eq!(
            m.matches("service-account-prod.json"),
            Some("service-account*.json")
        );
        assert_eq!(
            m.matches("aws-credentials.json"),
            Some("*-credentials.json")
        );
        assert_eq!(
            m.matches("application_default_credentials.json"),
            Some("application_default_credentials.json")
        );
    }

    #[test]
    fn matches_are_case_insensitive() {
        // macOS APFS is case-insensitive: an uppercase variant resolves to the same file and must be
        // guarded the same, so a write to `.ENV` cannot slip past the guard and clobber `.env`.
        let m = SensitiveMatcher::new(DEFAULT_SENSITIVE_PATTERNS).unwrap();
        assert_eq!(m.matches(".ENV"), Some(".env"));
        assert_eq!(m.matches("ID_RSA"), Some("id_rsa"));
        assert_eq!(m.matches("Server.PEM"), Some("*.pem"));
    }

    #[test]
    fn parse_drops_blanks_and_comments() {
        let parsed = parse_sensitive_pattern_lines("\n# only comments\n  \n# .env\n");
        assert!(parsed.is_empty());
    }

    #[test]
    fn parse_keeps_real_globs() {
        let parsed = parse_sensitive_pattern_lines("# header\n.mysecret\n\n*.token\n");
        assert_eq!(parsed, vec![".mysecret".to_string(), "*.token".to_string()]);
    }

    /// #87: env value that is non-empty but only comments/blanks must not yield an empty matcher.
    #[test]
    fn empty_after_parse_falls_back_to_defaults_via_sensitive_globs_logic() {
        let value = "\n# only comments\n  \n";
        let parsed = parse_sensitive_pattern_lines(value);
        assert!(parsed.is_empty());
        // Mirror sensitive_globs' fail-closed arm without mutating process env.
        let globs = if parsed.is_empty() {
            default_sensitive_globs()
        } else {
            parsed
        };
        let refs: Vec<&str> = globs.iter().map(String::as_str).collect();
        let m = SensitiveMatcher::new(&refs).unwrap();
        assert_eq!(m.matches(".env"), Some(".env"));
        assert!(!m.globs().is_empty());
    }

    #[test]
    fn does_not_match_normal_files() {
        let m = SensitiveMatcher::new(DEFAULT_SENSITIVE_PATTERNS).unwrap();
        assert_eq!(m.matches("main.rs"), None);
        assert_eq!(m.matches("README.md"), None);
        assert_eq!(m.matches("package.json"), None);
        assert_eq!(m.matches("src"), None);
    }

    #[test]
    fn empty_matcher_matches_nothing() {
        let m = SensitiveMatcher::empty();
        assert_eq!(m.matches(".env"), None);
        assert_eq!(m.matches("id_rsa"), None);
    }

    #[test]
    fn globs_returns_the_original_patterns() {
        let m = SensitiveMatcher::new(&[".env", "id_rsa", "*.pem"]).unwrap();
        assert_eq!(m.globs(), vec![".env", "id_rsa", "*.pem"]);
    }
}
