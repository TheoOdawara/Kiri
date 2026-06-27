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

    /// A matcher that matches nothing — for tests that don't exercise the sensitive guard.
    #[allow(dead_code)]
    pub fn empty() -> Self {
        Self {
            patterns: Arc::from(Vec::<(String, Regex)>::new()),
        }
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

/// Load the sensitive-file globs from `KIRI_SENSITIVE_PATTERNS` (newline-separated, `#` comments,
/// replaces the default) or fall back to the hardcoded default. Compiles each glob into a regex
/// and fails fast on an invalid pattern.
pub fn load_sensitive_matcher() -> Result<SensitiveMatcher> {
    let raw = std::env::var("KIRI_SENSITIVE_PATTERNS").ok();
    let globs: Vec<&str> = match &raw {
        Some(value) if !value.is_empty() => value
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .collect(),
        _ => DEFAULT_SENSITIVE_PATTERNS.to_vec(),
    };
    SensitiveMatcher::new(&globs)
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
}
