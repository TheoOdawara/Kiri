use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rusqlite::{Connection, params};
use uuid::Uuid;

use crate::modules::session::application::session_store::SessionStore;
use crate::modules::session::domain::session::{Session, SessionSummary};
use crate::modules::session::infrastructure::message_dto::StoredMessage;
use crate::shared::infra::sqlite::{lock, open_with_parent, run_blocking};
use crate::shared::kernel::error::{AgentError, AgentResult};
use crate::shared::kernel::message::Message;
use crate::shared::kernel::time::now_rfc3339;

/// `busy_timeout` for cross-process write contention on the global sessions DB. Kept STRICTLY BELOW
/// `DB_OP_TIMEOUT` so a persistent lock surfaces as a deterministic `SQLITE_BUSY` error *before* the
/// op-level `tokio::time` timeout fires. If they were equal, the timeout could win the race and cancel
/// the awaiting future while the detached blocking commit is still in flight: `flush_session` would
/// return early without advancing `persisted_len`, and the next flush would re-append the same delta
/// with fresh ordinals, duplicating messages on resume (BUG-01).
const SESSION_BUSY_TIMEOUT: Duration = Duration::from_secs(3);

/// Conversation persistence in a single SQLite database (`~/.kiri/sessions.db`). Mirrors
/// `SqliteSharedMemory`: the blocking `rusqlite` connection lives behind `Arc<Mutex<_>>` and every query
/// runs on a blocking thread bounded by `DB_OP_TIMEOUT`, so a slow disk never stalls the TUI runtime.
pub struct SqliteSessionStore {
    conn: Arc<Mutex<Connection>>,
    /// Whether `new` (not `in_memory_inert`) backed this store. Surfaced via `is_available()`, the
    /// canonical inert-store signal this tree converges on — the sibling memory store adopts the same
    /// model once its SQLite harness is unified.
    available: bool,
}

impl SqliteSessionStore {
    /// Open (creating it and its parent directory if needed) the sessions database. Call `init` for the
    /// schema. A store opened this way reports available. An open/IO-class failure here (missing parent,
    /// permissions, a locked file) surfaces as `AgentError::session` — that constructor deliberately also
    /// carries SQLite-open failures, not only the non-IO query errors, rather than add a separate IO class.
    pub fn new(db_path: PathBuf) -> AgentResult<Self> {
        let conn = open_with_parent(&db_path, AgentError::session)?;
        // ~/.kiri/sessions.db is global across every workspace and terminal, so a second running Kiri
        // can contend for a write. SQLITE_BUSY returns instantly (the op timeout cannot wait it out), so
        // a busy_timeout lets brief cross-process contention be waited out instead of failing — kept below
        // DB_OP_TIMEOUT (see SESSION_BUSY_TIMEOUT) so a persistent lock fails deterministically before the
        // op timeout fires. WAL is a best-effort throughput win that also reduces reader/writer contention.
        conn.busy_timeout(SESSION_BUSY_TIMEOUT)
            .map_err(AgentError::session)?;
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            available: true,
        })
    }

    /// An ephemeral, inert in-memory store used as the degraded fallback when the on-disk database
    /// cannot be opened or initialized — the harness still wires a (unavailable) store instead of
    /// failing to start. Reports `is_available() == false`.
    pub fn in_memory_inert() -> AgentResult<Self> {
        let conn = Connection::open_in_memory().map_err(AgentError::session)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            available: false,
        })
    }
}

/// Add the `thinking` column to `messages` if it is missing. No migration framework exists in this
/// codebase; `CREATE TABLE IF NOT EXISTS` is a no-op on a table that already exists, so a database created
/// before this column was introduced needs it added in place. Idempotent (checked on every `init()`,
/// altered at most once) — mirrors the existing `DROP INDEX IF EXISTS` + recreate precedent for evolving
/// this same table.
fn add_thinking_column_if_missing(conn: &Connection) -> AgentResult<()> {
    let has_column: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('messages') WHERE name = 'thinking'")
        .and_then(|mut stmt| stmt.exists([]))
        .map_err(AgentError::session)?;
    if has_column {
        return Ok(());
    }
    match conn.execute("ALTER TABLE messages ADD COLUMN thinking TEXT", []) {
        Ok(_) => Ok(()),
        // Two Kiri processes can race this exact check-then-ALTER window on a shared, not-yet-migrated
        // `~/.kiri/sessions.db` (e.g. simultaneous first launch after upgrading). SQLite has no distinct
        // error code for this, only the message text, so the loser recognizes and tolerates it instead of
        // degrading its whole session store to the in-memory inert fallback over a column that now exists.
        Err(error) if is_duplicate_column_error(&error) => Ok(()),
        Err(error) => Err(AgentError::session(error)),
    }
}

