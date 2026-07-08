pub mod consult_docs;
pub mod recall_memory;
pub mod remember;

use std::sync::Arc;

use crate::modules::memory::application::memory_port::Memory;
use crate::modules::memory::infrastructure::docs_library::DocsLibrary;
use crate::modules::tools::application::tool::Tool;

/// The memory/docs tool set advertised to the model: recall and persist memory, and consult the
/// project docs as a fallback. Wired alongside the file tools in `app::wire`.
pub fn default_memory_tools(
    memory: Arc<dyn Memory>,
    docs: Arc<DocsLibrary>,
    project_id: String,
) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(recall_memory::RecallMemory::new(memory.clone())),
        Arc::new(remember::Remember::new(memory, project_id)),
        Arc::new(consult_docs::ConsultDocs::new(docs)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::memory::application::memory_port::Memory;
    use crate::modules::memory::domain::entry::MemoryEntry;
    use crate::modules::memory::domain::scope::Scope;
    use crate::shared::kernel::error::AgentResult;

    /// A no-op `Memory` double: the read-only guard only reads each tool's `name()`/`is_read_only()`, so
    /// no method is ever called — the impl just satisfies the port so `default_memory_tools` can be built.
    struct NoopMemory;

    #[async_trait::async_trait]
    impl Memory for NoopMemory {
        async fn recall_project(&self, _q: &str, _l: usize) -> AgentResult<Vec<MemoryEntry>> {
            Ok(Vec::new())
        }
        async fn recall_shared(&self, _q: &str, _l: usize) -> AgentResult<Vec<MemoryEntry>> {
            Ok(Vec::new())
        }
        async fn recall_batch(
            &self,
            _scope: Scope,
            _queries: &[String],
            _limit: usize,
        ) -> AgentResult<Vec<Vec<MemoryEntry>>> {
            Ok(Vec::new())
        }
        async fn remember_project(&self, _entry: MemoryEntry) -> AgentResult<()> {
            Ok(())
        }
        async fn remember_shared(&self, _entry: MemoryEntry) -> AgentResult<()> {
            Ok(())
        }
        fn project_memory_available(&self) -> bool {
            false
        }
        fn shared_memory_available(&self) -> bool {
            false
        }
    }

    #[test]
    fn read_only_memory_tools_are_exactly_the_known_safe_set() {
        // SEC-01 / ADR 0029 guard (see `tools/infrastructure/fs.rs`): lock the read-only surface a
        // headless subagent can hold. `recall_memory` and `consult_docs` read only harness-owned stores
        // (the memory DBs and `docs/`), never an agent-supplied fs path; `remember` is a write (default
        // `is_read_only() == false`) and must stay out. A new read-only memory tool trips this guard.
        let tools = default_memory_tools(
            Arc::new(NoopMemory),
            Arc::new(DocsLibrary::new(std::path::PathBuf::from("docs"))),
            "guard-project".to_string(),
        );
        let mut read_only: Vec<&str> = tools
            .iter()
            .filter(|tool| tool.is_read_only())
            .map(|tool| tool.name())
            .collect();
        read_only.sort_unstable();
        assert_eq!(read_only, ["consult_docs", "recall_memory"]);
    }
}
