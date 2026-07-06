use std::borrow::Cow;
use std::fs::Metadata;
use std::path::Path;

use tokio::io::AsyncReadExt;

use crate::modules::tools::application::sandbox::{CreateResolution, Sandbox};
use crate::modules::tools::application::tool::ToolOutcome;
use crate::modules::tools::infrastructure::exec;

pub const READ_FILE_MAX_BYTES: usize = 64 * 1024;
pub const EDIT_FILE_MAX_BYTES: u64 = 4 * 1024 * 1024;
pub const SEARCH_MAX_MATCHES: usize = 100;
pub const SEARCH_MAX_LINE_CHARS: usize = 200;
pub const SEARCH_FILE_MAX_BYTES: u64 = 1024 * 1024;
pub const BINARY_SNIFF_BYTES: usize = 8 * 1024;

/// Read at most `cap` bytes from `path`, bounding allocation against very large files. Bounded by
/// `DEFAULT_TIMEOUT`: a wedged/stale mount must fail fast rather than stall the runtime.
pub async fn read_capped(path: &Path, cap: usize) -> std::io::Result<Vec<u8>> {
    let read = async {
        let mut buffer = Vec::new();
        tokio::fs::File::open(path)
            .await?
            .take(cap as u64)
            .read_to_end(&mut buffer)
            .await?;
        Ok(buffer)
    };
    match tokio::time::timeout(exec::DEFAULT_TIMEOUT, read).await {
        Ok(result) => result,
        Err(_) => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "read: timed out",
        )),
    }
}

/// Scan one file for `query`, appending `relative:line: text` matches (capped) to `matches`. Skips
/// files over the size cap and NUL-containing (binary) files. Callers bound the enclosing walk by
/// `DEFAULT_TIMEOUT`; a per-file stat/read hang is caught there too.
pub async fn search_file(path: &Path, query: &str, root: &Path, matches: &mut Vec<String>) {
    match tokio::fs::metadata(path).await {
        Ok(metadata) if metadata.len() > SEARCH_FILE_MAX_BYTES => return, // skip large files
        Ok(_) => {}
        Err(_) => return,
    }
    let Ok(bytes) = read_capped(path, SEARCH_FILE_MAX_BYTES as usize).await else {
        return;
    };
    let sniff = &bytes[..bytes.len().min(BINARY_SNIFF_BYTES)];
    if sniff.contains(&0) {
        return; // treat NUL-containing files as binary and skip
    }
    let text = String::from_utf8_lossy(&bytes);
    let relative = relative_display(path.strip_prefix(root).unwrap_or(path));
    for (number, line) in text.lines().enumerate() {
        if matches.len() >= SEARCH_MAX_MATCHES {
            return;
        }
        if line.contains(query) {
            // Common case: a line short enough by byte length that no char-truncation is possible is
            // shown borrowed, with no per-line allocation. Only a long line pays to char-truncate.
            let shown: Cow<str> = if line.len() <= SEARCH_MAX_LINE_CHARS {
                Cow::Borrowed(line)
            } else {
                Cow::Owned(line.chars().take(SEARCH_MAX_LINE_CHARS).collect())
            };
            matches.push(format!("{relative}:{}: {shown}", number + 1));
        }
    }
}

