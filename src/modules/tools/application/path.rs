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

/// Anchor a drive-relative absolute path (a leading `/`, e.g. `/etc/hosts`) to the drive of `root`, so
/// an out-of-root target resolves on the active workspace's drive rather than the process's current
/// drive — the two diverge once `/cd` relocates the root across drives. A path that already carries a
/// drive (`C:\…`, or a `~` expansion), and every path on Unix, is returned unchanged.
///
/// Not `root.join(expanded)`: `root` is `canonicalize`d, which on Windows is a verbatim path
/// (`\\?\C:\…`) where `/` is a literal character, not a separator — so joining a `/etc/hosts` onto it
/// would yield the broken `\\?\C:/etc/hosts`. We instead read the drive letter and rebuild with `\`.
#[cfg(windows)]
pub(crate) fn anchor_to_root_drive(expanded: &Path, root: &Path) -> PathBuf {
    use std::path::{Component, Prefix};
    if expanded.is_absolute() {
        return expanded.to_path_buf();
    }
    let drive = root.components().find_map(|component| match component {
        Component::Prefix(prefix) => match prefix.kind() {
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => Some(letter),
            _ => None,
        },
        _ => None,
    });
    match drive {
        Some(letter) => {
            let mut anchored = PathBuf::from(format!("{}:\\", letter as char));
            for component in expanded.components() {
                if let Component::Normal(segment) = component {
                    anchored.push(segment);
                }
            }
            anchored
        }
        // ponytail: a UNC-share root has no drive letter — best-effort, the path resolves against the
        // process drive as before. Vanishingly rare; add per-share anchoring only if it ever matters.
        None => expanded.to_path_buf(),
    }
}

/// On every non-Windows platform a leading `/` is already a fully absolute path, so anchoring is a
/// no-op — the input is returned unchanged.
#[cfg(not(windows))]
pub(crate) fn anchor_to_root_drive(expanded: &Path, _root: &Path) -> PathBuf {
    expanded.to_path_buf()
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

    // On Windows `Path::is_absolute` is false for a Unix-style `/x` (it needs a drive prefix), so the
    // classifier must lean on the `starts_with('/')` branch — otherwise a `/x` the model emits would be
    // misrouted into `join_checked` and refused with the schema-contradicting "absolute paths are not
    // allowed" error (issue #40, M3-2). This locks that divergence; on Unix `/x` is already absolute, so
    // the test would pass either way and cannot guard the regression — hence `cfg(windows)`.
    #[cfg(windows)]
    #[test]
    fn leading_slash_is_absolute_even_when_std_disagrees() {
        assert!(
            !Path::new("/etc/hosts").is_absolute(),
            "precondition: Windows std sees a driveless /path as relative"
        );
        assert!(is_absolute_path("/etc/hosts", Path::new("/etc/hosts")));
        assert!(is_absolute_path("C:\\Users\\x", Path::new("C:\\Users\\x")));
        assert!(!is_absolute_path("rel\\x", Path::new("rel\\x")));
    }

    #[cfg(windows)]
    #[test]
    fn anchor_puts_driveless_path_on_root_drive() {
        // A driveless `/etc/hosts` anchors to the root's drive, not the process's current drive.
        assert_eq!(
            anchor_to_root_drive(Path::new("/etc/hosts"), Path::new("D:\\work")),
            PathBuf::from("D:\\etc\\hosts")
        );
        // A path that already carries its own drive is left untouched.
        assert_eq!(
            anchor_to_root_drive(Path::new("C:\\Users\\x"), Path::new("D:\\work")),
            PathBuf::from("C:\\Users\\x")
        );
        // A verbatim root (what `canonicalize` returns) still yields a clean, non-verbatim target — the
        // reason we rebuild by drive letter instead of `root.join(expanded)`.
        assert_eq!(
            anchor_to_root_drive(Path::new("/a/b"), Path::new("\\\\?\\D:\\work")),
            PathBuf::from("D:\\a\\b")
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn anchor_is_identity_off_windows() {
        assert_eq!(
            anchor_to_root_drive(Path::new("/etc/hosts"), Path::new("/home/u")),
            PathBuf::from("/etc/hosts")
        );
    }
}
