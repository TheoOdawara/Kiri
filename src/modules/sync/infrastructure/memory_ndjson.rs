use std::path::Path;

use tokio::fs;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

use crate::modules::memory::application::shared_memory::SharedMemory;
use crate::modules::memory::domain::entry::MemoryEntry;
use crate::modules::sync::application::memory_exchange::{MemoryExchange, MergeReport};
use crate::modules::sync::domain::merge::incoming_wins;
use crate::shared::kernel::error::{AgentError, AgentResult};

/// A personal cross-project memory stays well under this.
const EXPORT_CAP: usize = 100_000;

/// Bounds the per-entry DB round-trips a hostile remote `memory.ndjson` can drive.
const IMPORT_CAP: usize = EXPORT_CAP;

/// A multi-gigabyte file (or one giant unterminated line) must be rejected by a cheap `stat` up front
/// rather than read at all; `IMPORT_CAP` then bounds the work done within it.
const MAX_IMPORT_BYTES: u64 = 512 * 1024 * 1024;

pub struct NdjsonMemoryExchange<'a> {
    memory: &'a dyn SharedMemory,
}

impl<'a> NdjsonMemoryExchange<'a> {
    pub fn new(memory: &'a dyn SharedMemory) -> Self {
        Self { memory }
    }
}

#[async_trait::async_trait]
impl MemoryExchange for NdjsonMemoryExchange<'_> {
    async fn export(&self, path: &Path) -> AgentResult<usize> {
        export(self.memory, path).await
    }

    async fn import(&self, path: &Path) -> AgentResult<MergeReport> {
        import(self.memory, path).await
    }
}

/// Sorted by id so the synced repo diffs cleanly and merges by line. Embedding vectors are machine-local
/// derived data, re-derivable from content, so they are not exported.
pub async fn export(memory: &dyn SharedMemory, path: &Path) -> AgentResult<usize> {
    let mut entries = memory.list(0, EXPORT_CAP).await?;
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    let mut body = String::new();
    for entry in &entries {
        body.push_str(&serde_json::to_string(entry).map_err(AgentError::memory)?);
        body.push('\n');
    }
    // Personal content: owner-only at create, so there is no transient world-readable window.
    write_owner_only(path, body.as_bytes()).await?;
    Ok(entries.len())
}

/// Last-write-wins by `updated_at` per entry id. A missing file is an empty merge. A malformed line
/// aborts with a clear error rather than silently dropping knowledge.
pub async fn import(memory: &dyn SharedMemory, path: &Path) -> AgentResult<MergeReport> {
    // The file is materialized by `git reset --hard` from an attacker-controlled remote, so a committed
    // `-> /dev/zero` (OOM) or `-> <fifo>` (blocking read) must never reach `File::open`, which follows
    // links. `symlink_metadata` does not.
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(MergeReport {
                merged: 0,
                skipped: 0,
            });
        }
        Err(error) => {
            return Err(AgentError::Memory(format!(
                "stat {}: {error}",
                path.display()
            )));
        }
    };
    if !metadata.file_type().is_file() {
        return Err(AgentError::Memory(format!(
            "refusing to import a non-regular sync memory file (symlink or special file): {}",
            path.display()
        )));
    }
    if metadata.len() > MAX_IMPORT_BYTES {
        return Err(AgentError::Memory(format!(
            "sync memory file too large ({} bytes > {MAX_IMPORT_BYTES} byte cap): {}",
            metadata.len(),
            path.display()
        )));
    }

    // The stat's `len()` is advisory, so `take` bounds what a single unterminated line can allocate.
    let file = fs::File::open(path).await?;
    let mut lines = BufReader::new(file.take(MAX_IMPORT_BYTES)).lines();
    let mut report = MergeReport {
        merged: 0,
        skipped: 0,
    };
    // Without this, hitting the `take` ceiling would look like a clean EOF instead of an error.
    let mut bytes_read: u64 = 0;
    while let Some(line) = lines.next_line().await.map_err(AgentError::memory)? {
        // `next_line` strips the terminator; count it back to track the on-disk size.
        bytes_read = bytes_read.saturating_add(line.len() as u64 + 1);
        if bytes_read > MAX_IMPORT_BYTES {
            return Err(AgentError::Memory(format!(
                "sync memory file exceeded the {MAX_IMPORT_BYTES} byte cap while streaming: {}",
                path.display()
            )));
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if report.merged + report.skipped >= IMPORT_CAP {
            break;
        }
        let entry: MemoryEntry = serde_json::from_str(line)
            .map_err(|error| AgentError::Memory(format!("invalid memory line: {error}")))?;
        let replace = match memory.load(&entry.id).await? {
            Some(existing) => incoming_wins(&entry.updated_at, &existing.updated_at),
            None => true,
        };
        if replace {
            memory.save(&entry).await?;
            report.merged += 1;
        } else {
            report.skipped += 1;
        }
    }
    Ok(report)
}

