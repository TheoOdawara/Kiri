//! The `/sync` push: the wire-injected [`SyncContext`] ports/paths and the push handler. The front-end
//! is not a composition root — it constructs no adapter and recomputes no path; `app::wire` chooses them.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use ratatui::DefaultTerminal;

use crate::modules::memory::application::shared_memory::SharedMemory;
use crate::modules::sync::application::git::Git;
use crate::modules::sync::application::sync_service::SyncService;
use crate::modules::sync::application::work_tree::SyncWorkTree;
use crate::modules::sync::infrastructure::memory_ndjson::NdjsonMemoryExchange;
use crate::modules::tui::domain::model::Model;
use crate::shared::kernel::error::AgentResult;

use super::render::draw_and_copy;

/// Open the sync shared store on demand, returning the store plus an optional degraded-mode warning the
/// caller surfaces on its own channel — so the open never creates an empty `shared.db` at boot (only on
/// the first `/sync`) and a failure is never swallowed nor `eprintln!`'d into the live TUI. The concrete
/// adapter *choice* lives only in `app::wire`; here it is seen as a port.
pub type SharedMemoryFactory = Arc<
    dyn Fn() -> Pin<
            Box<dyn Future<Output = AgentResult<(Arc<dyn SharedMemory>, Option<String>)>> + Send>,
        > + Send
        + Sync,
>;

/// The sync ports + paths, built once in `app::wire` and injected into the front-end so a live `/sync`
/// push constructs **no** adapter and recomputes **no** path — the runtime is no longer a second
/// composition root. The concrete git/shared-memory/work-tree adapter *choice* lives only in `wire`;
/// here they are seen only as ports, and the home/config paths come from `Settings`. The shared store is
/// a *factory* opened lazily on the first push, so a memory-off session that never syncs births no
/// `shared.db`.
pub struct SyncContext {
    git: Arc<dyn Git>,
    memory_factory: SharedMemoryFactory,
    work_tree: Arc<dyn SyncWorkTree>,
    global_dir: PathBuf,
    config_path: PathBuf,
}

impl SyncContext {
    pub fn new(
        git: Arc<dyn Git>,
        memory_factory: SharedMemoryFactory,
        work_tree: Arc<dyn SyncWorkTree>,
        global_dir: PathBuf,
        config_path: PathBuf,
    ) -> Self {
        Self {
            git,
            memory_factory,
            work_tree,
            global_dir,
            config_path,
        }
    }
}

/// Push the portable profile (config + shared memory) to the configured private repo via `/sync`. Shows
/// a "syncing" notice and draws it before the (network-bound, timeout-bounded) push, then reports the
/// result. Constructs no adapter and recomputes no path — the ports and the home/config paths come from
/// the wire-built [`SyncContext`], so the front-end is not a composition root.
pub(super) async fn sync_push(
    ctx: &SyncContext,
    model: &mut Model,
    terminal: &mut DefaultTerminal,
) {
    model.notify_info("sincronizando (push)…");
    model.timeline.render_at = Some(Instant::now());
    // Best-effort pre-op repaint to show the "syncing…" notice before the blocking push; the main loop
    // redraws on its next iteration, so a failed draw here must not block the sync.
    let _ = draw_and_copy(terminal, model);

    // Open the shared store on demand (first `/sync` of the session); a degraded-mode fallback surfaces
    // its reason as a notice, and a hard open failure aborts the push with a clear message.
    let (memory, warning) = match (ctx.memory_factory)().await {
        Ok(opened) => opened,
        Err(error) => {
            model.notify_error(format!("sync falhou: {error}"));
            return;
        }
    };
    if let Some(reason) = warning {
        model.notify_error(format!("sync: {reason}"));
    }

    let exchange = NdjsonMemoryExchange::new(memory.as_ref());
    let service = SyncService::new(
        ctx.git.as_ref(),
        ctx.global_dir.clone(),
        ctx.config_path.clone(),
        &exchange,
        ctx.work_tree.as_ref(),
    );
    match service.push().await {
        Ok(summary) => model.notify_info(format!("sync: {summary}")),
        Err(error) => model.notify_error(format!("sync falhou: {error}")),
    }
}
