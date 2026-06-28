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
/// plan-mode allow-list (safe inspection/build/test programs permitted in plan mode), the network
/// allow-list (dev/package commands that may reach the network under confinement), and whether
/// confinement is required (`KIRI_SANDBOX=require` refuses `run_command` when no OS sandbox is available).
pub fn default_fs_tools(
    plan_allow: Arc<[Regex]>,
    net_allow: Arc<[Regex]>,
    require_confinement: bool,
) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(read_file::ReadFile),
        Box::new(write_file::WriteFile),
        Box::new(edit_file::EditFile),
        Box::new(delete_file::DeleteFile),
        Box::new(move_path::MovePath),
        Box::new(list_dir::ListDir),
        Box::new(create_dir::CreateDir),
        Box::new(delete_dir::DeleteDir),
        Box::new(search::Search),
        Box::new(RunCommand::new(plan_allow, net_allow, require_confinement)),
    ]
}
