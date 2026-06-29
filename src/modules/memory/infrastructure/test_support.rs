//! Shared test helpers for the memory module: a real file+SQLite-backed `Memory` over a temp dir,
//! a generic in-memory `InMemoryStore` double, a sandbox fixture, and a `ToolCall` builder.

use std::sync::{Arc, Mutex};

use tempfile::TempDir;

use crate::modules::memory::application::memory_port::{LayeredMemory, Memory};
use crate::modules::memory::application::memory_store::MemoryStore;
use crate::modules::memory::application::project_memory::ProjectMemory;
use crate::modules::memory::application::shared_memory::SharedMemory;
use crate::modules::memory::application::shared_store::SharedStore;
use crate::modules::memory::domain::entry::{MemoryEntry, MemoryKind};
use crate::modules::memory::infrastructure::file_project_memory::FileProjectMemory;
use crate::modules::memory::infrastructure::sqlite_shared_memory::SqliteSharedMemory;
use crate::modules::tools::infrastructure::sandbox::FsSandbox;
use crate::modules::tools::infrastructure::sensitive::SensitiveMatcher;
use crate::shared::kernel::error::AgentResult;
use crate::shared::kernel::tool_call::{FunctionCall, ToolCall};

/// A `Memory` backed by real file (project) and SQLite (shared) stores under `dir`.
pub async fn temp_port(dir: &TempDir) -> Arc<dyn Memory> {
    let project = FileProjectMemory::new(dir.path().join(".kiri").join("memory"));
    project.init().await.unwrap();
    let shared = SqliteSharedMemory::new(dir.path().join("shared.db")).unwrap();
    shared.init().await.unwrap();
    Arc::new(LayeredMemory::new(project, shared))
}

/// A single in-memory store double covering both the base `MemoryStore` surface and the `SharedStore`
/// extension, replacing the per-module hand-rolled doubles. Keeps entries in a `Vec` and reports the
/// configured availability; embeddings use the trait defaults (no semantic recall in the double).
pub struct InMemoryStore {
    entries: Mutex<Vec<MemoryEntry>>,
    available: bool,
}

impl InMemoryStore {
    pub fn new(available: bool) -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
            available,
        }
    }
}

#[async_trait::async_trait]
impl MemoryStore for InMemoryStore {
    async fn save(&self, entry: MemoryEntry) -> AgentResult<()> {
        self.entries.lock().unwrap().push(entry);
        Ok(())
    }

    async fn search(&self, query: &str, limit: usize) -> AgentResult<Vec<MemoryEntry>> {
        let entries = self.entries.lock().unwrap();
        Ok(entries
            .iter()
            .filter(|e| e.matches_query(query))
            .take(limit)
            .cloned()
            .collect())
    }

    async fn list_by_kind(&self, kind: MemoryKind, limit: usize) -> AgentResult<Vec<MemoryEntry>> {
        let entries = self.entries.lock().unwrap();
        Ok(entries
            .iter()
            .filter(|e| e.kind == kind)
            .take(limit)
            .cloned()
            .collect())
    }

    async fn list_by_tag(&self, tag: &str, limit: usize) -> AgentResult<Vec<MemoryEntry>> {
        let entries = self.entries.lock().unwrap();
        Ok(entries
            .iter()
            .filter(|e| e.tags.contains(tag))
            .take(limit)
            .cloned()
            .collect())
    }

    fn is_available(&self) -> bool {
        self.available
    }
}

#[async_trait::async_trait]
impl SharedStore for InMemoryStore {
    async fn list_by_project(
        &self,
        project_id: &str,
        limit: usize,
    ) -> AgentResult<Vec<MemoryEntry>> {
        let entries = self.entries.lock().unwrap();
        Ok(entries
            .iter()
            .filter(|e| e.project_id.as_deref() == Some(project_id))
            .take(limit)
            .cloned()
            .collect())
    }
}

/// An `FsSandbox` rooted at the current directory with an empty sensitive matcher, for the memory tools'
/// `execute`/`confirmation` tests (which never touch the sandbox path, but the `Tool` API requires one).
pub(crate) fn sandbox() -> FsSandbox {
    FsSandbox::new(std::path::PathBuf::from("."), SensitiveMatcher::empty()).unwrap()
}

/// A `ToolCall` carrying the given JSON arguments.
pub fn call(arguments: &str) -> ToolCall {
    ToolCall {
        id: "c1".to_string(),
        kind: "function".to_string(),
        function: FunctionCall {
            name: "memory_tool".to_string(),
            arguments: arguments.to_string(),
        },
    }
}