fn is_duplicate_column_error(error: &rusqlite::Error) -> bool {
    matches!(error, rusqlite::Error::SqliteFailure(_, Some(message)) if message.contains("duplicate column name"))
}

#[async_trait::async_trait]
impl SessionStore for SqliteSessionStore {
    async fn init(&self) -> AgentResult<()> {
        let conn = self.conn.clone();
        run_blocking(move || -> AgentResult<()> {
            let conn = lock(&conn, AgentError::session)?;
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
            .map_err(AgentError::session)?;
            add_thinking_column_if_missing(&conn)?;
            Ok(())
        }, AgentError::session)
        .await
    }

    async fn create(&self, project_id: &str) -> AgentResult<Session> {
        let conn = self.conn.clone();
        let project_id = project_id.to_string();
        run_blocking(
            move || -> AgentResult<Session> {
                let id = Uuid::now_v7().to_string();
                let now = now_rfc3339();
                let conn = lock(&conn, AgentError::session)?;
                conn.execute(
                    "INSERT INTO sessions (id, project_id, title, created_at, updated_at)
                 VALUES (?1, ?2, '', ?3, ?3)",
                    params![id, project_id, now],
                )
                .map_err(AgentError::session)?;
                Ok(Session {
                    id,
                    title: String::new(),
                    messages: Vec::new(),
                    skipped_messages: 0,
                })
            },
            AgentError::session,
        )
        .await
    }

    async fn append_messages(&self, session_id: &str, messages: &[Message]) -> AgentResult<()> {
        if messages.is_empty() {
            return Ok(());
        }
        let conn = self.conn.clone();
        let session_id = session_id.to_string();
        // Serialize off the lock: turn each domain message into its stored columns up front.
        #[allow(clippy::type_complexity)]
        let rows: Vec<(
            String,
            Option<String>,
            String,
            String,
            Option<String>,
            Option<String>,
        )> = messages
            .iter()
            .map(|message| {
                let dto = StoredMessage::from(message);
                let images = serde_json::to_string(&dto.images).map_err(AgentError::session)?;
                let tool_calls =
                    serde_json::to_string(&dto.tool_calls).map_err(AgentError::session)?;
                let thinking = dto
                    .thinking
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()
                    .map_err(AgentError::session)?;
                Ok((
                    dto.role,
                    dto.content,
                    images,
                    tool_calls,
                    dto.tool_call_id,
                    thinking,
                ))
            })
            .collect::<AgentResult<_>>()?;
        run_blocking(
            move || -> AgentResult<()> {
                let now = now_rfc3339();
                let mut guard = lock(&conn, AgentError::session)?;
                // IMMEDIATE takes the write lock before the MAX(ordinal) read, so a second process cannot read
                // the same MAX and assign a duplicate ordinal. The RAII transaction rolls back on any `?`
                // early-return (no stranded transaction on the shared connection).
                let tx = guard
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .map_err(AgentError::session)?;
                let base: i64 = tx
                    .query_row(
                        "SELECT COALESCE(MAX(ordinal), -1) + 1 FROM messages WHERE session_id = ?1",
                        params![session_id],
                        |row| row.get(0),
                    )
                    .map_err(AgentError::session)?;
                for (offset, (role, content, images, tool_calls, tool_call_id, thinking)) in
                    rows.iter().enumerate()
                {
                    let ordinal = base + offset as i64;
                    tx.execute(
                        "INSERT INTO messages
                        (session_id, ordinal, role, content, images, tool_calls, tool_call_id, thinking)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                        params![
                            session_id,
                            ordinal,
                            role,
                            content,
                            images,
                            tool_calls,
                            tool_call_id,
                            thinking
                        ],
                    )
                    .map_err(AgentError::session)?;
                }
                tx.execute(
                    "UPDATE sessions SET updated_at = ?2 WHERE id = ?1",
                    params![session_id, now],
                )
                .map_err(AgentError::session)?;
                tx.commit().map_err(AgentError::session)?;
                Ok(())
            },
            AgentError::session,
        )
        .await
    }

