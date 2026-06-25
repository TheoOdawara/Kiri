//! Shared test helpers for the memory tools: a real file+SQLite-backed `MemoryPort` over a temp dir,
//! and a `ToolCall` builder.

use std::sync::Arc;

use tempfile::TempDir;

use crate::modules::memory::application::memory_port::{MemoryPort, MemoryPortImpl};
use crate::modules::memory::domain::project_memory::{ProjectMemory, SharedMemory};
use crate::modules::memory::infrastructure::file_project_memory::FileProjectMemory;
use crate::modules::memory::infrastructure::file_project_store::FileProjectStore;
use crate::modules::memory::infrastructure::sqlite_shared_memory::SqliteSharedMemory;
use crate::modules::memory::infrastructure::sqlite_shared_store::SqliteSharedStore;
use crate::shared::kernel::tool_call::{FunctionCall, ToolCall};

/// A `MemoryPort` backed by real file (project) and SQLite (shared) stores under `dir`.
pub async fn temp_port(dir: &TempDir) -> Arc<dyn MemoryPort> {
    let project = FileProjectMemory::new(dir.path().join(".kiri").join("memory"));
    let project_ok = project.init().await.is_ok();
    let shared = SqliteSharedMemory::new(dir.path().join("shared.db")).unwrap();
    let shared_ok = shared.init().await.is_ok();
    Arc::new(MemoryPortImpl::new(
        FileProjectStore::new(project, project_ok),
        SqliteSharedStore::new(shared, shared_ok),
    ))
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
