use std::path::Path;

use tokio::fs;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::modules::memory::application::shared_memory::SharedMemory;
use crate::modules::memory::domain::entry::MemoryEntry;
use crate::modules::sync::domain::merge::incoming_wins;
use crate::shared::kernel::error::AgentError;

type Result<T> = std::result::Result<T, AgentError>;

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

/// What an import merged versus skipped (an older or equal entry already present).
#[derive(Debug)]
pub struct MergeReport {
    pub merged: usize,
    pub skipped: usize,
}

fn ser<E: std::fmt::Display>(error: E) -> AgentError {
    AgentError::Memory(error.to_string())
}

/// Export the shared memory to deterministic NDJSON (one entry per line, sorted by id) so the synced repo
/// diffs cleanly and merges by line. Embedding vectors are NOT exported — they are machine-local derived
/// data, re-derivable from content on each machine.
pub async fn export(memory: &dyn SharedMemory, path: &Path) -> Result<usize> {
    let mut entries = memory.list(0, EXPORT_CAP).await?;
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    let mut body = String::new();
    for entry in &entries {
        body.push_str(&serde_json::to_string(entry).map_err(ser)?);
        body.push('\n');
    }
    // The exported memory is personal/preference content: create it owner-only up front (`0600` on Unix)
    // so there is no transient world-readable window before a chmod, mirroring `secrets/file_store.rs`.
    write_owner_only(path, body.as_bytes()).await?;
    Ok(entries.len())
}

/// Import NDJSON into the shared memory, last-write-wins by `updated_at` per entry id. A missing file is
/// an empty merge (nothing pulled yet). Untrusted remote content is bounded twice: a `stat`-checked byte
/// ceiling (`MAX_IMPORT_BYTES`) rejects an oversized file before any read, and the file is then **streamed
/// line-by-line** (never slurped whole) under the entry cap (`IMPORT_CAP`). A malformed line aborts with a
/// clear error rather than silently dropping knowledge.
pub async fn import(memory: &dyn SharedMemory, path: &Path) -> Result<MergeReport> {
    let metadata = match fs::metadata(path).await {
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
    if metadata.len() > MAX_IMPORT_BYTES {
        return Err(AgentError::Memory(format!(
            "sync memory file too large ({} bytes > {MAX_IMPORT_BYTES} byte cap): {}",
            metadata.len(),
            path.display()
        )));
    }

    let mut lines = BufReader::new(fs::File::open(path).await?).lines();
    let mut report = MergeReport {
        merged: 0,
        skipped: 0,
    };
    while let Some(line) = lines.next_line().await.map_err(ser)? {
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

/// Write `bytes` to `path` readable/writable by the owner only, created at that mode up front. On Unix
/// this is `0600` set at `open` (no post-write chmod window); on Windows the file inherits the
/// user-profile DACL (std exposes no ACL control) — the accepted equivalent. Mirrors
/// `provider/infrastructure/secrets/file_store.rs`.
#[cfg(unix)]
async fn write_owner_only(path: &Path, bytes: &[u8]) -> Result<()> {
    use tokio::io::AsyncWriteExt;

    // tokio's `OpenOptions::mode` is an inherent `cfg(unix)` method, so no `OpenOptionsExt` import.
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .await?;
    file.write_all(bytes).await?;
    file.flush().await?;
    Ok(())
}

#[cfg(not(unix))]
async fn write_owner_only(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::write(path, bytes).await?;
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
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("huge.ndjson");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_IMPORT_BYTES + 1).unwrap();
        drop(file);

        let dst = memory(&dir, "dst.db").await;
        let error = import(&dst, &path).await.unwrap_err();
        assert!(matches!(error, AgentError::Memory(_)), "{error:?}");
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
}
