use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use rusqlite::{Connection, params};
use time::OffsetDateTime;
use tokio::task::spawn_blocking;
use uuid::Uuid;

use crate::modules::session::application::session_store::SessionStore;
use crate::modules::session::domain::session::{Session, SessionSummary};
use crate::modules::session::infrastructure::message_dto::StoredMessage;
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::message::Message;

type Result<T> = std::result::Result<T, AgentError>;

/// Map any non-IO failure (SQLite, serde, join, lock) into the kernel's session error variant.
fn sess<E: std::fmt::Display>(error: E) -> AgentError {
    AgentError::Session(error.to_string())
}

/// RFC3339 timestamp for "now". Formatting a valid UTC instant cannot fail in practice; the empty
/// fallback keeps this runtime path total without an `unwrap` (forbidden outside tests).
fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// Conversation persistence in a single SQLite database (`~/.kiri/sessions.db`). Mirrors
/// `SqliteSharedMemory`: the blocking `rusqlite` connection lives behind `Arc<Mutex<_>>` and every query
/// runs on a blocking thread bounded by `DB_OP_TIMEOUT`, so a slow disk never stalls the TUI runtime.
pub struct SqliteSessionStore {
    conn: Arc<Mutex<Connection>>,
    available: bool,
}

impl SqliteSessionStore {
    /// Open (creating it and its parent directory if needed) the sessions database. Call `init` for the
    /// schema. A store opened this way reports available.
    pub fn new(db_path: PathBuf) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&db_path).map_err(sess)?;
        // ~/.kiri/sessions.db is global across every workspace and terminal, so a second running Kiri
        // can contend for a write. SQLITE_BUSY returns instantly (the op timeout cannot wait it out), so
        // a busy_timeout lets brief cross-process contention be waited out instead of failing. WAL is a
        // best-effort throughput win that also reduces reader/writer contention.
        conn.busy_timeout(DB_OP_TIMEOUT).map_err(sess)?;
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            available: true,
        })
    }

    /// An ephemeral, inert in-memory store used as the degraded fallback when the on-disk database
    /// cannot be opened or initialized — the harness still wires a (unavailable) store instead of
    /// failing to start. Reports `is_available() == false`.
    pub fn in_memory_inert() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(sess)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            available: false,
        })
    }
}

fn lock(conn: &Mutex<Connection>) -> Result<MutexGuard<'_, Connection>> {
    conn.lock()
        .map_err(|error| AgentError::Session(format!("sqlite mutex poisoned: {error}")))
}

/// Upper bound for a single blocking database operation, so a wedged lock or pathological query surfaces
/// as a clear error instead of hanging the runtime.
const DB_OP_TIMEOUT: Duration = Duration::from_secs(5);

/// Run a blocking database closure on the blocking pool, bounded by `DB_OP_TIMEOUT`.
async fn run_blocking<T, F>(op: F) -> Result<T>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    match tokio::time::timeout(DB_OP_TIMEOUT, spawn_blocking(op)).await {
        Ok(joined) => joined.map_err(sess)?,
        Err(_) => Err(AgentError::Session(
            "database operation timed out".to_string(),
        )),
    }
}

