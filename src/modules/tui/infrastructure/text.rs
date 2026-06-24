//! Display-text helpers: terminal cell width and a friendly workspace path.

use std::path::Path;

use unicode_width::UnicodeWidthStr;

/// The number of terminal cells `text` occupies, counting wide glyphs (brand seals, record dots,
/// box-drawing) as their true width. The layout math uses this instead of `chars().count()` so the
/// caps and badges never get pushed off-screen by a 2-cell glyph counted as one.
pub fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
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
