use std::path::PathBuf;

use tokio::fs;

use crate::shared::kernel::error::AgentResult;

/// The docs tree is a fallback knowledge source, not a search engine — these bounds protect the context
/// window and the runtime.
const MAX_FILES_SCANNED: usize = 500;
const MAX_FILE_BYTES: usize = 256 * 1024;
const EXCERPT_RADIUS: usize = 200;

/// A relevant slice of a documentation file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocMatch {
    /// Relative to the docs root, for display and re-reading.
    pub path: String,
    pub excerpt: String,
    /// Total term occurrences in the file — higher ranks first.
    pub score: usize,
}

/// Read-only access to the project's documentation tree, consulted as a fallback when memory does not
/// cover a question.
pub struct DocsLibrary {
    root: PathBuf,
}

impl DocsLibrary {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn is_available(&self) -> bool {
        self.root.is_dir()
    }

    /// An absent or empty docs tree, or a blank query, yields an empty result rather than an error.
    pub async fn search(&self, query: &str, limit: usize) -> AgentResult<Vec<DocMatch>> {
        let terms: Vec<String> = query
            .split_whitespace()
            .map(|t| t.to_lowercase())
            .filter(|t| !t.is_empty())
            .collect();
        if terms.is_empty() || limit == 0 || !self.is_available() {
            return Ok(Vec::new());
        }

        let files = self.collect_markdown_files().await?;
        let mut matches = Vec::new();
        for path in files {
            let Ok(bytes) = fs::read(&path).await else {
                continue;
            };
            let content = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_FILE_BYTES)]);
            if let Some(found) = score_content(&content, &terms) {
                // Forward slashes on every platform: the model re-reads this path via `read_file`, whose
                // workspace-relative paths are Unix-style regardless of host OS.
                let rel = path
                    .strip_prefix(&self.root)
                    .unwrap_or(path.as_path())
                    .to_string_lossy()
                    .replace('\\', "/");
                matches.push(DocMatch {
                    path: rel,
                    excerpt: found.excerpt,
                    score: found.score,
                });
            }
        }

        matches.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.path.cmp(&b.path)));
        matches.truncate(limit);
        Ok(matches)
    }

    async fn collect_markdown_files(&self) -> AgentResult<Vec<PathBuf>> {
        let mut files = Vec::new();
        let mut dirs = vec![self.root.clone()];
        while let Some(dir) = dirs.pop() {
            if files.len() >= MAX_FILES_SCANNED {
                break;
            }
            let mut reader = match fs::read_dir(&dir).await {
                Ok(reader) => reader,
                Err(_) => continue,
            };
            while let Some(entry) = reader.next_entry().await? {
                let path = entry.path();
                // `file_type` does not traverse a symlink, so `consult_docs` cannot follow one out of the
                // docs root.
                let file_type = entry.file_type().await?;
                if file_type.is_symlink() {
                    continue;
                } else if file_type.is_dir() {
                    dirs.push(path);
                } else if is_markdown(&path) {
                    files.push(path);
                    if files.len() >= MAX_FILES_SCANNED {
                        break;
                    }
                }
            }
        }
        Ok(files)
    }
}

fn is_markdown(path: &std::path::Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("md") | Some("markdown")
    )
}

struct Scored {
    excerpt: String,
    score: usize,
}

/// `None` when no term occurs.
fn score_content(content: &str, terms: &[String]) -> Option<Scored> {
    let lower = content.to_lowercase();
    let mut score = 0;
    let mut first_match: Option<usize> = None;
    for term in terms {
        let mut from = 0;
        while let Some(pos) = lower[from..].find(term.as_str()) {
            let at = from + pos;
            score += 1;
            first_match = Some(first_match.map_or(at, |m| m.min(at)));
            from = at + term.len();
        }
    }
    let at = first_match?;
    Some(Scored {
        excerpt: excerpt_around(content, at),
        score,
    })
}

/// Snapped to char boundaries, so multi-byte text never panics.
fn excerpt_around(content: &str, at: usize) -> String {
    let start = floor_char_boundary(content, at.saturating_sub(EXCERPT_RADIUS));
    let end = ceil_char_boundary(content, (at + EXCERPT_RADIUS).min(content.len()));
    let slice = content[start..end].trim();
    let collapsed = slice.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut excerpt = String::new();
    if start > 0 {
        excerpt.push('…');
    }
    excerpt.push_str(&collapsed);
    if end < content.len() {
        excerpt.push('…');
    }
    excerpt
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_char_boundary(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn write(dir: &std::path::Path, rel: &str, content: &str) {
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).await.unwrap();
        fs::write(path, content).await.unwrap();
    }

    #[tokio::test]
    async fn missing_root_is_unavailable_and_empty() {
        let dir = TempDir::new().unwrap();
        let docs = DocsLibrary::new(dir.path().join("docs"));
        assert!(!docs.is_available());
        assert!(docs.search("anything", 5).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn ranks_files_by_term_occurrence() {
        let dir = TempDir::new().unwrap();
        let docs_root = dir.path().join("docs");
        write(
            &docs_root,
            "decisions/0003-arch.md",
            "# Architecture\nThe architecture is modular hexagonal. Architecture matters.",
        )
        .await;
        write(
            &docs_root,
            "intro.md",
            "A short intro mentioning architecture once.",
        )
        .await;
        write(&docs_root, "unrelated.md", "Nothing relevant here.").await;

        let docs = DocsLibrary::new(docs_root);
        assert!(docs.is_available());

        let hits = docs.search("architecture", 10).await.unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].path, "decisions/0003-arch.md");
        assert!(hits[0].score >= hits[1].score);
        assert!(hits[0].excerpt.to_lowercase().contains("architecture"));
    }

    #[tokio::test]
    async fn blank_query_returns_nothing() {
        let dir = TempDir::new().unwrap();
        let docs_root = dir.path().join("docs");
        write(&docs_root, "a.md", "content").await;
        let docs = DocsLibrary::new(docs_root);
        assert!(docs.search("   ", 5).await.unwrap().is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn docs_walk_skips_symlinks() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let docs_root = dir.path().join("docs");
        write(&docs_root, "real.md", "architecture lives in this doc").await;

        // A secret outside the docs root, reachable only via a symlink placed inside it.
        let outside = dir.path().join("outside");
        write(
            &outside,
            "secret.md",
            "architecture secret outside the docs root",
        )
        .await;
        symlink(&outside, docs_root.join("linked")).unwrap();

        let docs = DocsLibrary::new(docs_root);
        let hits = docs.search("architecture", 10).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "real.md");
    }
}
