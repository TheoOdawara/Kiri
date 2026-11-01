use std::path::PathBuf;

pub(crate) fn home_directory() -> Option<PathBuf> {
    std::env::home_dir()
}
