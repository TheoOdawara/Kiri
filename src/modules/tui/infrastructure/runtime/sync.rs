//! The `/sync` push: the wire-injected [`SyncContext`] ports/paths and the push handler. The front-end
//! is not a composition root — it constructs no adapter and recomputes no path; `app::wire` chooses them.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use ratatui::DefaultTerminal;

use crate::modules::memory::application::shared_memory::SharedMemory;
use crate::modules::sync::application::git::Git;
use crate::modules::sync::application::sync_service::SyncService;
use crate::modules::sync::application::work_tree::SyncWorkTree;
use crate::modules::tui::domain::model::Model;

use super::render::draw_and_copy;

/// The sync ports + paths, built once in `app::wire` and injected into the front-end so a live `/sync`
/// push constructs **no** adapter and recomputes **no** path — the runtime is no longer a second
/// composition root. The concrete git/shared-memory/work-tree adapter *choice* lives only in `wire`;
/// here they are seen only as ports, and the home/config paths come from `Settings`.
pub struct SyncContext {
    git: Arc<dyn Git>,
    memory: Arc<dyn SharedMemory>,
    work_tree: Arc<dyn SyncWorkTree>,
    global_dir: PathBuf,
    config_path: PathBuf,
}

impl SyncContext {
    pub fn new(
        git: Arc<dyn Git>,
        memory: Arc<dyn SharedMemory>,
        work_tree: Arc<dyn SyncWorkTree>,
        global_dir: PathBuf,
        config_path: PathBuf,
    ) -> Self {
        Self {
            git,
            memory,
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
    let _ = draw_and_copy(terminal, model);

    let service = SyncService::new(
        ctx.git.as_ref(),
        ctx.global_dir.clone(),
        ctx.config_path.clone(),
        ctx.memory.as_ref(),
        ctx.work_tree.as_ref(),
    );
    match service.push().await {
        Ok(summary) => model.notify_info(format!("sync: {summary}")),
        Err(error) => model.notify_error(format!("sync falhou: {error}")),
    }
}
