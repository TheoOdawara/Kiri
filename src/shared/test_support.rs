//! Shared test-only fixtures. Compiled only under `cfg(test)`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A throwaway directory under the system temp dir, removed on drop. Kept hand-rolled to avoid a
/// `tempfile` dev-dependency; `tag` distinguishes call sites in the path for debugging, and a process
/// id plus an atomic counter keep concurrent tests from colliding.
pub struct TempDir {
    pub path: PathBuf,
}

impl TempDir {
    pub fn new(tag: &str) -> Self {
        let mut path = std::env::temp_dir();
        let pid = std::process::id();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        path.push(format!("t-cli-{tag}-{pid}-{n}"));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
