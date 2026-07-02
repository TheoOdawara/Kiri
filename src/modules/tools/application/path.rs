use std::path::{Path, PathBuf};

/// Expand a leading `~` (alone) or `~/…` to `home`; any other path is returned unchanged. `~user` is
/// intentionally not expanded. Pure, for testability.
pub(crate) fn expand_tilde(path: &str, home: Option<&Path>) -> PathBuf {
    if let Some(home) = home {
        if path == "~" {
            return home.to_path_buf();
        }
        if let Some(rest) = path.strip_prefix("~/") {
            return home.join(rest);
        }
    }
    PathBuf::from(path)
}

pub(crate) fn home() -> Option<PathBuf> {
    crate::shared::infra::home::home_dir()
}

/// Whether `raw` (as written, before expansion) denotes an absolute location, given its tilde-expanded
/// form. A leading `/` is always absolute, even on Windows, where `Path::is_absolute` alone requires a
/// drive prefix and would otherwise misclassify a Unix-style path as relative — the model (and the
/// sandbox's own path-resolution methods) emit/accept such paths regardless of host OS. `expanded` still
/// covers a native `C:\…` a Windows user might type, and a tilde expansion (`home()` already resolves to
/// a platform-native absolute path).
pub(crate) fn is_absolute_path(raw: &str, expanded: &Path) -> bool {
    raw.starts_with('/') || expanded.is_absolute()
}

/// Whether a tool path targets an explicit absolute location (after `~` expansion) — i.e. potentially
/// outside the active workspace. Used to pick the confirmation default (accept inside, decline outside).
pub(crate) fn is_absolute_target(path: &str) -> bool {
    is_absolute_path(path, &expand_tilde(path, home().as_deref()))
}

/// The confirmation default for a tool path: accept inside the workspace, decline for an explicit
/// absolute/`~` target (potentially outside it). The single source of the in/out-of-workspace rule.
pub(crate) fn default_accept_for(path: &str) -> bool {
    !is_absolute_target(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_tilde_expands_leading_home() {
        let home = Path::new("/home/u");
        assert_eq!(expand_tilde("~", Some(home)), PathBuf::from("/home/u"));
        assert_eq!(
            expand_tilde("~/dev/x", Some(home)),
            PathBuf::from("/home/u/dev/x")
        );
        assert_eq!(expand_tilde("dev/x", Some(home)), PathBuf::from("dev/x"));
        assert_eq!(expand_tilde("/abs", Some(home)), PathBuf::from("/abs"));
        assert_eq!(expand_tilde("~/x", None), PathBuf::from("~/x"));
    }
}
