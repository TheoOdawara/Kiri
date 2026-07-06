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