    async fn set_title(&self, session_id: &str, title: &str) -> AgentResult<()> {
        let conn = self.conn.clone();
        let session_id = session_id.to_string();
        let title = title.to_string();
        run_blocking(
            move || -> AgentResult<()> {
                let conn = lock(&conn, AgentError::session)?;
                conn.execute(
                    "UPDATE sessions SET title = ?2 WHERE id = ?1",
                    params![session_id, title],
                )
                .map_err(AgentError::session)?;
                Ok(())
            },
            AgentError::session,
        )
        .await
    }

    async fn latest_for_project(&self, project_id: &str) -> AgentResult<Option<SessionSummary>> {
        let mut summaries = self.list_for_project(project_id, 1).await?;
        Ok(summaries.drain(..).next())
    }

    async fn list_for_project(
        &self,
        project_id: &str,
        limit: usize,
    ) -> AgentResult<Vec<SessionSummary>> {
        let conn = self.conn.clone();
        let project_id = project_id.to_string();
        run_blocking(
            move || -> AgentResult<Vec<SessionSummary>> {
                let conn = lock(&conn, AgentError::session)?;
                let mut stmt = conn
                    .prepare(
                        "SELECT s.id, s.title, s.updated_at,
                            (SELECT COUNT(*) FROM messages m WHERE m.session_id = s.id)
                     FROM sessions s
                     WHERE s.project_id = ?1
                     ORDER BY s.updated_at DESC
                     LIMIT ?2",
                    )
                    .map_err(AgentError::session)?;
                let rows = stmt
                    .query_map(params![project_id, limit as i64], |row| {
                        Ok(SessionSummary {
                            id: row.get(0)?,
                            title: row.get(1)?,
                            updated_at: row.get(2)?,
                            message_count: row.get::<_, i64>(3)? as usize,
                        })
                    })
                    .map_err(AgentError::session)?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(AgentError::session)?);
                }
                Ok(out)
            },
            AgentError::session,
        )
        .await
    }

    async fn recent_user_prompts(
        &self,
        project_id: &str,
        limit: usize,
    ) -> AgentResult<Vec<String>> {
        let conn = self.conn.clone();
        let project_id = project_id.to_string();
        run_blocking(
            move || -> AgentResult<Vec<String>> {
                let conn = lock(&conn, AgentError::session)?;
                let mut stmt = conn
                    .prepare(
                        "SELECT m.content FROM messages m
                     JOIN sessions s ON m.session_id = s.id
                     WHERE s.project_id = ?1 AND m.role = 'user' AND m.content IS NOT NULL
                     ORDER BY s.updated_at DESC, m.ordinal DESC
                     LIMIT ?2",
                    )
                    .map_err(AgentError::session)?;
                let rows = stmt
                    .query_map(params![project_id, limit as i64], |row| {
                        row.get::<_, String>(0)
                    })
                    .map_err(AgentError::session)?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(AgentError::session)?);
                }
                Ok(out)
            },
            AgentError::session,
        )
        .await
    }

    async fn load(&self, session_id: &str) -> AgentResult<Option<Session>> {
        let conn = self.conn.clone();
        let session_id = session_id.to_string();
        run_blocking(
            move || -> AgentResult<Option<Session>> {
                let conn = lock(&conn, AgentError::session)?;
                let title = match conn.query_row(
                    "SELECT title FROM sessions WHERE id = ?1",
                    params![session_id],
                    |row| row.get::<_, String>(0),
                ) {
                    Ok(title) => title,
                    // Absent session is `Ok(None)`; a real DB error (locked/corrupt/IO) must surface, not be
                    // reported to the user as "session not found".
                    Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
                    Err(error) => return Err(AgentError::session(error)),
                };
                let mut stmt = conn
                    .prepare(
                        "SELECT role, content, images, tool_calls, tool_call_id, thinking
                     FROM messages WHERE session_id = ?1 ORDER BY ordinal",
                    )
                    .map_err(AgentError::session)?;
                let rows = stmt
                    .query_map(params![session_id], |row| {
                        let images_raw: String = row.get(2)?;
                        let tool_calls_raw: String = row.get(3)?;
                        let thinking_raw: Option<String> = row.get(5)?;
                        // A corrupt images/tool_calls/thinking column makes the row unsafe to keep: silently
                        // emptying tool_calls would leave an assistant message whose calls vanished while its
                        // answers (Role::Tool rows) still reference them — an orphaned exchange the provider
                        // rejects. Skip the whole message instead (returned as None, dropped below).
                        let images = match serde_json::from_str(&images_raw) {
                            Ok(value) => value,
                            Err(_) => return Ok(None),
                        };
                        let tool_calls = match serde_json::from_str(&tool_calls_raw) {
                            Ok(value) => value,
                            Err(_) => return Ok(None),
                        };
                        let thinking = match thinking_raw {
                            None => None,
                            Some(raw) => match serde_json::from_str(&raw) {
                                Ok(value) => value,
                                Err(_) => return Ok(None),
                            },
                        };
                        Ok(Some(StoredMessage {
                            role: row.get(0)?,
                            content: row.get(1)?,
                            images,
                            tool_calls,
                            tool_call_id: row.get(4)?,
                            thinking,
                        }))
                    })
                    .map_err(AgentError::session)?;
                let mut messages = Vec::new();
                let mut skipped = 0usize;
                for row in rows {
                    // Skip a corrupt row (unparseable images/tool_calls) or one with an unrecognized role
                    // rather than abort the load; count the drops so the resume path can surface that the
                    // conversation was silently shortened instead of losing turns invisibly.
                    match row
                        .map_err(AgentError::session)?
                        .and_then(StoredMessage::into_domain)
                    {
                        Some(message) => messages.push(message),
                        None => skipped += 1,
                    }
                }
                Ok(Some(Session {
                    id: session_id,
                    title,
                    messages,
                    skipped_messages: skipped,
                }))
            },
            AgentError::session,
        )
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

    #[test]
    fn busy_timeout_is_strictly_below_the_op_timeout() {
        // BUG-01: if busy_timeout == DB_OP_TIMEOUT, the op-level tokio timeout can cancel an in-flight
        // commit on a persistent cross-process lock, leaving flush_session to re-append the same delta
        // and duplicate messages on resume. The busy_timeout must resolve SQLITE_BUSY first.
        assert!(
            SESSION_BUSY_TIMEOUT < crate::shared::infra::sqlite::DB_OP_TIMEOUT,
            "SESSION_BUSY_TIMEOUT {SESSION_BUSY_TIMEOUT:?} must be < DB_OP_TIMEOUT to fail deterministically first"
        );
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

        let guard = lock(&store.conn, AgentError::session).unwrap();
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

        let guard = lock(&store.conn, AgentError::session).unwrap();
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
            let mut guard = lock(&store.conn, AgentError::session).unwrap();
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

    #[tokio::test]
    async fn load_reports_skipped_corrupt_rows() {
        let dir = TempDir::new().unwrap();
        let store = store(&dir).await;
        let session = store.create("proj-a").await.unwrap();
        store
            .append_messages(&session.id, &[Message::user("intact")])
            .await
            .unwrap();

        // Plant a corrupt row: `tool_calls` is not valid JSON, so load must drop it (never abort) and
        // report the drop instead of silently shortening the conversation.
        {
            let guard = lock(&store.conn, AgentError::session).unwrap();
            guard
                .execute(
                    "INSERT INTO messages (session_id, ordinal, role, content, images, tool_calls)
                     VALUES (?1, 1, 'assistant', 'broken', '[]', 'not-json')",
                    params![session.id],
                )
                .unwrap();
        }

        let loaded = store.load(&session.id).await.unwrap().unwrap();
        assert_eq!(
            loaded.skipped_messages, 1,
            "the corrupt row must be counted as skipped"
        );
        assert_eq!(loaded.messages.len(), 1, "the intact message survives");
        assert_eq!(loaded.messages[0].content.as_deref(), Some("intact"));
    }

    #[tokio::test]
    async fn append_and_load_round_trip_thinking_blocks() {
        use crate::shared::kernel::message::ThinkingBlock;

        let dir = TempDir::new().unwrap();
        let store = store(&dir).await;
        let session = store.create("proj-a").await.unwrap();

        let visible = Message::assistant_text("2 + 2 = 4").with_thinking(ThinkingBlock::Visible {
            text: "adding".to_string(),
            signature: Some("sig".to_string()),
        });
        let redacted = Message::assistant_text("done").with_thinking(ThinkingBlock::Redacted {
            data: "encrypted-blob".to_string(),
        });
        store
            .append_messages(
                &session.id,
                &[visible, redacted, Message::user("no thinking")],
            )
            .await
            .unwrap();

        let loaded = store.load(&session.id).await.unwrap().unwrap();
        assert_eq!(loaded.messages.len(), 3);
        match loaded.messages[0].thinking.as_ref().unwrap() {
            ThinkingBlock::Visible { text, signature } => {
                assert_eq!(text, "adding");
                assert_eq!(signature.as_deref(), Some("sig"));
            }
            other => panic!("expected Visible, got {other:?}"),
        }
        match loaded.messages[1].thinking.as_ref().unwrap() {
            ThinkingBlock::Redacted { data } => assert_eq!(data, "encrypted-blob"),
            other => panic!("expected Redacted, got {other:?}"),
        }
        assert!(loaded.messages[2].thinking.is_none());
    }

    #[tokio::test]
    async fn legacy_messages_table_gets_the_thinking_column_and_old_rows_load_as_none() {
        // Simulate a `~/.kiri/sessions.db` created before the `thinking` column existed: a `messages`
        // table matching the pre-migration schema exactly, with a row already in it. `init()` must add
        // the column in place (not just on a brand-new table) and the pre-existing row must load with
        // `thinking: None` rather than being dropped as corrupt.
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sessions.db");
        let store = SqliteSessionStore::new(db).unwrap();
        {
            let guard = lock(&store.conn, AgentError::session).unwrap();
            guard
                .execute_batch(
                    "CREATE TABLE sessions (
                        id TEXT PRIMARY KEY, project_id TEXT NOT NULL, title TEXT NOT NULL DEFAULT '',
                        created_at TEXT NOT NULL, updated_at TEXT NOT NULL
                    );
                    CREATE TABLE messages (
                        id INTEGER PRIMARY KEY AUTOINCREMENT,
                        session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                        ordinal INTEGER NOT NULL, role TEXT NOT NULL, content TEXT,
                        images TEXT NOT NULL DEFAULT '[]', tool_calls TEXT NOT NULL DEFAULT '[]',
                        tool_call_id TEXT
                    );
                    INSERT INTO sessions (id, project_id, created_at, updated_at)
                        VALUES ('s1', 'proj-a', 't', 't');
                    INSERT INTO messages (session_id, ordinal, role, content)
                        VALUES ('s1', 0, 'user', 'from before the migration');",
                )
                .unwrap();
        }

        store.init().await.unwrap();

        let has_column: bool = {
            let guard = lock(&store.conn, AgentError::session).unwrap();
            guard
                .prepare("SELECT 1 FROM pragma_table_info('messages') WHERE name = 'thinking'")
                .unwrap()
                .exists([])
                .unwrap()
        };
        assert!(
            has_column,
            "init() must add the thinking column to a legacy messages table"
        );

        let loaded = store.load("s1").await.unwrap().unwrap();
        assert_eq!(
            loaded.messages.len(),
            1,
            "the legacy row must not be dropped"
        );
        assert_eq!(
            loaded.messages[0].content.as_deref(),
            Some("from before the migration")
        );
        assert!(
            loaded.messages[0].thinking.is_none(),
            "a NULL thinking column on a legacy row must load as None, not a corrupt-row drop"
        );
    }

    #[test]
    fn duplicate_column_alter_is_recognized_as_a_benign_race() {
        // Deterministically reproduces the exact error SQLite returns when a second process wins the
        // check-then-ALTER race: the column already exists, so a raw ALTER against it fails with
        // "duplicate column name", not a distinct error code.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE messages (id INTEGER PRIMARY KEY, thinking TEXT)")
            .unwrap();
        let error = conn
            .execute("ALTER TABLE messages ADD COLUMN thinking TEXT", [])
            .unwrap_err();
        assert!(
            is_duplicate_column_error(&error),
            "expected a duplicate-column error, got {error}"
        );
    }

    #[tokio::test]
    async fn recent_user_prompts_filters_projects_and_roles_ordered_newest_first_with_limit() {
        let dir = TempDir::new().unwrap();
        let store = store(&dir).await;

        let s1 = store.create("proj-a").await.unwrap();
        store
            .append_messages(
                &s1.id,
                &[Message::user("first"), Message::assistant_text("reply")],
            )
            .await
            .unwrap();
        let s2 = store.create("proj-a").await.unwrap();
        store
            .append_messages(&s2.id, &[Message::user("second"), Message::user("third")])
            .await
            .unwrap();
        let other = store.create("proj-b").await.unwrap();
        store
            .append_messages(&other.id, &[Message::user("other project")])
            .await
            .unwrap();

        let prompts = store.recent_user_prompts("proj-a", 10).await.unwrap();
        assert_eq!(
            prompts,
            vec!["third", "second", "first"],
            "newest first across sessions, assistant rows and other projects excluded"
        );

        let limited = store.recent_user_prompts("proj-a", 2).await.unwrap();
        assert_eq!(limited, vec!["third", "second"], "limit is respected");
    }
}
