use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rusqlite::{Connection, Row, params};

use crate::modules::memory::application::shared_memory::SharedMemory;
use crate::modules::memory::domain::entry::{MemoryEntry, MemoryKind};
use crate::shared::infra::sqlite::{lock, open_with_parent, run_blocking};
use crate::shared::kernel::error::AgentError;

type Result<T> = std::result::Result<T, AgentError>;

const SELECT_COLUMNS: &str =
    "id, kind, content, tags, project_id, created_at, updated_at FROM entries";

/// Cross-project shared memory persisted in a single SQLite database (`~/.kiri/memory/shared.db`).
/// The blocking `rusqlite` connection lives behind an `Arc<Mutex<_>>` and every query runs on a
/// blocking thread (`spawn_blocking`), so a slow disk never stalls the single-threaded TUI runtime.
pub struct SqliteSharedMemory {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteSharedMemory {
    /// Open (creating it and its parent directory if needed) the shared database. Does not yet create
    /// the schema — call `init` for that.
    pub fn new(db_path: PathBuf) -> Result<Self> {
        let conn = open_with_parent(&db_path, AgentError::memory)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Open an ephemeral in-memory database. Used as an inert fallback when the on-disk store cannot
    /// be opened, so the harness still wires a (unavailable) store instead of failing to start.
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(AgentError::memory)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Persist (or replace) the embedding vector for an entry. Stored as little-endian f32 bytes.
    pub async fn save_embedding(&self, entry_id: &str, model: &str, vector: &[f32]) -> Result<()> {
        let conn = self.conn.clone();
        let entry_id = entry_id.to_string();
        let model = model.to_string();
        let dim = vector.len() as i64;
        let blob = vec_to_blob(vector);
        run_blocking(
            move || -> Result<()> {
                let conn = lock(&conn, AgentError::memory)?;
                conn.execute(
                    "INSERT INTO entry_embeddings (entry_id, model, dim, vector)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(entry_id) DO UPDATE SET model = ?2, dim = ?3, vector = ?4",
                    params![entry_id, model, dim, blob],
                )
                .map_err(AgentError::memory)?;
                Ok(())
            },
            AgentError::memory,
        )
        .await
    }

    /// The most recently updated entries embedded under `model`, paired with their vector. Backs the
    /// semantic recall's candidate set (ranked by cosine in the application layer). Scoping to `model`
    /// keeps cross-model vectors out of the ranking when the active embedder changes.
    pub async fn embedded_candidates(
        &self,
        model: &str,
        limit: usize,
    ) -> Result<Vec<(MemoryEntry, Vec<f32>)>> {
        let conn = self.conn.clone();
        let model = model.to_string();
        run_blocking(
            move || -> Result<Vec<(MemoryEntry, Vec<f32>)>> {
                let conn = lock(&conn, AgentError::memory)?;
                let mut stmt = conn
                    .prepare(
                        "SELECT e.id, e.kind, e.content, e.tags, e.project_id, e.created_at, \
                     e.updated_at, emb.vector \
                     FROM entries e JOIN entry_embeddings emb ON emb.entry_id = e.id \
                     WHERE emb.model = ?1 \
                     ORDER BY e.updated_at DESC LIMIT ?2",
                    )
                    .map_err(AgentError::memory)?;
                let rows = stmt
                    .query_map(params![model, limit as i64], |row| {
                        let entry = row_to_entry(row)?;
                        let blob: Vec<u8> = row.get("vector")?;
                        Ok((entry, blob_to_vec(&blob)))
                    })
                    .map_err(AgentError::memory)?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(AgentError::memory)?);
                }
                Ok(out)
            },
            AgentError::memory,
        )
        .await
    }
}

