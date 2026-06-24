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

use crate::modules::tools::application::tool::Tool;

/// The default file tool set, in the order advertised to the model.
pub fn default_fs_tools() -> Vec<Box<dyn Tool>> {
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
        Box::new(run_command::RunCommand),
    ]
}
