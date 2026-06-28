use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use regex::Regex;

use crate::shared::kernel::sandbox::{NetworkPolicy, NetworkStance, SandboxMode};

use super::defaults::DEFAULT_NET_ALLOW;

/// Parse a millisecond duration from raw text, falling back to `default` when absent, unparseable, or
/// zero. Pure so the parsing is unit-testable.
fn parse_duration_ms(raw: Option<&str>, default: Duration) -> Duration {
    match raw.and_then(|v| v.trim().parse::<u64>().ok()) {
        Some(ms) if ms > 0 => Duration::from_millis(ms),
        _ => default,
    }
}

/// Resolve a timeout: a positive config value wins, else the `KIRI_..._MS` env override, else default.
pub(super) fn resolve_timeout(
    config_ms: Option<u64>,
    env_key: &str,
    default: Duration,
) -> Duration {
    if let Some(ms) = config_ms.filter(|ms| *ms > 0) {
        return Duration::from_millis(ms);
    }
    parse_duration_ms(std::env::var(env_key).ok().as_deref(), default)
}

/// Parse a boolean from raw text (`1/true/on/yes` vs `0/false/off/no`, case-insensitive), falling back
/// to `default`. Pure so the parsing is unit-testable.
fn parse_bool(raw: Option<&str>, default: bool) -> bool {
    match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("0" | "false" | "off" | "no") => false,
        Some("1" | "true" | "on" | "yes") => true,
        _ => default,
    }
}

/// Resolve a boolean: a config value wins, else the env override, else `default`.
pub(super) fn resolve_bool(config: Option<bool>, env_key: &str, default: bool) -> bool {
    config.unwrap_or_else(|| parse_bool(std::env::var(env_key).ok().as_deref(), default))
}

/// The warning for a present-but-unrecognized sandbox value, else `None` (recognized token, empty, or
/// absent). Pure so it is unit-testable; the resolvers print the returned message to stderr. `recognized`
/// mirrors the matching `from_config`'s exact token set, so a typo (e.g. `KIRI_SANDBOX=of`) is surfaced
/// rather than silently collapsing to the safe default — a no-silent-no-op on a security-relevant knob.
fn unrecognized_sandbox_warning(
    key: &str,
    raw: Option<&str>,
    recognized: &[&str],
    default: &str,
) -> Option<String> {
    let value = raw.filter(|v| !v.is_empty())?;
    if recognized.contains(&value) {
        return None;
    }
    Some(format!(
        "kiri: warning: unrecognized {key}={value:?}; using the safe default {default:?}"
    ))
}

/// `KIRI_SANDBOX` / `[sandbox].mode`: `os` (default) uses the platform adapter where available; `off`
/// disables OS confinement; `require` refuses `run_command` when no OS sandbox is available. Returns
/// `(enabled, require)`. The parse lives in the kernel [`SandboxMode`] so the loader and the sync trust
/// gate read it one way; this resolver only owns the config-then-env precedence and the runtime mapping.
pub(super) fn resolve_sandbox_mode(config: Option<&str>) -> (bool, bool) {
    let raw = config
        .map(str::to_string)
        .or_else(|| std::env::var("KIRI_SANDBOX").ok());
    if let Some(warning) = unrecognized_sandbox_warning(
        "KIRI_SANDBOX",
        raw.as_deref(),
        &["os", "off", "require"],
        "os",
    ) {
        eprintln!("{warning}");
    }
    match SandboxMode::from_config(raw.as_deref()) {
        SandboxMode::Off => (false, false),
        SandboxMode::Os => (true, false),
        SandboxMode::Require => (true, true),
    }
}

/// `KIRI_SANDBOX_NETWORK` / `[sandbox].network`: the base network stance for `run_command`. `deny`
/// default. The parse lives in the kernel [`NetworkStance`]; this resolver maps it to the tools-layer
/// [`NetworkPolicy`] runtime enum.
pub(super) fn resolve_sandbox_network(config: Option<&str>) -> NetworkPolicy {
    let raw = config
        .map(str::to_string)
        .or_else(|| std::env::var("KIRI_SANDBOX_NETWORK").ok());
    if let Some(warning) = unrecognized_sandbox_warning(
        "KIRI_SANDBOX_NETWORK",
        raw.as_deref(),
        &["allow", "deny"],
        "deny",
    ) {
        eprintln!("{warning}");
    }
    match NetworkStance::from_config(raw.as_deref()) {
        NetworkStance::Allow => NetworkPolicy::Allow,
        NetworkStance::Deny => NetworkPolicy::Deny,
    }
}

