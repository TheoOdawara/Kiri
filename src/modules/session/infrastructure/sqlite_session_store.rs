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

/// Must stay STRICTLY BELOW `DB_OP_TIMEOUT`: a persistent cross-process lock has to surface as
/// `SQLITE_BUSY` before the op-level `tokio::time` timeout can cancel an in-flight commit — that
/// cancellation would leave `flush_session` re-appending the same delta and duplicating messages on
/// resume (BUG-01). It does not close a non-BUSY stall; `MESSAGE_UUID_NAMESPACE` closes that residual.
const SESSION_BUSY_TIMEOUT: Duration = Duration::from_secs(3);

/// A message's identity is `uuid_v5(MESSAGE_UUID_NAMESPACE, "{salt}:{abs_index}")`. Retrying the same
/// flush — the case where a "failed" append actually committed under a cancelled timeout — recomputes the
/// same uuids and is dropped by the `UNIQUE(session_id, message_uuid)` index, closing BUG-01's residual by
/// construction rather than by timing. A different salt (a second process on the same session) yields
/// different uuids, so concurrent appends are never falsely deduplicated. The value is arbitrary but must
/// stay fixed across runs.
const MESSAGE_UUID_NAMESPACE: Uuid = uuid::uuid!("1f3e5a9c-7721-4e88-930a-621d4b7fa102");

/// Every query runs on a blocking thread bounded by `DB_OP_TIMEOUT`, so a slow disk never stalls the TUI.
pub struct SqliteSessionStore {
    conn: Arc<Mutex<Connection>>,
    available: bool,
}

impl SqliteSessionStore {
    /// Opens the database (and its parent dir); the schema comes from `init`.
    pub fn new(db_path: PathBuf) -> AgentResult<Self> {
        let conn = open_with_parent(&db_path, AgentError::session)?;
        // `~/.kiri/sessions.db` is shared across every workspace and terminal, and SQLITE_BUSY returns
        // instantly, so a second running Kiri needs a busy_timeout to wait out brief write contention.
        conn.busy_timeout(SESSION_BUSY_TIMEOUT)
            .map_err(AgentError::session)?;
        // WAL is a best-effort throughput win; the rollback journal is correct too, so a refusal is safe.
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            available: true,
        })
    }

    /// Degraded fallback when the on-disk database cannot be opened: the harness wires an unavailable
    /// store rather than failing to start.
    pub fn in_memory_inert() -> AgentResult<Self> {
        let conn = Connection::open_in_memory().map_err(AgentError::session)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            available: false,
        })
    }
}

/// There is no migration framework, and `CREATE TABLE IF NOT EXISTS` is a no-op on an existing table,
/// so a database created before this column needs it added in place.
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
        // Two processes can race the check-then-ALTER window; the loser tolerates it rather than degrade
        // to the inert fallback over a column that now exists.
        Err(error) if is_duplicate_column_error(&error) => Ok(()),
        Err(error) => Err(AgentError::session(error)),
    }
}

fn is_duplicate_column_error(error: &rusqlite::Error) -> bool {
    matches!(error, rusqlite::Error::SqliteFailure(_, Some(message)) if message.contains("duplicate column name"))
}

/// Add the `message_uuid` column to `messages` if it is missing (issue #34), mirroring
/// `add_thinking_column_if_missing`'s in-place-migration precedent exactly, including the same benign
/// concurrent-ALTER race tolerance. Existing rows get `NULL` — SQLite treats each `NULL` as distinct in a
/// `UNIQUE` index, so legacy rows coexist under `idx_messages_session_uuid` without a backfill.
fn add_message_uuid_column_if_missing(conn: &Connection) -> AgentResult<()> {
    let has_column: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('messages') WHERE name = 'message_uuid'")
        .and_then(|mut stmt| stmt.exists([]))
        .map_err(AgentError::session)?;
    if has_column {
        return Ok(());
    }
    match conn.execute("ALTER TABLE messages ADD COLUMN message_uuid TEXT", []) {
        Ok(_) => Ok(()),
        Err(error) if is_duplicate_column_error(&error) => Ok(()),
        Err(error) => Err(AgentError::session(error)),
    }
}