/// A symlink committed into the work-tree would let a write follow it out of the tree.
async fn refuse_irregular_target(path: &Path) -> AgentResult<()> {
    match fs::symlink_metadata(path).await {
        Ok(metadata) if !metadata.file_type().is_file() => Err(AgentError::Memory(format!(
            "refusing to write through a non-regular sync path (symlink or special file): {}",
            path.display()
        ))),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(AgentError::Memory(format!(
            "stat {}: {error}",
            path.display()
        ))),
    }
}

/// `0600` at `open`, so there is no post-write chmod window. On Windows the file inherits the
/// user-profile DACL — std exposes no ACL control, and that is the accepted equivalent.
#[cfg(unix)]
async fn write_owner_only(path: &Path, bytes: &[u8]) -> AgentResult<()> {
    use std::os::unix::fs::PermissionsExt;
    use tokio::io::AsyncWriteExt;

    let tmp = crate::shared::infra::fs::temp_sibling(path);
    // A hostile tree could pre-place a symlink at the temp name too, not only at the final target.
    refuse_irregular_target(path).await?;
    refuse_irregular_target(&tmp).await?;

    // tokio's `OpenOptions::mode` is an inherent `cfg(unix)` method, so no `OpenOptionsExt` import.
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .await?;
    // `mode()` only applies at create, so a 0644 temp left by an interrupted export needs tightening.
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .await?;
    file.write_all(bytes).await?;
    file.flush().await?;
    file.sync_all().await?;
    drop(file);
    fs::rename(&tmp, path).await?;
    Ok(())
}

