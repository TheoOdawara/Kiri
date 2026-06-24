pub mod create_dir;
pub mod delete_dir;
pub mod delete_file;
pub mod edit_file;
pub mod list_dir;
pub mod move_path;
pub mod read_file;
pub mod run_command;
pub mod search;
pub mod write_file;

use std::sync::Arc;

use regex::Regex;

use crate::modules::tools::application::tool::Tool;

/// The default file tool set, in the order advertised to the model. The plan blacklist is
/// injected into `RunCommand` so it can refuse destructive shell commands in plan mode.
pub fn default_fs_tools(plan_blacklist: Arc<[Regex]>) -> Vec<Box<dyn Tool>> {
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
        Box::new(run_command::RunCommand::new(plan_blacklist)),
    ]
}