/// Workspace-relative, comma-joined list of the directories a create/move would have to make.
pub fn missing_dirs_label(resolution: &CreateResolution, sandbox: &dyn Sandbox) -> String {
    resolution
        .missing_dirs
        .iter()
        .map(|dir| relative_display(dir.strip_prefix(sandbox.root()).unwrap_or(dir)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Render a workspace-relative path with forward slashes, so the user-facing text and the frozen
/// characterization snapshot stay byte-identical across platforms (Windows otherwise yields `\`).
fn relative_display(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

/// Create the missing parent directories of a create/move target, mapping a mkdir failure to the
/// shared error outcome. Call before writing/renaming when the resolution reported missing dirs;
/// `path` is the user-facing path interpolated into the error. Bounded by `DEFAULT_TIMEOUT`.
pub async fn ensure_parent_dirs(
    resolution: &CreateResolution,
    path: &str,
) -> Result<(), ToolOutcome> {
    if resolution.missing_dirs.is_empty() {
        return Ok(());
    }
    let Some(parent) = resolution.target.parent() else {
        return Ok(());
    };
    match tokio::time::timeout(exec::DEFAULT_TIMEOUT, tokio::fs::create_dir_all(parent)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(ToolOutcome::Error(format!(
            "cannot create directories for {path}: {error}"
        ))),
        // `tokio::fs` runs on the blocking pool and can't be cancelled once dispatched: the mkdir may
        // still land after this timeout is reported (issue #53, security-debt).
        Err(_) => Err(ToolOutcome::Error(format!(
            "cannot create directories for {path}: timed out (it may still complete in the background)"
        ))),
    }
}

/// Stat `path` once as a pre-flight guard. `reject` inspects the metadata and returns `Some(error)` to
/// veto the operation; a stat failure maps to the shared `cannot stat` error. `label` is the
/// user-facing path interpolated into both messages.
pub async fn stat_guard(
    path: &Path,
    label: &str,
    reject: impl FnOnce(&Metadata) -> Option<String>,
) -> Result<(), ToolOutcome> {
    // Bound the stat: on a wedged/stale network mount `metadata` can hang forever, and the contract
    // requires every blocking await to fail fast rather than stall the runtime.
    let metadata = match tokio::time::timeout(exec::DEFAULT_TIMEOUT, tokio::fs::metadata(path))
        .await
    {
        Ok(Ok(metadata)) => metadata,
        Ok(Err(error)) => return Err(ToolOutcome::Error(format!("cannot stat {label}: {error}"))),
        Err(_) => {
            return Err(ToolOutcome::Error(format!(
                "cannot stat {label}: timed out"
            )));
        }
    };
    match reject(&metadata) {
        Some(error) => Err(ToolOutcome::Error(error)),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::{SEARCH_MAX_LINE_CHARS, search_file};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_file(tag: &str, contents: &[u8]) -> PathBuf {
        let mut path = std::env::temp_dir();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        path.push(format!("t-cli-support-{}-{n}-{tag}", std::process::id()));
        fs::write(&path, contents).unwrap();
        path
    }

    #[tokio::test]
    async fn long_multibyte_line_truncates_at_a_char_boundary() {
        // 300 two-byte chars (600 bytes): the byte-length fast path is skipped, and truncation must
        // cut at a char boundary, never mid-codepoint.
        let line = "é".repeat(300);
        let file = temp_file("multibyte", line.as_bytes());
        let root = file.parent().unwrap().to_path_buf();
        let mut matches = Vec::new();
        search_file(&file, "é", &root, &mut matches).await;
        let _ = fs::remove_file(&file);

        assert_eq!(matches.len(), 1);
        let shown = matches[0].rsplit_once(": ").unwrap().1;
        assert_eq!(shown.chars().count(), SEARCH_MAX_LINE_CHARS);
        assert!(shown.chars().all(|c| c == 'é'));
    }

    #[tokio::test]
    async fn short_multibyte_line_is_returned_whole() {
        let file = temp_file("short", "héllo wörld".as_bytes());
        let root = file.parent().unwrap().to_path_buf();
        let mut matches = Vec::new();
        search_file(&file, "wörld", &root, &mut matches).await;
        let _ = fs::remove_file(&file);

        assert_eq!(matches.len(), 1);
        assert!(matches[0].ends_with("héllo wörld"));
    }

    #[tokio::test]
    async fn read_capped_reports_the_underlying_error() {
        // A real hang (the case DEFAULT_TIMEOUT bounds) isn't reproducible in a fast, portable unit
        // test; this covers that the async rewrite still surfaces a plain open() failure as Err,
        // same as the previous sync implementation did.
        let dir = std::env::temp_dir();
        let result = super::read_capped(&dir, 1024).await;
        assert!(
            result.is_err(),
            "expected an error reading a directory as a file"
        );
    }
}
