use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::shared::infra::home;
use crate::shared::kernel::sandbox::{NetworkPolicy, NetworkStance, SandboxMode};

/// A positive millisecond count, or `None` for text that is not one. Pure so the parsing is
/// unit-testable, and `None`-on-garbage is what lets the caller tell a typo from an absent value.
fn parse_duration_ms(raw: &str) -> Option<Duration> {
    raw.trim()
        .parse::<u64>()
        .ok()
        .filter(|ms| *ms > 0)
        .map(Duration::from_millis)
}

/// Resolve a timeout: a positive config value wins, else the `KIRI_..._MS` env override, else default.
/// An unusable value — a non-number, or a zero that would mean "no timeout" but silently means "default" —
/// is reported rather than absorbed.
pub(super) fn resolve_timeout(
    config_ms: Option<u64>,
    env_key: &str,
    default: Duration,
    warnings: &mut Vec<String>,
) -> Duration {
    let fallback = |warnings: &mut Vec<String>, source: String| {
        warnings.push(format!(
            "ignoring {source}: expected a positive whole number of milliseconds; using {}ms",
            default.as_millis()
        ));
        default
    };
    if let Some(ms) = config_ms {
        return match ms {
            0 => fallback(warnings, format!("{env_key}'s config value 0")),
            ms => Duration::from_millis(ms),
        };
    }
    let Some(raw) = std::env::var(env_key).ok().filter(|v| !v.trim().is_empty()) else {
        return default;
    };
    match parse_duration_ms(&raw) {
        Some(duration) => duration,
        None => fallback(warnings, format!("{env_key}={raw:?}")),
    }
}

/// Parse a boolean from raw text (`1/true/on/yes` vs `0/false/off/no`, case-insensitive). `None` for
/// anything else, so the caller can report a typo instead of quietly taking the default.
fn parse_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "0" | "false" | "off" | "no" => Some(false),
        "1" | "true" | "on" | "yes" => Some(true),
        _ => None,
    }
}

/// Resolve a boolean: a config value wins (TOML already typed it), else the env override, else `default`.
/// `KIRI_THINKING=maybe` is a typo, not an instruction to keep the default silently.
pub(super) fn resolve_bool(
    config: Option<bool>,
    env_key: &str,
    default: bool,
    warnings: &mut Vec<String>,
) -> bool {
    if let Some(value) = config {
        return value;
    }
    let Some(raw) = std::env::var(env_key).ok().filter(|v| !v.trim().is_empty()) else {
        return default;
    };
    parse_bool(&raw).unwrap_or_else(|| {
        warnings.push(format!(
            "ignoring {env_key}={raw:?}: expected one of 1/true/on/yes or 0/false/off/no; using {default}"
        ));
        default
    })
}

/// `recognized` comes from the kernel enum that owns the tokens (`SandboxMode::RECOGNIZED` /
/// `NetworkStance::RECOGNIZED`), so a typo (`KIRI_SANDBOX=of`) is surfaced rather than silently collapsing
/// to the safe default — no silent no-op on a security knob — and a mode added to the enum cannot end up
/// recognized by the parser but unknown here.
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
        "unrecognized {key}={value:?}; using the safe default {default:?}"
    ))
}

/// Returns `(enabled, require)`. The parse itself lives in the kernel [`SandboxMode`], so the loader and
/// the sync trust gate read it one way; this owns only the config-then-env precedence.
pub(super) fn resolve_sandbox_mode(
    config: Option<&str>,
    warnings: &mut Vec<String>,
) -> (bool, bool) {
    let raw = config
        .map(str::to_string)
        .or_else(|| std::env::var("KIRI_SANDBOX").ok());
    warnings.extend(unrecognized_sandbox_warning(
        "KIRI_SANDBOX",
        raw.as_deref(),
        SandboxMode::RECOGNIZED,
        "os",
    ));
    match SandboxMode::from_config(raw.as_deref()) {
        SandboxMode::Off => (false, false),
        SandboxMode::Os => (true, false),
        SandboxMode::Require => (true, true),
    }
}

/// Maps the kernel [`NetworkStance`] to the tools-layer [`NetworkPolicy`]. `deny` by default.
pub(super) fn resolve_sandbox_network(
    config: Option<&str>,
    warnings: &mut Vec<String>,
) -> NetworkPolicy {
    let raw = config
        .map(str::to_string)
        .or_else(|| std::env::var("KIRI_SANDBOX_NETWORK").ok());
    warnings.extend(unrecognized_sandbox_warning(
        "KIRI_SANDBOX_NETWORK",
        raw.as_deref(),
        NetworkStance::RECOGNIZED,
        "deny",
    ));
    match NetworkStance::from_config(raw.as_deref()) {
        NetworkStance::Allow => NetworkPolicy::Allow,
        NetworkStance::Deny => NetworkPolicy::Deny,
    }
}

/// Matches each platform's own `PATH` convention.
#[cfg(not(windows))]
const PATH_LIST_SEPARATOR: char = ':';
#[cfg(windows)]
const PATH_LIST_SEPARATOR: char = ';';

/// Home resolution lives in [`crate::shared::infra::home`], the single source the agent tool-path tilde
/// expander also reads, so both agree on one home directory.
pub(super) fn expand_home(path: &str) -> PathBuf {
    expand_home_with(path, home::home_dir().as_deref())
}

