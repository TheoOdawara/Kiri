pub mod create_dir;
pub mod delete_dir;
pub mod delete_file;
pub mod edit_file;
pub mod list_dir;
pub mod move_path;
pub mod read_file;
pub mod search;
pub mod write_file;

use std::sync::Arc;

use regex::Regex;

use crate::modules::tools::application::tool::Tool;
use crate::modules::tools::infrastructure::run_command::RunCommand;

/// The default file tool set, in the order advertised to the model. `RunCommand` is injected with the
/// plan-mode allow-list (safe inspection/build/test programs permitted in plan mode) and whether
/// confinement is required (`KIRI_SANDBOX=require` refuses `run_command` when no OS sandbox is available).
/// Network access is the sandbox's base stance only (ADR 0022) — no per-command widening. `Arc` (not
/// `Box`) so the same tool instances can be shared into a filtered child registry for a dispatched
/// subagent (ADR 0029) without rebuilding or double-connecting anything stateful (e.g. MCP proxies).
pub fn default_fs_tools(plan_allow: Arc<[Regex]>, require_confinement: bool) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(read_file::ReadFile),
        Arc::new(write_file::WriteFile),
        Arc::new(edit_file::EditFile),
        Arc::new(delete_file::DeleteFile),
        Arc::new(move_path::MovePath),
        Arc::new(list_dir::ListDir),
        Arc::new(create_dir::CreateDir),
        Arc::new(delete_dir::DeleteDir),
        Arc::new(search::Search),
        Arc::new(RunCommand::new(plan_allow, require_confinement)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_fs_tools_are_exactly_the_known_safe_set() {
        // SEC-01 / ADR 0029: a dispatched subagent's toolset is the read-only intersection of the parent
        // pool, and every read-only fs tool it can hold must self-gate an out-of-root target
        // (`default_accept_for`). This locks the read-only surface: a new fs tool that returns
        // `is_read_only() == true` trips this guard, forcing a conscious check that it self-gates before it
        // can silently reach a headless subagent.
        let tools = default_fs_tools(Arc::from([]), false);
        let mut read_only: Vec<&str> = tools
            .iter()
            .filter(|tool| tool.is_read_only())
            .map(|tool| tool.name())
            .collect();
        read_only.sort_unstable();
        assert_eq!(read_only, ["list_dir", "read_file", "search"]);
    }
}
