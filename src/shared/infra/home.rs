//! Cross-platform home-directory resolution — the single source read by both the config/global-dir
//! resolver (`shared/infra/config/resolve.rs`, for `~/.kiri` and the docs/memory extra-path lists) and
//! the agent tool-path tilde expander (`tools/application/path.rs`). One source keeps the two readers
//! from drifting onto different home directories.

use std::ffi::OsString;
use std::path::PathBuf;

/// Resolve the current user's home directory: `$HOME` first (always set on Unix; also set on Windows
/// under Git Bash and similar POSIX-ish shells), falling back on Windows to `%USERPROFILE%`, then the
/// pre-Vista `%HOMEDRIVE%%HOMEPATH%` pair (still the only one set in some restricted/service contexts).
/// `None` only when none of these are set.
pub fn home_dir() -> Option<PathBuf> {
    resolve_home(|key| std::env::var_os(key))
}

/// Pure fallback chain over an injectable env lookup, so the precedence is unit-testable without
/// mutating the real process environment (env vars are process-global state, unsafe to fight over under
/// edition-2024's parallel test execution).
fn resolve_home(var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if let Some(home) = var("HOME") {
        return Some(PathBuf::from(home));
    }
    if let Some(profile) = var("USERPROFILE") {
        return Some(PathBuf::from(profile));
    }
    let drive = var("HOMEDRIVE")?;
    let path = var("HOMEPATH")?;
    let mut combined = drive;
    combined.push(path);
    Some(PathBuf::from(combined))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lookup<'a>(map: &'a HashMap<&'a str, &'a str>) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |key| map.get(key).map(|v| OsString::from(*v))
    }

    #[test]
    fn prefers_home_when_set() {
        let map = HashMap::from([("HOME", "/home/alice"), ("USERPROFILE", "C:\\Users\\alice")]);
        assert_eq!(
            resolve_home(lookup(&map)),
            Some(PathBuf::from("/home/alice"))
        );
    }

    #[test]
    fn falls_back_to_userprofile_when_home_unset() {
        let map = HashMap::from([("USERPROFILE", "C:\\Users\\alice")]);
        assert_eq!(
            resolve_home(lookup(&map)),
            Some(PathBuf::from("C:\\Users\\alice"))
        );
    }

    #[test]
    fn falls_back_to_homedrive_homepath_when_only_those_are_set() {
        let map = HashMap::from([("HOMEDRIVE", "C:"), ("HOMEPATH", "\\Users\\alice")]);
        assert_eq!(
            resolve_home(lookup(&map)),
            Some(PathBuf::from("C:\\Users\\alice"))
        );
    }

    #[test]
    fn none_when_nothing_is_set() {
        assert_eq!(resolve_home(lookup(&HashMap::new())), None);
    }

    #[test]
    fn homedrive_without_homepath_yields_none() {
        // A partial pair must not silently resolve to just the drive root.
        let map = HashMap::from([("HOMEDRIVE", "C:")]);
        assert_eq!(resolve_home(lookup(&map)), None);
    }
}
