use std::fs;
use std::io::Read;
use std::path::Path;

use crate::modules::tools::infrastructure::sandbox::{CreateResolution, Sandbox};

pub const READ_FILE_MAX_BYTES: usize = 64 * 1024;
pub const EDIT_FILE_MAX_BYTES: u64 = 4 * 1024 * 1024;
pub const SEARCH_FILE_MAX_BYTES: u64 = 1024 * 1024;
pub const SEARCH_MAX_MATCHES: usize = 100;
pub const SEARCH_MAX_LINE_CHARS: usize = 200;
pub const BINARY_SNIFF_BYTES: usize = 8 * 1024;

/// Read at most `cap` bytes from `path`, bounding allocation against very large files.
pub fn read_capped(path: &Path, cap: usize) -> std::io::Result<Vec<u8>> {
    let mut buffer = Vec::new();
    fs::File::open(path)?
        .take(cap as u64)
        .read_to_end(&mut buffer)?;
    Ok(buffer)
}

/// Scan one file for `query`, appending `relative:line: text` matches (capped) to `matches`. Skips
/// files over the size cap and NUL-containing (binary) files.
pub fn search_file(path: &Path, query: &str, root: &Path, matches: &mut Vec<String>) {
    match fs::metadata(path) {
        Ok(metadata) if metadata.len() > SEARCH_FILE_MAX_BYTES => return, // skip large files
        Ok(_) => {}
        Err(_) => return,
    }
    let Ok(bytes) = read_capped(path, SEARCH_FILE_MAX_BYTES as usize) else {
        return;
    };
    let sniff = &bytes[..bytes.len().min(BINARY_SNIFF_BYTES)];
    if sniff.contains(&0) {
        return; // treat NUL-containing files as binary and skip
    }
    let text = String::from_utf8_lossy(&bytes);
    let relative = path.strip_prefix(root).unwrap_or(path).to_string_lossy();
    for (number, line) in text.lines().enumerate() {
        if matches.len() >= SEARCH_MAX_MATCHES {
            return;
        }
        if line.contains(query) {
            let shown: String = line.chars().take(SEARCH_MAX_LINE_CHARS).collect();
            matches.push(format!("{relative}:{}: {shown}", number + 1));
        }
    }
}

/// Workspace-relative, comma-joined list of the directories a create/move would have to make.
pub fn missing_dirs_label(resolution: &CreateResolution, sandbox: &Sandbox) -> String {
    resolution
        .missing_dirs
        .iter()
        .map(|dir| {
            dir.strip_prefix(sandbox.root())
                .unwrap_or(dir)
                .display()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join(", ")
}
