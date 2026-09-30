//! Display-text helpers: terminal cell width and a friendly workspace path.

use std::path::Path;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// The number of terminal cells `text` occupies, counting wide glyphs (brand seals, record dots,
/// box-drawing) as their true width. The layout math uses this instead of `chars().count()` so the
/// caps and badges never get pushed off-screen by a 2-cell glyph counted as one.
pub fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// Split `text` into chunks each at most `width` display cells wide, breaking between characters.
/// Used to hard-wrap a word longer than the available width without ever cutting through a 2-cell
/// glyph or overflowing the column. A single character wider than `width` still takes its own chunk.
pub fn chunk_by_width(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut cols = 0;
    for ch in text.chars() {
        let ch_cols = UnicodeWidthChar::width(ch).unwrap_or(0);
        if cols + ch_cols > width && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
            cols = 0;
        }
        current.push(ch);
        cols += ch_cols;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// Greedy word-wrap: split `line` on single spaces and pack words onto rows up to `width` display cells
/// (one space between words), hard-splitting any word wider than `width` by display cells. The single
/// source for the transcript and editor wrappers, both measuring with [`display_width`] so a wide glyph
/// wraps identically in each — the editor previously counted `chars()`, under-wrapping wide glyphs.
pub fn greedy_wrap(line: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut current = String::new();
    for word in line.split(' ') {
        let word_cols = display_width(word);
        if current.is_empty() {
            if word_cols <= width {
                current.push_str(word);
            } else {
                // The word alone exceeds the width: hard-split it by display cells.
                rows.extend(chunk_by_width(word, width));
            }
        } else if display_width(&current) + 1 + word_cols <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            rows.push(std::mem::take(&mut current));
            if word_cols <= width {
                current.push_str(word);
            } else {
                rows.extend(chunk_by_width(word, width));
            }
        }
    }
    if !current.is_empty() {
        rows.push(current);
    }
    rows
}

/// A friendly, display-only rendering of a workspace root. `std::fs::canonicalize` adds the Windows
/// verbatim prefix (`\\?\C:\…`, or `\\?\UNC\server\share` for shares); strip it and normalize
/// separators to `/` so the meta rule shows `C:/work` rather than `\\?\C:\work`. On non-Windows the
/// path has neither, so it is returned unchanged.
pub fn display_path(root: &Path) -> String {
    let raw = root.display().to_string();
    let stripped = if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = raw.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        raw
    };
    stripped.replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn display_width_counts_wide_glyphs() {
        assert_eq!(display_width("abc"), 3);
        // Wide (CJK) glyphs occupy two cells each — not undercounted as one like `chars().count()`.
        assert_eq!(display_width("世界"), 4);
    }

    #[test]
    fn chunk_by_width_splits_on_display_cells_not_chars() {
        assert_eq!(chunk_by_width("abcde", 2), vec!["ab", "cd", "e"]);
        // Each CJK glyph is 2 cells, so only one fits per 2-cell chunk — char count would wrongly fit two.
        assert_eq!(chunk_by_width("世界世", 2), vec!["世", "界", "世"]);
        // A glyph wider than the width still gets its own chunk rather than being dropped or looping.
        assert_eq!(chunk_by_width("世", 1), vec!["世"]);
    }

    #[test]
    fn word_wrap_measures_display_width() {
        // Each "世界" is 4 display cells (2 chars). At width 5 only one fits per row, so a wide-glyph
        // line wraps to one word per row — a `chars().count()` metric would wrongly pack two.
        assert_eq!(
            greedy_wrap("世界 世界 世界", 5),
            vec!["世界", "世界", "世界"]
        );
        // ASCII words pack greedily up to the width.
        assert_eq!(greedy_wrap("aa bb cc", 4), vec!["aa", "bb", "cc"]);
        // A word wider than the row is hard-split by display cells.
        assert_eq!(greedy_wrap("verylongword", 4), vec!["very", "long", "word"]);
    }

    #[test]
    fn display_path_strips_windows_verbatim_prefix() {
        assert_eq!(
            display_path(&PathBuf::from(r"\\?\C:\work\kiri")),
            "C:/work/kiri"
        );
        assert_eq!(
            display_path(&PathBuf::from(r"\\?\UNC\server\share\dir")),
            "//server/share/dir"
        );
    }

    #[test]
    fn display_path_leaves_plain_paths_untouched() {
        assert_eq!(
            display_path(&PathBuf::from("/home/user/work")),
            "/home/user/work"
        );
    }
}
