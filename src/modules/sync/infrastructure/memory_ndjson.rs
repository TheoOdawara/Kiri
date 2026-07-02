use std::path::Path;

use tokio::fs;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

use crate::modules::memory::application::shared_memory::SharedMemory;
use crate::modules::memory::domain::entry::MemoryEntry;
use crate::modules::sync::application::memory_exchange::{MemoryExchange, MergeReport};
use crate::modules::sync::domain::merge::incoming_wins;
use crate::shared::kernel::error::{AgentError, AgentResult};

/// Upper bound on entries exported in one pass — a personal cross-project memory stays well under this.
const EXPORT_CAP: usize = 100_000;

/// Upper bound on entries imported in one pass (mirrors `EXPORT_CAP`), so a large or hostile remote
/// `memory.ndjson` cannot drive an unbounded number of per-entry DB round-trips.
const IMPORT_CAP: usize = EXPORT_CAP;

/// Hard byte ceiling on an imported `memory.ndjson`, far above any real personal memory. Untrusted
/// remote content is streamed line-by-line, but a multi-gigabyte file (or one giant unterminated line)
/// must be rejected by a cheap `stat` up front rather than read at all — the byte cap fails fast, the
/// entry cap (`IMPORT_CAP`) bounds the work done within it.
const MAX_IMPORT_BYTES: u64 = 512 * 1024 * 1024;

/// The NDJSON adapter behind the [`MemoryExchange`] port, bound to the shared store it serializes. The
/// composition root injects one into `SyncService`; the free `export`/`import` functions below hold the
/// actual logic and are exercised directly by this module's tests.
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

/// Export the shared memory to deterministic NDJSON (one entry per line, sorted by id) so the synced repo
/// diffs cleanly and merges by line. Embedding vectors are NOT exported — they are machine-local derived
/// data, re-derivable from content on each machine.
pub async fn export(memory: &dyn SharedMemory, path: &Path) -> AgentResult<usize> {
    let mut entries = memory.list(0, EXPORT_CAP).await?;
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    let mut body = String::new();
    for entry in &entries {
        body.push_str(&serde_json::to_string(entry).map_err(AgentError::memory)?);
        body.push('\n');
    }
    // The exported memory is personal/preference content: create it owner-only up front (`0600` on Unix)
    // so there is no transient world-readable window before a chmod, mirroring `secrets/file_store.rs`.
    write_owner_only(path, body.as_bytes()).await?;
    Ok(entries.len())
}

/// Import NDJSON into the shared memory, last-write-wins by `updated_at` per entry id. A missing file is
/// an empty merge (nothing pulled yet). Untrusted remote content (materialized by `git reset --hard` from
/// the profile repo) is hardened three ways: a `symlink_metadata` `stat` that does NOT follow symlinks
/// rejects a symlink or any non-regular file (a committed `-> /dev/zero` or `-> <fifo>`) before it is
/// opened; a byte ceiling (`MAX_IMPORT_BYTES`) rejects an oversized regular file; and the file is then
/// **streamed line-by-line** (never slurped whole) under the entry cap (`IMPORT_CAP`), with the streamed
/// bytes bounded too. A malformed line aborts with a clear error rather than silently dropping knowledge.
pub async fn import(memory: &dyn SharedMemory, path: &Path) -> AgentResult<MergeReport> {
    // `symlink_metadata` does NOT follow symlinks: reject a symlink or any non-regular file before it is
    // opened. The work-tree file is materialized by `git reset --hard` from an attacker-controlled remote,
    // so a committed `memory.ndjson -> /dev/zero` (infinite read → OOM) or `-> <fifo>` (blocking read, no
    // timeout) must never reach `File::open`, which would follow it. `len()` is advisory, so the streaming
    // loop below bounds the bytes actually read as well.
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

    // `len()` is advisory, so bound the bytes actually read with `take`: a hard ceiling that even a single
    // unterminated line cannot allocate past, no matter what the stat reported.
    let file = fs::File::open(path).await?;
    let mut lines = BufReader::new(file.take(MAX_IMPORT_BYTES)).lines();
    let mut report = MergeReport {
        merged: 0,
        skipped: 0,
    };
    // Belt-and-suspenders over the `take` ceiling: surface a clear over-cap error instead of a silent EOF.
    let mut bytes_read: u64 = 0;
    while let Some(line) = lines.next_line().await.map_err(AgentError::memory)? {
        // `next_line` strips the terminator; count it back so the running total tracks the on-disk size.
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

/// Write `bytes` to `path` readable/writable by the owner only. On Unix this is `0600` set at `open` (no
/// post-write chmod window) and re-coerced afterwards so a pre-existing wider mode is tightened; on
/// Windows the file inherits the user-profile DACL (std exposes no ACL control) — the accepted
/// equivalent, but the write is still crash-atomic (temp sibling + rename) via `write_atomic`. Mirrors
/// `provider/infrastructure/secrets/file_store.rs`.
#[cfg(unix)]
async fn write_owner_only(path: &Path, bytes: &[u8]) -> AgentResult<()> {
    use std::os::unix::fs::PermissionsExt;
    use tokio::io::AsyncWriteExt;

    // Refuse to follow a symlink/special file at the target: the sync work-tree is materialized by
    // `git reset --hard` from an untrusted remote, so a committed symlink here would let this write follow
    // it out of the tree (arbitrary truncate/overwrite). Mirrors `import`'s symlink_metadata guard; a
    // missing path is fine (created fresh below).
    match fs::symlink_metadata(path).await {
        Ok(metadata) if !metadata.file_type().is_file() => {
            return Err(AgentError::Memory(format!(
                "refusing to write through a non-regular sync path (symlink or special file): {}",
                path.display()
            )));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(AgentError::Memory(format!(
                "stat {}: {error}",
                path.display()
            )));
        }
    }

    // tokio's `OpenOptions::mode` is an inherent `cfg(unix)` method, so no `OpenOptionsExt` import.
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .await?;
    // `mode()` only applies when the file is created; coerce a pre-existing wider mode (e.g. a 0644 file
    // left by a prior export or materialized 0644 by `git reset --hard`) down to 0600 on every export.
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .await?;
    file.write_all(bytes).await?;
    file.flush().await?;
    Ok(())
}

#[cfg(not(unix))]
async fn write_owner_only(path: &Path, bytes: &[u8]) -> AgentResult<()> {
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
        // Local copy is newer.
        dst.save(&entry("a", "local-new", "2026-06-01T00:00:00Z"))
            .await
            .unwrap();

        // Incoming is older → skipped.
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
        // A file whose length exceeds the byte cap is rejected by the up-front stat, before any line is
        // read and before any DB write. A sparse `set_len` reports the large length without allocating it.
        // The assertion locks the SPECIFIC byte-cap rejection (distinct from a parse error), so reverting
        // the cap — which would let the all-NUL file be read and then fail to serde-parse — fails here.
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
        // A committed symlink at the import path is rejected before any read. The link points at a VALID
        // regular file, so rejection is proven to come from the file type — not the content — closing the
        // `memory.ndjson -> /dev/zero` (infinite read / OOM) and `-> <fifo>` (blocking read) vectors.
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
        // The streamed reader merges every entry and skips blank lines (regression that bounding the
        // read did not break the line loop).
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

        // A pre-existing 0644 export (or one materialized 0644 by `git reset --hard`) must be tightened:
        // `OpenOptions::mode` only applies at create, so re-export must coerce the mode explicitly.
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

        // A hostile remote can materialize the export path as a symlink to an outside file (e.g. ~/.bashrc)
        // via `git reset --hard`; export must refuse to follow it rather than truncate/overwrite the target.
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
}