#[cfg(not(unix))]
async fn write_owner_only(path: &Path, bytes: &[u8]) -> AgentResult<()> {
    // A committed symlink is a cross-platform concern, not a Unix-specific one.
    refuse_irregular_target(path).await?;
    refuse_irregular_target(&crate::shared::infra::fs::temp_sibling(path)).await?;
    crate::shared::infra::fs::write_atomic(path, bytes).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::memory::domain::entry::MemoryKind;
    use crate::modules::memory::infrastructure::sqlite_shared_memory::SqliteSharedMemory;
    use tempfile::TempDir;

    async fn memory(dir: &TempDir, name: &str) -> SqliteSharedMemory {
        let db = SqliteSharedMemory::new(dir.path().join(name)).unwrap();
        db.init().await.unwrap();
        db
    }

    fn entry(id_seed: &str, content: &str, updated_at: &str) -> MemoryEntry {
        let mut e = MemoryEntry::new(MemoryKind::Fact, content.into(), Default::default(), None);
        e.id = id_seed.to_string();
        e.updated_at = updated_at.to_string();
        e
    }

    #[tokio::test]
    async fn export_then_import_round_trips() {
        let dir = TempDir::new().unwrap();
        let src = memory(&dir, "src.db").await;
        src.save(&entry("a", "alpha", "2026-01-01T00:00:00Z"))
            .await
            .unwrap();
        src.save(&entry("b", "beta", "2026-01-02T00:00:00Z"))
            .await
            .unwrap();

        let path = dir.path().join("memory.ndjson");
        assert_eq!(export(&src, &path).await.unwrap(), 2);

        let dst = memory(&dir, "dst.db").await;
        let report = import(&dst, &path).await.unwrap();
        assert_eq!(report.merged, 2);
        assert_eq!(dst.count().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn import_keeps_the_newer_entry() {
        let dir = TempDir::new().unwrap();
        let dst = memory(&dir, "dst.db").await;
        dst.save(&entry("a", "local-new", "2026-06-01T00:00:00Z"))
            .await
            .unwrap();

        let path = dir.path().join("incoming.ndjson");
        let src = memory(&dir, "src.db").await;
        src.save(&entry("a", "remote-old", "2026-01-01T00:00:00Z"))
            .await
            .unwrap();
        export(&src, &path).await.unwrap();

        let report = import(&dst, &path).await.unwrap();
        assert_eq!(report.skipped, 1);
        assert_eq!(report.merged, 0);
        assert_eq!(dst.load("a").await.unwrap().unwrap().content, "local-new");
    }

    #[tokio::test]
    async fn import_rejects_oversized_file() {
        // A sparse `set_len` reports the large length without allocating it.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("huge.ndjson");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_IMPORT_BYTES + 1).unwrap();
        drop(file);

        let dst = memory(&dir, "dst.db").await;
        let error = import(&dst, &path).await.unwrap_err();
        let AgentError::Memory(message) = &error else {
            panic!("expected AgentError::Memory, got {error:?}");
        };
        assert!(
            message.contains("too large"),
            "must reject with the byte-cap error, not a parse error: {message}"
        );
        assert_eq!(dst.count().await.unwrap(), 0, "nothing must be merged");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn import_rejects_symlinked_file() {
        // The link points at a VALID file, so the rejection is proven to come from the type, not content.
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("real.ndjson");
        let a = serde_json::to_string(&entry("a", "alpha", "2026-01-01T00:00:00Z")).unwrap();
        std::fs::write(&target, format!("{a}\n")).unwrap();
        let link = dir.path().join("memory.ndjson");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let dst = memory(&dir, "dst.db").await;
        let error = import(&dst, &link).await.unwrap_err();
        let AgentError::Memory(message) = &error else {
            panic!("expected AgentError::Memory, got {error:?}");
        };
        assert!(
            message.contains("non-regular"),
            "a symlink must be rejected before reading: {message}"
        );
        assert_eq!(dst.count().await.unwrap(), 0, "nothing must be merged");
    }

    #[tokio::test]
    async fn import_streams_within_caps() {
        let dir = TempDir::new().unwrap();
        let dst = memory(&dir, "dst.db").await;
        let a = serde_json::to_string(&entry("a", "alpha", "2026-01-01T00:00:00Z")).unwrap();
        let b = serde_json::to_string(&entry("b", "beta", "2026-01-02T00:00:00Z")).unwrap();
        let path = dir.path().join("memory.ndjson");
        std::fs::write(&path, format!("{a}\n\n{b}\n")).unwrap();

        let report = import(&dst, &path).await.unwrap();
        assert_eq!(report.merged, 2);
        assert_eq!(dst.count().await.unwrap(), 2);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exported_ndjson_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let src = memory(&dir, "src.db").await;
        src.save(&entry("a", "alpha", "2026-01-01T00:00:00Z"))
            .await
            .unwrap();
        let path = dir.path().join("memory.ndjson");
        export(&src, &path).await.unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "exported memory must be 0600, got {mode:o}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn export_coerces_preexisting_wider_mode_to_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let src = memory(&dir, "src.db").await;
        src.save(&entry("a", "alpha", "2026-01-01T00:00:00Z"))
            .await
            .unwrap();

        let path = dir.path().join("memory.ndjson");
        std::fs::write(&path, b"stale\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        export(&src, &path).await.unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "re-export must coerce 0644 down to 0600, got {mode:o}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn export_refuses_a_symlinked_target() {
        let dir = TempDir::new().unwrap();
        let src = memory(&dir, "src.db").await;
        src.save(&entry("a", "alpha", "2026-01-01T00:00:00Z"))
            .await
            .unwrap();

        let outside = dir.path().join("victim");
        std::fs::write(&outside, b"do not clobber\n").unwrap();
        let path = dir.path().join("memory.ndjson");
        std::os::unix::fs::symlink(&outside, &path).unwrap();

        let error = export(&src, &path).await.unwrap_err();
        assert!(
            matches!(&error, AgentError::Memory(message) if message.contains("non-regular")),
            "export through a symlink must be refused, got {error:?}"
        );
        assert_eq!(
            std::fs::read(&outside).unwrap(),
            b"do not clobber\n",
            "the symlink target must be untouched"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn export_refuses_a_symlinked_temp_sibling() {
        let dir = TempDir::new().unwrap();
        let src = memory(&dir, "src.db").await;
        src.save(&entry("a", "alpha", "2026-01-01T00:00:00Z"))
            .await
            .unwrap();

        let outside = dir.path().join("victim");
        std::fs::write(&outside, b"do not clobber\n").unwrap();
        let path = dir.path().join("memory.ndjson");
        let tmp = crate::shared::infra::fs::temp_sibling(&path);
        std::os::unix::fs::symlink(&outside, &tmp).unwrap();

        let error = export(&src, &path).await.unwrap_err();
        assert!(
            matches!(&error, AgentError::Memory(message) if message.contains("non-regular")),
            "export through a symlinked temp sibling must be refused, got {error:?}"
        );
        assert_eq!(
            std::fs::read(&outside).unwrap(),
            b"do not clobber\n",
            "the symlink target must be untouched"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn export_is_crash_atomic_and_leaves_no_temp_sibling() {
        let dir = TempDir::new().unwrap();
        let src = memory(&dir, "src.db").await;
        src.save(&entry("a", "alpha", "2026-01-01T00:00:00Z"))
            .await
            .unwrap();
        let path = dir.path().join("memory.ndjson");

        export(&src, &path).await.unwrap();

        assert!(path.exists(), "the export target must exist");
        assert!(
            !crate::shared::infra::fs::temp_sibling(&path).exists(),
            "the rename must consume the temp sibling, leaving none behind"
        );
    }
}
