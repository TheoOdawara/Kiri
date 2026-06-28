use std::path::{Path, PathBuf};

use tokio::fs;

/// The temp sibling for an atomic write: `.{file_name}.kiri-tmp` in the same directory as `path`. A
/// sibling (not a temp-dir file) keeps the follow-up rename on the same filesystem and therefore atomic.
/// Prefixing the original file name — rather than `with_extension("…tmp")` — stays correct for names that
/// already carry an extension (`config.toml` -> `.config.toml.kiri-tmp`) and for extensionless `.md`
/// bodies alike. Exposed so a caller that must guard the temp path before it is written (the sync
/// work-tree's symlink refusal) targets the exact path `write_atomic` will write.
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

/// Write `content` to `path` atomically: write the temp sibling, then rename it over `path`. The rename is
/// atomic on a POSIX filesystem, so a crash mid-write leaves either the old bytes or the new ones — never a
/// truncated or half-written file. The single source for this crash-safety idiom: memory's index/body
/// writes and the sync work-tree's config write both route through here. Returns the raw `io::Result` so
/// each caller maps it to its own `AgentError` variant (memory → `Io` via `?`/`#[from]`, sync → `Sync`),
/// keeping this a pure fs utility with no dependency on the kernel error type.
pub async fn write_atomic(path: &Path, content: &[u8]) -> std::io::Result<()> {
    let tmp = temp_sibling(path);
    fs::write(&tmp, content).await?;
    fs::rename(&tmp, path).await?;
    Ok(())
}

/// Synchronous sibling of `write_atomic`, for the config writers that run outside an async context (the
/// live `/effort`/`/models`/`/provider` handlers). Same crash-safety: write the temp sibling, then rename
/// it over `path`, so a crash mid-write can never leave the boot-critical `config.toml` truncated. Returns
/// the raw `io::Result` so the caller maps it to its own `AgentError` variant.
pub(crate) fn write_atomic_sync(path: &Path, content: &[u8]) -> std::io::Result<()> {
    let tmp = temp_sibling(path);
    std::fs::write(&tmp, content)?;
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
