use std::path::{Path, PathBuf};

use tokio::fs;

/// A sibling, not a temp-dir file, so the follow-up rename stays on one filesystem and therefore atomic.
/// Prefixing the file name (rather than `with_extension`) keeps an existing extension intact. Exposed so
/// a caller guarding the temp path before it is written targets the exact path `write_atomic` will use.
pub(crate) fn temp_sibling(path: &Path) -> PathBuf {
    match path.file_name().and_then(|name| name.to_str()) {
        Some(name) => path.with_file_name(format!(".{name}.kiri-tmp")),
        // No representable file name (a path ending in `..`/root, or a non-UTF-8 name): suffix the whole
        // path so the temp still lands beside the target and the rename stays on the same filesystem.
        None => {
            let mut raw = path.as_os_str().to_owned();
            raw.push(".kiri-tmp");
            PathBuf::from(raw)
        }
    }
}

/// Rename-over-temp: a crash mid-write leaves the old bytes or the new ones, never a truncated file. The
/// raw `io::Result` lets each caller pick its own `AgentError` variant, keeping this free of the kernel
/// error type.
pub async fn write_atomic(path: &Path, content: &[u8]) -> std::io::Result<()> {
    let tmp = temp_sibling(path);
    fs::write(&tmp, content).await?;
    fs::rename(&tmp, path).await?;
    Ok(())
}

/// [`write_atomic`] for the config writers, which run outside an async context.
pub(crate) fn write_atomic_sync(path: &Path, content: &[u8]) -> std::io::Result<()> {
    let tmp = temp_sibling(path);
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// [`write_atomic_sync`] for secrets. The temp sibling is created `0600` and the rename inherits its mode,
/// so the file is owner-only from birth; plain `write_atomic_sync` would leave it umask-wide (~`0644`).
/// The `sync_all` before rename flushes the bytes, so a crash never leaves a partial credentials file.
#[cfg(unix)]
pub(crate) fn write_atomic_owner_only(path: &Path, content: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let tmp = temp_sibling(path);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    // mode() only applies at create; coerce in case a stale temp from an interrupted run pre-existed wider.
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(content)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn write_atomic_creates_file_with_content() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("index.json");
        write_atomic(&target, b"hello").await.unwrap();
        assert_eq!(fs::read(&target).await.unwrap(), b"hello");
    }

    #[tokio::test]
    async fn write_atomic_overwrites_existing_file() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("index.json");
        write_atomic(&target, b"first").await.unwrap();
        write_atomic(&target, b"second").await.unwrap();
        assert_eq!(fs::read(&target).await.unwrap(), b"second");
    }

    #[tokio::test]
    async fn write_atomic_leaves_no_temp_sibling_on_success() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("index.json");
        write_atomic(&target, b"x").await.unwrap();
        let mut reader = fs::read_dir(dir.path()).await.unwrap();
        while let Some(entry) = reader.next_entry().await.unwrap() {
            assert!(
                !entry.file_name().to_string_lossy().ends_with(".kiri-tmp"),
                "the rename must consume the temp sibling: {:?}",
                entry.file_name()
            );
        }
    }

    #[test]
    fn write_atomic_sync_overwrites_and_leaves_no_temp() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("config.toml");
        write_atomic_sync(&target, b"first").unwrap();
        write_atomic_sync(&target, b"second").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "second");
        assert!(
            !dir.path().join(".config.toml.kiri-tmp").exists(),
            "the sync atomic write must consume the temp sibling"
        );
    }

    #[tokio::test]
    async fn write_atomic_handles_a_name_with_an_existing_extension() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("config.toml");
        // The temp policy prefixes the full file name, so an existing extension survives (unlike
        // `with_extension`, which would clobber `.toml`).
        assert_eq!(
            temp_sibling(&target),
            dir.path().join(".config.toml.kiri-tmp")
        );
        write_atomic(&target, b"k = 1\n").await.unwrap();
        assert_eq!(fs::read_to_string(&target).await.unwrap(), "k = 1\n");
        assert!(!dir.path().join(".config.toml.kiri-tmp").exists());
    }
}