/// Load the network allow-list from `KIRI_SANDBOX_NET_ALLOW_CMDS` (newline-separated regexes, `#`
/// comments, replaces the default) or the hardcoded dev-command default. Fails fast on a bad pattern.
pub(super) fn load_net_allow() -> Result<Arc<[Regex]>> {
    compile_patterns("KIRI_SANDBOX_NET_ALLOW_CMDS", DEFAULT_NET_ALLOW)
}

/// The single read of the platform's home directory — the one extension point for home resolution.
/// Unix-only today (`$HOME`); macOS is the v1 target. Windows: fall back to `%USERPROFILE%` here and
/// nowhere else when Windows support lands (deferred — do not add untested Windows code now).
fn home_dir() -> Option<OsString> {
    std::env::var_os("HOME")
}

/// The separator between entries in a colon-list env var (the extra docs/memory paths). Unix-only today
/// (`:`); Windows uses `;` — change it here, the single site, when Windows support lands (deferred).
const PATH_LIST_SEPARATOR: char = ':';

/// Expand a leading `~`/`~/…` to the home dir; any other path is taken as given.
pub(super) fn expand_home(path: &str) -> PathBuf {
    expand_home_with(path, home_dir().as_ref())
}

/// Pure tilde expansion against an explicit `home`: `~` and `~/…` expand when `home` is `Some`, else
/// (and for any non-tilde path) the input is taken verbatim. The env read lives in `expand_home`.
fn expand_home_with(path: &str, home: Option<&OsString>) -> PathBuf {
    if path == "~" {
        if let Some(home) = home {
            return PathBuf::from(home);
        }
    } else if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = home
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(path)
}

/// Parse a `PATH_LIST_SEPARATOR`-separated path list from `env`, tilde-expanded, prefixed with `defaults`.
pub(super) fn load_extra_paths(env: &str, defaults: &[&str]) -> Arc<[PathBuf]> {
    let mut paths: Vec<PathBuf> = defaults.iter().map(|p| expand_home(p)).collect();
    if let Some(value) = std::env::var(env).ok().filter(|v| !v.is_empty()) {
        paths.extend(
            value
                .split(PATH_LIST_SEPARATOR)
                .filter(|s| !s.is_empty())
                .map(expand_home),
        );
    }
    Arc::from(paths)
}

/// The non-blank, non-comment lines of a newline-separated override, trimmed.
fn usable_pattern_lines(value: &str) -> Vec<&str> {
    value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
}