/// Split out from `expand_home` so the expansion is testable without the env read.
fn expand_home_with(path: &str, home: Option<&Path>) -> PathBuf {
    if path == "~" {
        if let Some(home) = home {
            return home.to_path_buf();
        }
    } else if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = home
    {
        return home.join(rest);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_ms_rejects_garbage_and_zero() {
        // `None` is the signal the caller turns into a warning; a silent fallback here would hide the typo.
        assert_eq!(parse_duration_ms("not-a-number"), None);
        assert_eq!(parse_duration_ms("0"), None);
        assert_eq!(parse_duration_ms("  "), None);
        assert_eq!(parse_duration_ms("-5"), None);
    }

    #[test]
    fn parse_duration_ms_reads_a_positive_value() {
        assert_eq!(
            parse_duration_ms("  2500 "),
            Some(Duration::from_millis(2500))
        );
    }

    #[test]
    fn parse_bool_reads_truthy_and_falsy_and_rejects_the_rest() {
        for truthy in ["1", "true", "on", "yes", " TRUE "] {
            assert_eq!(parse_bool(truthy), Some(true), "{truthy} should be true");
        }
        for falsy in ["0", "false", "off", "no", " Off "] {
            assert_eq!(parse_bool(falsy), Some(false), "{falsy} should be false");
        }
        assert_eq!(
            parse_bool("garbage"),
            None,
            "unknown is reportable, not a default"
        );
        assert_eq!(parse_bool(""), None);
    }

    #[test]
    fn a_zero_config_timeout_is_reported_rather_than_absorbed() {
        // `read_timeout_ms = 0` reads as "no timeout" but resolves to the default. Whichever the user
        // meant, silence is wrong: the config branch is pure, so this never touches the env.
        let mut warnings = no_warnings();
        let resolved = resolve_timeout(
            Some(0),
            "KIRI_UNUSED_TEST_KEY",
            Duration::from_secs(7),
            &mut warnings,
        );
        assert_eq!(resolved, Duration::from_secs(7));
        assert_eq!(warnings.len(), 1, "got: {warnings:?}");
        assert!(warnings[0].contains("positive"), "got: {warnings:?}");
    }

    #[test]
    fn a_config_bool_wins_without_consulting_the_env_or_warning() {
        let mut warnings = no_warnings();
        assert!(resolve_bool(
            Some(true),
            "KIRI_UNUSED_TEST_KEY",
            false,
            &mut warnings
        ));
        assert!(warnings.is_empty());
    }

    /// Discards the warnings channel for the cases that assert only the resolved value.
    fn no_warnings() -> Vec<String> {
        Vec::new()
    }

    #[test]
    fn resolve_sandbox_mode_maps_config_values() {
        // The config branch is pure (a `Some` config short-circuits the env read), so these never touch
        // the process env — safe under edition-2024 parallel tests.
        let mut warnings = no_warnings();
        assert_eq!(
            resolve_sandbox_mode(Some("off"), &mut warnings),
            (false, false)
        );
        assert_eq!(
            resolve_sandbox_mode(Some("os"), &mut warnings),
            (true, false)
        );
        assert_eq!(
            resolve_sandbox_mode(Some("require"), &mut warnings),
            (true, true)
        );
        assert!(warnings.is_empty(), "recognized values warn about nothing");
        // Unknown maps to the os default — never a silent downgrade to off.
        assert_eq!(
            resolve_sandbox_mode(Some("bogus"), &mut warnings),
            (true, false)
        );
    }

    #[test]
    fn resolve_sandbox_network_maps_config_values() {
        let mut warnings = no_warnings();
        assert_eq!(
            resolve_sandbox_network(Some("allow"), &mut warnings),
            NetworkPolicy::Allow
        );
        assert_eq!(
            resolve_sandbox_network(Some("deny"), &mut warnings),
            NetworkPolicy::Deny
        );
        assert!(warnings.is_empty(), "recognized values warn about nothing");
        // Unknown maps to deny — never a silent widening.
        assert_eq!(
            resolve_sandbox_network(Some("bogus"), &mut warnings),
            NetworkPolicy::Deny
        );
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
        // The resolver still falls back to the os default (sandbox stays enabled, not disabled) AND now
        // hands the warning to the caller's channel instead of an `eprintln!` the TUI would swallow.
        let mut warnings = no_warnings();
        assert_eq!(
            resolve_sandbox_mode(Some("of"), &mut warnings),
            (true, false)
        );
        assert_eq!(warnings.len(), 1, "the typo must reach the caller");
        assert!(warnings[0].contains("KIRI_SANDBOX"), "got: {warnings:?}");

        let net = ["allow", "deny"];
        assert!(
            unrecognized_sandbox_warning("KIRI_SANDBOX_NETWORK", Some("alow"), &net, "deny")
                .is_some()
        );
        let mut warnings = no_warnings();
        assert_eq!(
            resolve_sandbox_network(Some("alow"), &mut warnings),
            NetworkPolicy::Deny
        );
        assert_eq!(warnings.len(), 1, "the typo must reach the caller");
    }

    #[test]
    fn resolve_timeout_config_wins() {
        // A positive config value wins and never consults the env (the pure branch).
        assert_eq!(
            resolve_timeout(
                Some(5000),
                "KIRI_UNUSED_TEST_KEY",
                Duration::from_secs(1),
                &mut no_warnings()
            ),
            Duration::from_millis(5000)
        );
    }

    #[test]
    fn expand_home_with_cases() {
        let home = Path::new("/home/alice");
        assert_eq!(
            expand_home_with("~", Some(home)),
            PathBuf::from("/home/alice")
        );
        assert_eq!(
            expand_home_with("~/x/y", Some(home)),
            PathBuf::from("/home/alice/x/y")
        );
        // No home → the tilde is not expanded (taken verbatim).
        assert_eq!(expand_home_with("~", None), PathBuf::from("~"));
        assert_eq!(expand_home_with("~/x", None), PathBuf::from("~/x"));
        // A non-tilde path is unchanged regardless of home.
        assert_eq!(
            expand_home_with("/abs/path", Some(home)),
            PathBuf::from("/abs/path")
        );
    }
}