#[async_trait]
impl SessionStore for SqliteSessionStore {
    async fn init(&self) -> Result<()> {
        let conn = self.conn.clone();
        run_blocking(move || -> Result<()> {
            let conn = lock(&conn)?;
            // foreign_keys is per-connection; set it so the messages cascade on session delete.
            conn.execute_batch(
                "PRAGMA foreign_keys = ON;
                CREATE TABLE IF NOT EXISTS sessions (
                    id          TEXT PRIMARY KEY,
                    project_id  TEXT NOT NULL,
                    title       TEXT NOT NULL DEFAULT '',
                    created_at  TEXT NOT NULL,
                    updated_at  TEXT NOT NULL
                );
                CREATE TABLE IF NOT EXISTS messages (
                    id           INTEGER PRIMARY KEY AUTOINCREMENT,
                    session_id   TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                    ordinal      INTEGER NOT NULL,
                    role         TEXT NOT NULL,
                    content      TEXT,
                    images       TEXT NOT NULL DEFAULT '[]',
                    tool_calls   TEXT NOT NULL DEFAULT '[]',
                    tool_call_id TEXT
                );
                CREATE INDEX IF NOT EXISTS idx_sessions_project ON sessions(project_id, updated_at DESC);
                -- Drop the prior non-unique index and replace it with a UNIQUE one on the same columns:
                -- it doubles as the lookup index and the belt-and-suspenders that fails an insert if a
                -- cross-process race ever produced a duplicate ordinal. Pre-1.0 caveat: a legacy DB that
                -- already holds duplicate (session_id, ordinal) rows would fail this creation; no migration
                -- is shipped (no released versions to upgrade from).
                DROP INDEX IF EXISTS idx_messages_session;
                CREATE UNIQUE INDEX IF NOT EXISTS idx_messages_session_ordinal
                    ON messages(session_id, ordinal);",
            )
            .map_err(sess)?;
            Ok(())
        })
        .await
    }

    async fn create(&self, project_id: &str) -> Result<Session> {
        let conn = self.conn.clone();
        let project_id = project_id.to_string();
        run_blocking(move || -> Result<Session> {
            let id = Uuid::now_v7().to_string();
            let now = now_rfc3339();
            let conn = lock(&conn)?;
            conn.execute(
                "INSERT INTO sessions (id, project_id, title, created_at, updated_at)
                 VALUES (?1, ?2, '', ?3, ?3)",
                params![id, project_id, now],
            )
            .map_err(sess)?;
            Ok(Session {
                id,
                project_id,
                title: String::new(),
                created_at: now.clone(),
                updated_at: now,
                messages: Vec::new(),
            })
        })
        .await
    }