/// Select the effective pattern list from a raw override value: a non-empty override's usable lines,
/// else the `defaults`. Pure (the env read lives in `compile_patterns`) so it is unit-testable. A
/// non-empty override that filters to zero usable lines (e.g. only comments) falls back to `defaults`
/// rather than silently disabling a safety list (a blacklist that would then block nothing).
fn select_patterns<'a>(raw: Option<&'a str>, defaults: &[&'a str]) -> Vec<&'a str> {
    match raw {
        Some(value) if !value.is_empty() => {
            let filtered = usable_pattern_lines(value);
            if filtered.is_empty() {
                defaults.to_vec()
            } else {
                filtered
            }
        }
        _ => defaults.to_vec(),
    }
}

/// Compile a newline-separated regex list from `env` (with `#` comments) or the given default, failing
/// fast on an invalid pattern.
pub(super) fn compile_patterns(env: &str, defaults: &[&str]) -> Result<Arc<[Regex]>> {
    let raw = std::env::var(env).ok();
    let patterns = select_patterns(raw.as_deref(), defaults);
    // Warn here, where the env name is known, when a present override emptied to nothing and we fell
    // back to defaults — so a user who tried to override a safety list is not silently ignored.
    if raw
        .as_deref()
        .is_some_and(|value| !value.is_empty() && usable_pattern_lines(value).is_empty())
    {
        eprintln!(
            "kiri: {env} has no usable patterns after stripping blank/comment lines; using defaults"
        );
    }
    let regexes: Result<Vec<Regex>, regex::Error> =
        patterns.iter().map(|p| Regex::new(p)).collect();
    let regexes = regexes.map_err(|e| anyhow!("invalid regex in {env}: {e}"))?;
    Ok(Arc::from(regexes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_ms_uses_default_when_absent_invalid_or_zero() {
        let default = Duration::from_secs(15);
        assert_eq!(parse_duration_ms(None, default), default);
        assert_eq!(parse_duration_ms(Some("not-a-number"), default), default);
        assert_eq!(parse_duration_ms(Some("0"), default), default);
        assert_eq!(parse_duration_ms(Some("  "), default), default);
    }

    #[test]
    fn parse_duration_ms_reads_a_positive_value() {
        assert_eq!(
            parse_duration_ms(Some("  2500 "), Duration::from_secs(15)),
            Duration::from_millis(2500)
        );
    }

    #[test]
    fn parse_bool_reads_truthy_and_falsy_and_falls_back() {
        for truthy in ["1", "true", "on", "yes", " TRUE "] {
            assert!(parse_bool(Some(truthy), false), "{truthy} should be true");
        }
        for falsy in ["0", "false", "off", "no", " Off "] {
            assert!(!parse_bool(Some(falsy), true), "{falsy} should be false");
        }
        assert!(parse_bool(None, true), "absent falls back to default");
        assert!(!parse_bool(Some("garbage"), false), "unknown falls back");
    }

    #[test]
    fn resolve_sandbox_mode_maps_config_values() {
        // The config branch is pure (a `Some` config short-circuits the env read), so these never touch
        // the process env — safe under edition-2024 parallel tests.
        assert_eq!(resolve_sandbox_mode(Some("off")), (false, false));
        assert_eq!(resolve_sandbox_mode(Some("os")), (true, false));
        assert_eq!(resolve_sandbox_mode(Some("require")), (true, true));
        // Unknown maps to the os default — never a silent downgrade to off.
        assert_eq!(resolve_sandbox_mode(Some("bogus")), (true, false));
    }

    #[test]
    fn resolve_sandbox_network_maps_config_values() {
        assert_eq!(resolve_sandbox_network(Some("allow")), NetworkPolicy::Allow);
        assert_eq!(resolve_sandbox_network(Some("deny")), NetworkPolicy::Deny);
        // Unknown maps to deny — never a silent widening.
        assert_eq!(resolve_sandbox_network(Some("bogus")), NetworkPolicy::Deny);
    }

    #[test]
    fn unrecognized_sandbox_env_warns_and_defaults_secure() {
        // SHARED-12: a present-but-unrecognized value yields a warning and still resolves to the safe
        // default — no silent no-op on a security-relevant knob. Recognized tokens, empty, and absent
        // produce no warning.
        let mode = ["os", "off", "require"];
        assert!(
            unrecognized_sandbox_warning("KIRI_SANDBOX", Some("of"), &mode, "os").is_some(),
            "a typo must warn"
        );
        assert!(unrecognized_sandbox_warning("KIRI_SANDBOX", Some("off"), &mode, "os").is_none());
        assert!(unrecognized_sandbox_warning("KIRI_SANDBOX", Some(""), &mode, "os").is_none());
        assert!(unrecognized_sandbox_warning("KIRI_SANDBOX", None, &mode, "os").is_none());
        // The resolver still falls back to the os default (sandbox stays enabled, not disabled).
        assert_eq!(resolve_sandbox_mode(Some("of")), (true, false));

        let net = ["allow", "deny"];
        assert!(
            unrecognized_sandbox_warning("KIRI_SANDBOX_NETWORK", Some("alow"), &net, "deny")
                .is_some()
        );
        assert_eq!(resolve_sandbox_network(Some("alow")), NetworkPolicy::Deny);
    }

    #[test]
    fn select_patterns_falls_back_when_override_empties() {
        let defaults = ["alpha", "beta"];
        // An override of only blank/comment lines falls back to defaults (never a silently empty list).
        assert_eq!(
            select_patterns(Some("# x\n   \n# y\n"), &defaults),
            vec!["alpha", "beta"]
        );
        // A real override is used verbatim (trimmed, comments stripped).
        assert_eq!(
            select_patterns(Some("foo\n# c\nbar\n"), &defaults),
            vec!["foo", "bar"]
        );
        // Absent or empty → defaults.
        assert_eq!(select_patterns(None, &defaults), vec!["alpha", "beta"]);
        assert_eq!(select_patterns(Some(""), &defaults), vec!["alpha", "beta"]);
    }

    #[test]
    fn resolve_timeout_config_wins() {
        // A positive config value wins and never consults the env (the pure branch).
        assert_eq!(
            resolve_timeout(Some(5000), "KIRI_UNUSED_TEST_KEY", Duration::from_secs(1)),
            Duration::from_millis(5000)
        );
    }

    #[test]
    fn expand_home_with_cases() {
        let home = OsString::from("/home/alice");
        assert_eq!(
            expand_home_with("~", Some(&home)),
            PathBuf::from("/home/alice")
        );
        assert_eq!(
            expand_home_with("~/x/y", Some(&home)),
            PathBuf::from("/home/alice/x/y")
        );
        // No home → the tilde is not expanded (taken verbatim).
        assert_eq!(expand_home_with("~", None), PathBuf::from("~"));
        assert_eq!(expand_home_with("~/x", None), PathBuf::from("~/x"));
        // A non-tilde path is unchanged regardless of home.
        assert_eq!(
            expand_home_with("/abs/path", Some(&home)),
            PathBuf::from("/abs/path")
        );
    }
}