/// Encode an f32 vector as little-endian bytes for BLOB storage.
fn vec_to_blob(vector: &[f32]) -> Vec<u8> {
    vector.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Decode a little-endian f32 BLOB. A trailing partial chunk (corrupt row) is ignored defensively.
fn blob_to_vec(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Map a SQLite row to a `MemoryEntry`. Unknown kinds fall back to `Fact` and unparseable tags to an
/// empty set — defensive recovery, never a panic, for a database an external tool may have touched.
fn row_to_entry(row: &Row) -> rusqlite::Result<MemoryEntry> {
    let kind: String = row.get("kind")?;
    let tags: String = row.get("tags")?;
    Ok(MemoryEntry {
        id: row.get("id")?,
        kind: MemoryKind::from_str(&kind).unwrap_or(MemoryKind::Fact),
        content: row.get("content")?,
        tags: serde_json::from_str(&tags).unwrap_or_default(),
        project_id: row.get("project_id")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

/// Run a parameterless "list" query on a blocking thread, collecting the rows into entries.
async fn query_entries(
    conn: Arc<Mutex<Connection>>,
    sql: String,
    bind: Vec<Box<dyn rusqlite::ToSql + Send>>,
) -> Result<Vec<MemoryEntry>> {
    run_blocking(
        move || -> Result<Vec<MemoryEntry>> {
            let conn = lock(&conn, AgentError::memory)?;
            let mut stmt = conn.prepare(&sql).map_err(AgentError::memory)?;
            let params = rusqlite::params_from_iter(bind.iter().map(|b| b.as_ref()));
            let rows = stmt
                .query_map(params, row_to_entry)
                .map_err(AgentError::memory)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(AgentError::memory)?);
            }
            Ok(out)
        },
        AgentError::memory,
    )
    .await
}

#[async_trait]
impl SharedMemory for SqliteSharedMemory {
    async fn init(&self) -> Result<()> {
        let conn = self.conn.clone();
        run_blocking(
            move || -> Result<()> {
                let conn = lock(&conn, AgentError::memory)?;
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS entries (
                    id          TEXT PRIMARY KEY,
                    kind        TEXT NOT NULL,
                    content     TEXT NOT NULL,
                    tags        TEXT NOT NULL,
                    project_id  TEXT,
                    created_at  TEXT NOT NULL,
                    updated_at  TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_entries_project ON entries(project_id);
                CREATE INDEX IF NOT EXISTS idx_entries_kind ON entries(kind);
                CREATE TABLE IF NOT EXISTS entry_embeddings (
                    entry_id TEXT PRIMARY KEY,
                    model    TEXT NOT NULL,
                    dim      INTEGER NOT NULL,
                    vector   BLOB NOT NULL
                );",
                )
                .map_err(AgentError::memory)?;
                Ok(())
            },
            AgentError::memory,
        )
        .await
    }

    async fn save(&self, entry: &MemoryEntry) -> Result<()> {
        let conn = self.conn.clone();
        let entry = entry.clone();
        run_blocking(
            move || -> Result<()> {
                let tags = serde_json::to_string(&entry.tags).map_err(AgentError::memory)?;
                let conn = lock(&conn, AgentError::memory)?;
                conn.execute(
                "INSERT INTO entries (id, kind, content, tags, project_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(id) DO UPDATE SET
                    kind = ?2, content = ?3, tags = ?4, project_id = ?5, updated_at = ?7",
                params![
                    entry.id,
                    entry.kind.as_str(),
                    entry.content,
                    tags,
                    entry.project_id,
                    entry.created_at,
                    entry.updated_at,
                ],
            )
            .map_err(AgentError::memory)?;
                Ok(())
            },
            AgentError::memory,
        )
        .await
    }

    async fn load(&self, id: &str) -> Result<Option<MemoryEntry>> {
        let conn = self.conn.clone();
        let id = id.to_string();
        run_blocking(
            move || -> Result<Option<MemoryEntry>> {
                let conn = lock(&conn, AgentError::memory)?;
                let mut stmt = conn
                    .prepare(&format!("SELECT {SELECT_COLUMNS} WHERE id = ?1"))
                    .map_err(AgentError::memory)?;
                let mut rows = stmt
                    .query_map(params![id], row_to_entry)
                    .map_err(AgentError::memory)?;
                match rows.next() {
                    Some(row) => Ok(Some(row.map_err(AgentError::memory)?)),
                    None => Ok(None),
                }
            },
            AgentError::memory,
        )
        .await
    }

    async fn delete(&self, id: &str) -> Result<bool> {
        let conn = self.conn.clone();
        let id = id.to_string();
        run_blocking(
            move || -> Result<bool> {
                let conn = lock(&conn, AgentError::memory)?;
                let affected = conn
                    .execute("DELETE FROM entries WHERE id = ?1", params![id])
                    .map_err(AgentError::memory)?;
                Ok(affected > 0)
            },
            AgentError::memory,
        )
        .await
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>> {
        let like = format!("%{}%", query.to_lowercase());
        query_entries(
            self.conn.clone(),
            format!(
                "SELECT {SELECT_COLUMNS} WHERE lower(content) LIKE ?1 OR lower(tags) LIKE ?1 \
                 OR kind LIKE ?1 ORDER BY updated_at DESC LIMIT ?2"
            ),
            vec![Box::new(like), Box::new(limit as i64)],
        )
        .await
    }

    async fn list(&self, offset: usize, limit: usize) -> Result<Vec<MemoryEntry>> {
        query_entries(
            self.conn.clone(),
            format!("SELECT {SELECT_COLUMNS} ORDER BY updated_at DESC LIMIT ?1 OFFSET ?2"),
            vec![Box::new(limit as i64), Box::new(offset as i64)],
        )
        .await
    }

    async fn list_by_kind(&self, kind: MemoryKind, limit: usize) -> Result<Vec<MemoryEntry>> {
        query_entries(
            self.conn.clone(),
            format!("SELECT {SELECT_COLUMNS} WHERE kind = ?1 ORDER BY updated_at DESC LIMIT ?2"),
            vec![Box::new(kind.as_str().to_string()), Box::new(limit as i64)],
        )
        .await
    }

    async fn list_by_tag(&self, tag: &str, limit: usize) -> Result<Vec<MemoryEntry>> {
        // Tags are stored as a JSON array; match the quoted token to avoid prefix collisions.
        let like = format!("%\"{tag}\"%");
        query_entries(
            self.conn.clone(),
            format!("SELECT {SELECT_COLUMNS} WHERE tags LIKE ?1 ORDER BY updated_at DESC LIMIT ?2"),
            vec![Box::new(like), Box::new(limit as i64)],
        )
        .await
    }

    async fn list_by_project(&self, project_id: &str, limit: usize) -> Result<Vec<MemoryEntry>> {
        query_entries(
            self.conn.clone(),
            format!(
                "SELECT {SELECT_COLUMNS} WHERE project_id = ?1 ORDER BY updated_at DESC LIMIT ?2"
            ),
            vec![Box::new(project_id.to_string()), Box::new(limit as i64)],
        )
        .await
    }

    async fn count(&self) -> Result<usize> {
        let conn = self.conn.clone();
        run_blocking(
            move || -> Result<usize> {
                let conn = lock(&conn, AgentError::memory)?;
                let count: i64 = conn
                    .query_row("SELECT COUNT(*) FROM entries", [], |row| row.get(0))
                    .map_err(AgentError::memory)?;
                Ok(count as usize)
            },
            AgentError::memory,
        )
        .await
    }

    async fn count_by_project(&self, project_id: &str) -> Result<usize> {
        let conn = self.conn.clone();
        let project_id = project_id.to_string();
        run_blocking(
            move || -> Result<usize> {
                let conn = lock(&conn, AgentError::memory)?;
                let count: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM entries WHERE project_id = ?1",
                        params![project_id],
                        |row| row.get(0),
                    )
                    .map_err(AgentError::memory)?;
                Ok(count as usize)
            },
            AgentError::memory,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn memory(dir: &TempDir) -> SqliteSharedMemory {
        let db = dir.path().join("memory").join("shared.db");
        let memory = SqliteSharedMemory::new(db).unwrap();
        memory.init().await.unwrap();
        memory
    }

    fn entry(kind: MemoryKind, content: &str, tags: &[&str], project: Option<&str>) -> MemoryEntry {
        MemoryEntry::new(
            kind,
            content.into(),
            tags.iter().map(|t| t.to_string()).collect(),
            project.map(String::from),
        )
    }

    #[tokio::test]
    async fn save_load_and_delete() {
        let dir = TempDir::new().unwrap();
        let memory = memory(&dir).await;

        let e = entry(
            MemoryKind::Heuristic,
            "fail fast on bad input",
            &["rust"],
            None,
        );
        memory.save(&e).await.unwrap();

        let loaded = memory.load(&e.id).await.unwrap().unwrap();
        assert_eq!(loaded.id, e.id);
        assert_eq!(loaded.kind, MemoryKind::Heuristic);
        assert!(loaded.tags.contains("rust"));

        assert!(memory.delete(&e.id).await.unwrap());
        assert!(memory.load(&e.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn search_and_scopes() {
        let dir = TempDir::new().unwrap();
        let memory = memory(&dir).await;

        memory
            .save(&entry(
                MemoryKind::Pattern,
                "Use newtypes for ids",
                &["rust", "types"],
                Some("proj-a"),
            ))
            .await
            .unwrap();
        memory
            .save(&entry(
                MemoryKind::Fact,
                "python uses None",
                &["python"],
                None,
            ))
            .await
            .unwrap();

        assert_eq!(memory.search("newtypes", 10).await.unwrap().len(), 1);
        assert_eq!(memory.search("rust", 10).await.unwrap().len(), 1);
        assert_eq!(
            memory
                .list_by_kind(MemoryKind::Fact, 10)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(memory.list_by_tag("types", 10).await.unwrap().len(), 1);
        assert_eq!(memory.list_by_project("proj-a", 10).await.unwrap().len(), 1);
        assert_eq!(memory.count().await.unwrap(), 2);
        assert_eq!(memory.count_by_project("proj-a").await.unwrap(), 1);
    }

    #[tokio::test]
    async fn upsert_updates_in_place() {
        let dir = TempDir::new().unwrap();
        let memory = memory(&dir).await;

        let mut e = entry(MemoryKind::Fact, "old", &[], None);
        memory.save(&e).await.unwrap();
        e.update_content("new".into());
        memory.save(&e).await.unwrap();

        assert_eq!(memory.count().await.unwrap(), 1);
        assert_eq!(memory.load(&e.id).await.unwrap().unwrap().content, "new");
    }

    #[tokio::test]
    async fn persists_across_reopen() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("memory").join("shared.db");
        {
            let memory = SqliteSharedMemory::new(db.clone()).unwrap();
            memory.init().await.unwrap();
            memory
                .save(&entry(MemoryKind::Snippet, "boilerplate", &[], None))
                .await
                .unwrap();
        }
        let reopened = SqliteSharedMemory::new(db).unwrap();
        reopened.init().await.unwrap();
        assert_eq!(reopened.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn embedded_candidates_filters_by_model() {
        let dir = TempDir::new().unwrap();
        let memory = memory(&dir).await;

        let a = entry(MemoryKind::Fact, "content a", &[], None);
        let b = entry(MemoryKind::Fact, "content b", &[], None);
        memory.save(&a).await.unwrap();
        memory.save(&b).await.unwrap();
        memory
            .save_embedding(&a.id, "model-a", &[1.0, 0.0])
            .await
            .unwrap();
        memory
            .save_embedding(&b.id, "model-b", &[0.0, 1.0])
            .await
            .unwrap();

        let candidates = memory.embedded_candidates("model-a", 10).await.unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].0.id, a.id);
    }
}