    async fn append_messages(&self, session_id: &str, messages: &[Message]) -> Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        let conn = self.conn.clone();
        let session_id = session_id.to_string();
        // Serialize off the lock: turn each domain message into its stored columns up front.
        let rows: Vec<(String, Option<String>, String, String, Option<String>)> = messages
            .iter()
            .map(|message| {
                let dto = StoredMessage::from(message);
                let images = serde_json::to_string(&dto.images).map_err(sess)?;
                let tool_calls = serde_json::to_string(&dto.tool_calls).map_err(sess)?;
                Ok((dto.role, dto.content, images, tool_calls, dto.tool_call_id))
            })
            .collect::<Result<_>>()?;
        run_blocking(move || -> Result<()> {
            let now = now_rfc3339();
            let mut guard = lock(&conn)?;
            // IMMEDIATE takes the write lock before the MAX(ordinal) read, so a second process cannot read
            // the same MAX and assign a duplicate ordinal. The RAII transaction rolls back on any `?`
            // early-return (no stranded transaction on the shared connection).
            let tx = guard
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(sess)?;
            let base: i64 = tx
                .query_row(
                    "SELECT COALESCE(MAX(ordinal), -1) + 1 FROM messages WHERE session_id = ?1",
                    params![session_id],
                    |row| row.get(0),
                )
                .map_err(sess)?;
            for (offset, (role, content, images, tool_calls, tool_call_id)) in
                rows.iter().enumerate()
            {
                let ordinal = base + offset as i64;
                tx.execute(
                    "INSERT INTO messages
                        (session_id, ordinal, role, content, images, tool_calls, tool_call_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        session_id,
                        ordinal,
                        role,
                        content,
                        images,
                        tool_calls,
                        tool_call_id
                    ],
                )
                .map_err(sess)?;
            }
            tx.execute(
                "UPDATE sessions SET updated_at = ?2 WHERE id = ?1",
                params![session_id, now],
            )
            .map_err(sess)?;
            tx.commit().map_err(sess)?;
            Ok(())
        })
        .await
    }

    async fn set_title(&self, session_id: &str, title: &str) -> Result<()> {
        let conn = self.conn.clone();
        let session_id = session_id.to_string();
        let title = title.to_string();
        run_blocking(move || -> Result<()> {
            let conn = lock(&conn)?;
            conn.execute(
                "UPDATE sessions SET title = ?2 WHERE id = ?1",
                params![session_id, title],
            )
            .map_err(sess)?;
            Ok(())
        })
        .await
    }

    async fn latest_for_project(&self, project_id: &str) -> Result<Option<SessionSummary>> {
        let mut summaries = self.list_for_project(project_id, 1).await?;
        Ok(summaries.drain(..).next())
    }

    async fn list_for_project(
        &self,
        project_id: &str,
        limit: usize,
    ) -> Result<Vec<SessionSummary>> {
        let conn = self.conn.clone();
        let project_id = project_id.to_string();
        run_blocking(move || -> Result<Vec<SessionSummary>> {
            let conn = lock(&conn)?;
            let mut stmt = conn
                .prepare(
                    "SELECT s.id, s.title, s.updated_at,
                            (SELECT COUNT(*) FROM messages m WHERE m.session_id = s.id)
                     FROM sessions s
                     WHERE s.project_id = ?1
                     ORDER BY s.updated_at DESC
                     LIMIT ?2",
                )
                .map_err(sess)?;
            let rows = stmt
                .query_map(params![project_id, limit as i64], |row| {
                    Ok(SessionSummary {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        updated_at: row.get(2)?,
                        message_count: row.get::<_, i64>(3)? as usize,
                    })
                })
                .map_err(sess)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(sess)?);
            }
            Ok(out)
        })
        .await
    }

    async fn load(&self, session_id: &str) -> Result<Option<Session>> {
        let conn = self.conn.clone();
        let session_id = session_id.to_string();
        run_blocking(move || -> Result<Option<Session>> {
            let conn = lock(&conn)?;
            let header = match conn.query_row(
                "SELECT project_id, title, created_at, updated_at FROM sessions WHERE id = ?1",
                params![session_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            ) {
                Ok(header) => header,
                // Absent session is `Ok(None)`; a real DB error (locked/corrupt/IO) must surface, not be
                // reported to the user as "session not found".
                Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
                Err(error) => return Err(sess(error)),
            };
            let (project_id, title, created_at, updated_at) = header;
            let mut stmt = conn
                .prepare(
                    "SELECT role, content, images, tool_calls, tool_call_id
                     FROM messages WHERE session_id = ?1 ORDER BY ordinal",
                )
                .map_err(sess)?;
            let rows = stmt
                .query_map(params![session_id], |row| {
                    let images_raw: String = row.get(2)?;
                    let tool_calls_raw: String = row.get(3)?;
                    // A corrupt images/tool_calls column makes the row unsafe to keep: silently emptying
                    // tool_calls would leave an assistant message whose calls vanished while its answers
                    // (Role::Tool rows) still reference them — an orphaned exchange the provider rejects.
                    // Skip the whole message instead (returned as None, dropped below).
                    let images = match serde_json::from_str(&images_raw) {
                        Ok(value) => value,
                        Err(_) => return Ok(None),
                    };
                    let tool_calls = match serde_json::from_str(&tool_calls_raw) {
                        Ok(value) => value,
                        Err(_) => return Ok(None),
                    };
                    Ok(Some(StoredMessage {
                        role: row.get(0)?,
                        content: row.get(1)?,
                        images,
                        tool_calls,
                        tool_call_id: row.get(4)?,
                    }))
                })
                .map_err(sess)?;
            let mut messages = Vec::new();
            for row in rows {
                // Skip a corrupt row, and a row with an unrecognized role, rather than abort the load.
                if let Some(message) = row.map_err(sess)?.and_then(StoredMessage::into_domain) {
                    messages.push(message);
                }
            }
            Ok(Some(Session {
                id: session_id,
                project_id,
                title,
                created_at,
                updated_at,
                messages,
            }))
        })
        .await
    }

    async fn delete(&self, session_id: &str) -> Result<bool> {
        let conn = self.conn.clone();
        let session_id = session_id.to_string();
        run_blocking(move || -> Result<bool> {
            let conn = lock(&conn)?;
            // Delete child rows explicitly: ON DELETE CASCADE needs the per-connection PRAGMA, which is
            // only guaranteed on the init connection — this is unconditionally correct.
            conn.execute(
                "DELETE FROM messages WHERE session_id = ?1",
                params![session_id],
            )
            .map_err(sess)?;
            let affected = conn
                .execute("DELETE FROM sessions WHERE id = ?1", params![session_id])
                .map_err(sess)?;
            Ok(affected > 0)
        })
        .await
    }

    fn is_available(&self) -> bool {
        self.available
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn store(dir: &TempDir) -> SqliteSessionStore {
        let db = dir.path().join("sessions.db");
        let store = SqliteSessionStore::new(db).unwrap();
        store.init().await.unwrap();
        store
    }

    #[tokio::test]
    async fn create_append_and_load() {
        let dir = TempDir::new().unwrap();
        let store = store(&dir).await;

        let session = store.create("proj-a").await.unwrap();
        store
            .append_messages(
                &session.id,
                &[Message::user("hello"), Message::assistant_text("hi there")],
            )
            .await
            .unwrap();
        store.set_title(&session.id, "hello").await.unwrap();

        let loaded = store.load(&session.id).await.unwrap().unwrap();
        assert_eq!(loaded.title, "hello");
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.messages[0].content.as_deref(), Some("hello"));
        assert_eq!(loaded.messages[1].content.as_deref(), Some("hi there"));
    }

    #[tokio::test]
    async fn append_preserves_order_across_calls() {
        let dir = TempDir::new().unwrap();
        let store = store(&dir).await;
        let session = store.create("proj-a").await.unwrap();

        store
            .append_messages(&session.id, &[Message::user("first")])
            .await
            .unwrap();
        store
            .append_messages(&session.id, &[Message::assistant_text("second")])
            .await
            .unwrap();
        store
            .append_messages(&session.id, &[Message::user("third")])
            .await
            .unwrap();

        let loaded = store.load(&session.id).await.unwrap().unwrap();
        let contents: Vec<_> = loaded
            .messages
            .iter()
            .map(|m| m.content.clone().unwrap())
            .collect();
        assert_eq!(contents, vec!["first", "second", "third"]);
    }

    #[tokio::test]
    async fn append_assigns_contiguous_ordinals() {
        let dir = TempDir::new().unwrap();
        let store = store(&dir).await;
        let session = store.create("proj-a").await.unwrap();

        store
            .append_messages(
                &session.id,
                &[Message::user("a"), Message::assistant_text("b")],
            )
            .await
            .unwrap();
        store
            .append_messages(&session.id, &[Message::user("c")])
            .await
            .unwrap();

        let guard = lock(&store.conn).unwrap();
        let mut stmt = guard
            .prepare("SELECT ordinal FROM messages WHERE session_id = ?1 ORDER BY ordinal")
            .unwrap();
        let ordinals: Vec<i64> = stmt
            .query_map(params![session.id], |row| row.get(0))
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        assert_eq!(ordinals, vec![0, 1, 2]);
    }

    #[tokio::test]
    async fn duplicate_ordinal_is_rejected() {
        let dir = TempDir::new().unwrap();
        let store = store(&dir).await;
        let session = store.create("proj-a").await.unwrap();

        let guard = lock(&store.conn).unwrap();
        guard
            .execute(
                "INSERT INTO messages (session_id, ordinal, role, content) VALUES (?1, 0, 'user', 'a')",
                params![session.id],
            )
            .unwrap();
        let duplicate = guard.execute(
            "INSERT INTO messages (session_id, ordinal, role, content) VALUES (?1, 0, 'user', 'b')",
            params![session.id],
        );
        assert!(
            duplicate.is_err(),
            "a second row at the same (session_id, ordinal) must violate the unique index"
        );
    }

    #[tokio::test]
    async fn append_error_rolls_back() {
        let dir = TempDir::new().unwrap();
        let store = store(&dir).await;
        let session = store.create("proj-a").await.unwrap();
        store
            .append_messages(
                &session.id,
                &[Message::user("a"), Message::assistant_text("b")],
            )
            .await
            .unwrap();

        // `append_messages` never self-collides (MAX(ordinal)+1 is always free), so the rollback path
        // is forced here by driving the same IMMEDIATE RAII transaction: a valid insert followed by a
        // duplicate-ordinal insert that violates the unique index. Dropping the uncommitted transaction
        // must discard the valid insert too (atomicity) and leave the shared connection usable.
        {
            let mut guard = lock(&store.conn).unwrap();
            let tx = guard
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .unwrap();
            tx.execute(
                "INSERT INTO messages (session_id, ordinal, role, content) VALUES (?1, 2, 'user', 'x')",
                params![session.id],
            )
            .unwrap();
            let duplicate = tx.execute(
                "INSERT INTO messages (session_id, ordinal, role, content) VALUES (?1, 0, 'user', 'y')",
                params![session.id],
            );
            assert!(
                duplicate.is_err(),
                "the colliding insert must fail mid-batch"
            );
            // `tx` drops here without commit -> rollback discards the ordinal-2 insert.
        }

        let loaded = store.load(&session.id).await.unwrap().unwrap();
        assert_eq!(
            loaded.messages.len(),
            2,
            "the rolled-back insert must not persist"
        );

        // The connection must not be stranded in an open transaction: the next append still works.
        store
            .append_messages(&session.id, &[Message::user("c")])
            .await
            .unwrap();
        let loaded = store.load(&session.id).await.unwrap().unwrap();
        assert_eq!(
            loaded.messages.len(),
            3,
            "the connection stays usable after the rollback"
        );
    }

    #[tokio::test]
    async fn list_and_latest_for_project() {
        let dir = TempDir::new().unwrap();
        let store = store(&dir).await;

        let s1 = store.create("proj-a").await.unwrap();
        store
            .append_messages(&s1.id, &[Message::user("a")])
            .await
            .unwrap();
        let s2 = store.create("proj-a").await.unwrap();
        store
            .append_messages(&s2.id, &[Message::user("b")])
            .await
            .unwrap();
        let _other = store.create("proj-b").await.unwrap();

        let list = store.list_for_project("proj-a", 10).await.unwrap();
        assert_eq!(list.len(), 2);
        // Newest first: s2 was updated last.
        assert_eq!(list[0].id, s2.id);
        assert_eq!(list[0].message_count, 1);

        let latest = store.latest_for_project("proj-a").await.unwrap().unwrap();
        assert_eq!(latest.id, s2.id);
    }

    #[tokio::test]
    async fn delete_removes_session_and_messages() {
        let dir = TempDir::new().unwrap();
        let store = store(&dir).await;
        let session = store.create("proj-a").await.unwrap();
        store
            .append_messages(&session.id, &[Message::user("x")])
            .await
            .unwrap();

        assert!(store.delete(&session.id).await.unwrap());
        assert!(store.load(&session.id).await.unwrap().is_none());
        assert!(
            store
                .list_for_project("proj-a", 10)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn persists_across_reopen() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sessions.db");
        let session_id = {
            let store = SqliteSessionStore::new(db.clone()).unwrap();
            store.init().await.unwrap();
            let session = store.create("proj-a").await.unwrap();
            store
                .append_messages(&session.id, &[Message::user("persisted")])
                .await
                .unwrap();
            session.id
        };
        let reopened = SqliteSessionStore::new(db).unwrap();
        reopened.init().await.unwrap();
        let loaded = reopened.load(&session_id).await.unwrap().unwrap();
        assert_eq!(loaded.messages.len(), 1);
    }

    #[tokio::test]
    async fn inert_store_reports_unavailable() {
        let store = SqliteSessionStore::in_memory_inert().unwrap();
        assert!(!store.is_available());
    }
}
