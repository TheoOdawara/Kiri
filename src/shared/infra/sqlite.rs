use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::Connection;
use tokio::task::spawn_blocking;

use crate::shared::kernel::error::AgentError;

/// Upper bound for a single blocking database operation, so an op that never returns (a wedged lock or
/// pathological query) surfaces as a clear error instead of hanging the runtime. Shared by every
/// SQLite-backed store (memory, session). This is a last-resort safety valve, not a hot path: it wraps
/// `spawn_blocking`, so it also counts pool-queue wait, and on a contended CI runner (Windows Defender
/// scanning a freshly-created `.db`, ~1000 tests in parallel) a cold `init()` can spike past a tight
/// bound and flake. Kept generous — a genuine hang is still caught, just later; a false-positive is not.
pub const DB_OP_TIMEOUT: Duration = Duration::from_secs(30);

/// Open (creating its parent directory if needed) a SQLite database. The parent-dir failure flows to
/// `AgentError::Io` via the kernel `#[from]`; the open failure is classified through `map_err`, so each
/// store stamps its own variant (`AgentError::memory` / `AgentError::session`). The single source for the
/// blocking-store open path — per-store pragmas (the session store's `busy_timeout` / WAL) layer on after.
pub fn open_with_parent(
    db_path: &Path,
    map_err: fn(String) -> AgentError,
) -> Result<Connection, AgentError> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Connection::open(db_path).map_err(|error| map_err(error.to_string()))
}

/// Lock the shared connection, mapping a poisoned mutex through `map_err`. A poisoned mutex means a prior
/// holder panicked mid-operation; surfacing it (rather than recovering the guard) is the conservative
/// choice for an auxiliary store the harness can degrade without.
pub fn lock(
    conn: &Mutex<Connection>,
    map_err: fn(String) -> AgentError,
) -> Result<MutexGuard<'_, Connection>, AgentError> {
    conn.lock()
        .map_err(|error| map_err(format!("sqlite mutex poisoned: {error}")))
}

/// Run a blocking database closure on the blocking pool, bounded by `timeout`. Owns the `spawn_blocking`,
/// `tokio::time::timeout`, and join/timeout mapping that every query shares; the join-error branch (a
/// panicked or cancelled blocking task) and the timeout branch both build their `AgentError` through
/// `map_err`, so each store stamps its own variant. This is the testable seam: a test points here with a
/// tiny `timeout` to exercise the timeout path without a real multi-second sleep.
pub async fn run_blocking_with_timeout<T: Send + 'static>(
    op: impl FnOnce() -> Result<T, AgentError> + Send + 'static,
    timeout: Duration,
    map_err: fn(String) -> AgentError,
) -> Result<T, AgentError> {
    match tokio::time::timeout(timeout, spawn_blocking(op)).await {
        Ok(joined) => joined.map_err(|error| map_err(error.to_string()))?,
        Err(_) => Err(map_err("database operation timed out".to_string())),
    }
}

/// Run a blocking database closure bounded by the shared `DB_OP_TIMEOUT`. Production code calls this; it
/// delegates to `run_blocking_with_timeout` (the testable seam).
pub async fn run_blocking<T: Send + 'static>(
    op: impl FnOnce() -> Result<T, AgentError> + Send + 'static,
    map_err: fn(String) -> AgentError,
) -> Result<T, AgentError> {
    run_blocking_with_timeout(op, DB_OP_TIMEOUT, map_err).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn run_blocking_returns_the_op_result() {
        let value = run_blocking(|| Ok(42_i32), AgentError::memory)
            .await
            .unwrap();
        assert_eq!(value, 42);
    }

    #[tokio::test]
    async fn run_blocking_maps_a_panicking_closure_via_the_constructor() {
        let error = run_blocking(
            || -> Result<(), AgentError> { panic!("blocking task blew up") },
            AgentError::memory,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(error, AgentError::Memory(_)),
            "a panicked blocking task must map through the passed constructor, got {error:?}"
        );
    }

    #[tokio::test]
    async fn run_blocking_times_out_via_the_constructor() {
        // The testable seam: a near-zero timeout against a closure that blocks past it exercises the
        // timeout branch in milliseconds, never the production 30s DB_OP_TIMEOUT.
        let error = run_blocking_with_timeout(
            || {
                std::thread::sleep(Duration::from_millis(100));
                Ok::<(), AgentError>(())
            },
            Duration::from_millis(1),
            AgentError::session,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&error, AgentError::Session(message) if message.contains("timed out")),
            "the timeout must be built by the passed constructor, got {error:?}"
        );
    }

    #[test]
    fn lock_maps_a_poisoned_mutex_via_the_constructor() {
        let mutex = Mutex::new(Connection::open_in_memory().unwrap());
        // Poison the mutex: panic while holding the guard, then catch it so the test continues.
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = mutex.lock().unwrap();
            panic!("poison the lock");
        }));
        assert!(panicked.is_err());

        let error = lock(&mutex, AgentError::memory).unwrap_err();
        assert!(
            matches!(&error, AgentError::Memory(message) if message.contains("poisoned")),
            "a poisoned mutex must map through the passed constructor, got {error:?}"
        );
    }

    #[test]
    fn open_with_parent_creates_the_missing_parent_dir() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("nested").join("deeper").join("store.db");
        assert!(!db_path.parent().unwrap().exists());
        let conn = open_with_parent(&db_path, AgentError::memory).unwrap();
        assert!(db_path.parent().unwrap().exists());
        drop(conn);
    }
}