#[async_trait::async_trait]
impl SessionStore for SqliteSessionStore {
    async fn init(&self) -> AgentResult<()> {
        let conn = self.conn.clone();
        run_blocking(move || -> AgentResult<()> {
            let conn = lock(&conn, AgentError::session)?;
            // foreign_keys is per-connection, not per-database.
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
                -- UNIQUE replaces the prior non-unique index: it is both the lookup index and the guard
                -- that fails an insert if a cross-process race ever produced a duplicate ordinal.
                DROP INDEX IF EXISTS idx_messages_session;
                CREATE UNIQUE INDEX IF NOT EXISTS idx_messages_session_ordinal
                    ON messages(session_id, ordinal);",
            )
            .map_err(AgentError::session)?;
            add_thinking_column_if_missing(&conn)?;
            add_message_uuid_column_if_missing(&conn)?;
            // The uuid index is created only after the column above is guaranteed to exist — it must
            // run separately from the CREATE TABLE batch above, which never adds the column itself (it
            // mirrors `thinking`, added purely through migration so new and legacy DBs share one path).
            conn.execute_batch(
                "CREATE UNIQUE INDEX IF NOT EXISTS idx_messages_session_uuid
                    ON messages(session_id, message_uuid);",
            )
            .map_err(AgentError::session)?;
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

    async fn append_messages(
        &self,
        session_id: &str,
        base_index: usize,
        salt: &str,
        messages: &[Message],
    ) -> AgentResult<()> {
        if messages.is_empty() {
            return Ok(());
        }
        let conn = self.conn.clone();
        let session_id = session_id.to_string();
        let salt = salt.to_string();
        // Serialize before taking the lock, not under it.
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
                // IMMEDIATE takes the write lock before the MAX(ordinal) read, so a second process cannot
                // read the same MAX and assign a duplicate ordinal.
                let tx = guard
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .map_err(AgentError::session)?;
                let mut ordinal: i64 = tx
                    .query_row(
                        "SELECT COALESCE(MAX(ordinal), -1) + 1 FROM messages WHERE session_id = ?1",
                        params![session_id],
                        |row| row.get(0),
                    )
                    .map_err(AgentError::session)?;
                let mut inserted = 0usize;
                for (offset, (role, content, images, tool_calls, tool_call_id, thinking)) in
                    rows.iter().enumerate()
                {
                    let abs_index = base_index + offset;
                    let message_uuid = Uuid::new_v5(
                        &MESSAGE_UUID_NAMESPACE,
                        format!("{salt}:{abs_index}").as_bytes(),
                    )
                    .to_string();
                    // ON CONFLICT(session_id, message_uuid) DO NOTHING: a message whose uuid already
                    // exists is a retry of an earlier flush whose commit actually landed (issue #34) —
                    // silently deduplicated, and its ordinal slot is never consumed (see below), so no
                    // gap is left behind. Scoped to that ONE index deliberately — a blanket
                    // `INSERT OR IGNORE` would also swallow an unrelated ordinal collision or NOT
                    // NULL/FK violation, which must still propagate as a hard error (security review).
                    let changed = tx
                        .execute(
                            "INSERT INTO messages
                            (session_id, ordinal, role, content, images, tool_calls, tool_call_id,
                             thinking, message_uuid)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                         ON CONFLICT(session_id, message_uuid) DO NOTHING",
                            params![
                                session_id,
                                ordinal,
                                role,
                                content,
                                images,
                                tool_calls,
                                tool_call_id,
                                thinking,
                                message_uuid
                            ],
                        )
                        .map_err(AgentError::session)?;
                    if changed == 1 {
                        ordinal += 1;
                        inserted += 1;
                    }
                }
                // Skip the timestamp bump on a fully-deduplicated retry (nothing actually changed) —
                // only a real insert should move `updated_at`.
                if inserted > 0 {
                    tx.execute(
                        "UPDATE sessions SET updated_at = ?2 WHERE id = ?1",
                        params![session_id, now],
                    )
                    .map_err(AgentError::session)?;
                }
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
                    // Only an absent session is `Ok(None)`; a locked/corrupt/IO error must not be reported
                    // to the user as "session not found".
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
                        // Skip the whole row rather than default a corrupt column: an emptied `tool_calls`
                        // leaves the `Role::Tool` answers referencing calls that vanished, and the provider
                        // rejects that orphaned exchange.
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

    /// A fixed salt for tests that don't exercise the concurrency-preserving property itself — mirrors
    /// one `RunLoop`'s stable per-conversation `SessionCursor::salt`.
    const TEST_SALT: &str = "test-salt";

    async fn store(dir: &TempDir) -> SqliteSessionStore {
        let db = dir.path().join("sessions.db");
        let store = SqliteSessionStore::new(db).unwrap();
        store.init().await.unwrap();
        store
    }

    #[test]
    fn busy_timeout_is_strictly_below_the_op_timeout() {
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
                0,
                TEST_SALT,
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
            .append_messages(&session.id, 0, TEST_SALT, &[Message::user("first")])
            .await
            .unwrap();
        store
            .append_messages(
                &session.id,
                1,
                TEST_SALT,
                &[Message::assistant_text("second")],
            )
            .await
            .unwrap();
        store
            .append_messages(&session.id, 2, TEST_SALT, &[Message::user("third")])
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
                0,
                TEST_SALT,
                &[Message::user("a"), Message::assistant_text("b")],
            )
            .await
            .unwrap();
        store
            .append_messages(&session.id, 2, TEST_SALT, &[Message::user("c")])
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

    // Security review (issue #34): the dedup insert must scope its conflict suppression to
    // `(session_id, message_uuid)` ONLY — a blanket `INSERT OR IGNORE` would also silently swallow an
    // unrelated ordinal collision, which must still propagate as a hard error, not vanish unnoticed.
    #[tokio::test]
    async fn dedup_insert_still_errors_on_an_unrelated_ordinal_collision() {
        let dir = TempDir::new().unwrap();
        let store = store(&dir).await;
        let session = store.create("proj-a").await.unwrap();

        let guard = lock(&store.conn, AgentError::session).unwrap();
        guard
            .execute(
                "INSERT INTO messages (session_id, ordinal, role, content, message_uuid)
                 VALUES (?1, 0, 'user', 'a', 'uuid-a')",
                params![session.id],
            )
            .unwrap();
        // Same statement shape production uses, at the SAME ordinal but a DIFFERENT message_uuid: the
        // `ON CONFLICT(session_id, message_uuid)` target does not match this row's cause of failure
        // (the ordinal's unique index), so it must still surface as an error rather than being ignored.
        let colliding = guard.execute(
            "INSERT INTO messages (session_id, ordinal, role, content, message_uuid)
             VALUES (?1, 0, 'user', 'b', 'uuid-b')
             ON CONFLICT(session_id, message_uuid) DO NOTHING",
            params![session.id],
        );
        assert!(
            colliding.is_err(),
            "an ordinal collision with a different message_uuid must not be silently ignored"
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
                0,
                TEST_SALT,
                &[Message::user("a"), Message::assistant_text("b")],
            )
            .await
            .unwrap();

        // `append_messages` never self-collides (MAX(ordinal)+1 is always free), so the rollback path is
        // forced by driving the same IMMEDIATE transaction by hand.
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
            // `tx` drops uncommitted here: the rollback discards the ordinal-2 insert too.
        }

        let loaded = store.load(&session.id).await.unwrap().unwrap();
        assert_eq!(
            loaded.messages.len(),
            2,
            "the rolled-back insert must not persist"
        );

        store
            .append_messages(&session.id, 2, TEST_SALT, &[Message::user("c")])
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
            .append_messages(&s1.id, 0, TEST_SALT, &[Message::user("a")])
            .await
            .unwrap();
        let s2 = store.create("proj-a").await.unwrap();
        store
            .append_messages(&s2.id, 0, TEST_SALT, &[Message::user("b")])
            .await
            .unwrap();
        let _other = store.create("proj-b").await.unwrap();

        let list = store.list_for_project("proj-a", 10).await.unwrap();
        assert_eq!(list.len(), 2);
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
                .append_messages(&session.id, 0, TEST_SALT, &[Message::user("persisted")])
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
            .append_messages(&session.id, 0, TEST_SALT, &[Message::user("intact")])
            .await
            .unwrap();

        // `tool_calls` is deliberately not valid JSON.
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
                0,
                TEST_SALT,
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
        // The schema below is the pre-migration one, verbatim.
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
        // SQLite signals this only in the message text, never a distinct error code.
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
                0,
                TEST_SALT,
                &[Message::user("first"), Message::assistant_text("reply")],
            )
            .await
            .unwrap();
        let s2 = store.create("proj-a").await.unwrap();
        store
            .append_messages(
                &s2.id,
                0,
                TEST_SALT,
                &[Message::user("second"), Message::user("third")],
            )
            .await
            .unwrap();
        let other = store.create("proj-b").await.unwrap();
        store
            .append_messages(&other.id, 0, TEST_SALT, &[Message::user("other project")])
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

    // Issue #34 / BUG-01 residual: a delta that the caller believes failed (op-level timeout) but whose
    // commit actually landed underneath must not duplicate when retried with an unmoved cursor.
    #[tokio::test]
    async fn append_messages_is_idempotent_under_a_retried_delta() {
        let dir = TempDir::new().unwrap();
        let store = store(&dir).await;
        let session = store.create("proj-a").await.unwrap();
        let delta = [Message::user("a"), Message::assistant_text("b")];

        store
            .append_messages(&session.id, 0, TEST_SALT, &delta)
            .await
            .unwrap();
        // Retry: same base_index, same salt, same messages — simulates the caller re-sending after a
        // timeout whose commit had actually landed.
        store
            .append_messages(&session.id, 0, TEST_SALT, &delta)
            .await
            .unwrap();

        let loaded = store.load(&session.id).await.unwrap().unwrap();
        assert_eq!(
            loaded.messages.len(),
            2,
            "a retried delta must not duplicate messages"
        );
    }

    #[tokio::test]
    async fn append_messages_retry_leaves_ordinals_contiguous() {
        // The dedup must not leave gaps: an ignored (duplicate) row must not consume an ordinal slot.
        let dir = TempDir::new().unwrap();
        let store = store(&dir).await;
        let session = store.create("proj-a").await.unwrap();

        store
            .append_messages(&session.id, 0, TEST_SALT, &[Message::user("a")])
            .await
            .unwrap();
        // Full retry of the same base_index/salt/message — must not move the ordinal counter — followed
        // by a genuinely new message at the next real position.
        store
            .append_messages(&session.id, 0, TEST_SALT, &[Message::user("a")])
            .await
            .unwrap();
        store
            .append_messages(&session.id, 1, TEST_SALT, &[Message::user("b")])
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
        assert_eq!(
            ordinals,
            vec![0, 1],
            "an ignored duplicate row must not leave a gap in the ordinal sequence"
        );
    }

    #[tokio::test]
    async fn append_messages_preserves_concurrent_appends_with_different_salts() {
        // Two RunLoops (e.g. two terminals resuming the same session) racing the same base_index must
        // NOT be falsely deduplicated — each has its own per-conversation salt, so their message uuids
        // differ even at the same abs_index, and both messages must persist.
        let dir = TempDir::new().unwrap();
        let store = store(&dir).await;
        let session = store.create("proj-a").await.unwrap();

        store
            .append_messages(&session.id, 0, "salt-process-a", &[Message::user("from a")])
            .await
            .unwrap();
        store
            .append_messages(&session.id, 0, "salt-process-b", &[Message::user("from b")])
            .await
            .unwrap();

        let loaded = store.load(&session.id).await.unwrap().unwrap();
        assert_eq!(
            loaded.messages.len(),
            2,
            "different salts at the same abs_index must not be deduplicated"
        );
    }

    #[tokio::test]
    async fn legacy_messages_table_gets_the_message_uuid_column_and_stays_idempotent() {
        // Simulate a `~/.kiri/sessions.db` created before `message_uuid` existed (already past the
        // `thinking` migration, but no message_uuid column and no uuid index yet). `init()` must add the
        // column AND the unique index in place, and a subsequent append on the migrated table must be
        // idempotent going forward.
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
                        tool_call_id TEXT, thinking TEXT
                    );
                    CREATE UNIQUE INDEX idx_messages_session_ordinal ON messages(session_id, ordinal);
                    INSERT INTO sessions (id, project_id, created_at, updated_at)
                        VALUES ('s1', 'proj-a', 't', 't');
                    INSERT INTO messages (session_id, ordinal, role, content)
                        VALUES ('s1', 0, 'user', 'from before message_uuid');",
                )
                .unwrap();
        }

        store.init().await.unwrap();

        let has_column: bool = {
            let guard = lock(&store.conn, AgentError::session).unwrap();
            guard
                .prepare("SELECT 1 FROM pragma_table_info('messages') WHERE name = 'message_uuid'")
                .unwrap()
                .exists([])
                .unwrap()
        };
        assert!(
            has_column,
            "init() must add the message_uuid column to a legacy messages table"
        );

        // A fresh append on the migrated table must succeed and be idempotent going forward.
        store
            .append_messages("s1", 1, TEST_SALT, &[Message::user("after migration")])
            .await
            .unwrap();
        store
            .append_messages("s1", 1, TEST_SALT, &[Message::user("after migration")])
            .await
            .unwrap();

        let loaded = store.load("s1").await.unwrap().unwrap();
        assert_eq!(
            loaded.messages.len(),
            2,
            "legacy row + one new idempotently-appended message"
        );
    }
}
