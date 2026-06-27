//! Crate-level structural guards for the modular-hexagonal invariants (ADR 0003). These walk the source
//! tree and fail the build if a boundary is re-breached, so an architecture rule cannot silently rot.

#![cfg(test)]

use std::path::{Path, PathBuf};

/// Collect every `*.rs` file under `dir`, recursively.
fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read directory") {
        let path = entry.expect("directory entry").path();
        if path.is_dir() {
            rs_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// ADR 0017: `InputBuffer` (its home is `tui/domain/view_state.rs`) is the *only* sanctioned
/// `domain → UI-crate` coupling. Walk every `*.rs` under each `src/modules/*/domain/` recursively — so a
/// future nested `domain/<sub>/foo.rs` cannot silently re-breach — and assert none except that one file
/// imports `ratatui`/`tui_textarea`.
#[test]
fn only_input_buffer_couples_domain_to_ui_crates() {
    let modules = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("modules");

    let mut domain_files = Vec::new();
    for module in std::fs::read_dir(&modules).expect("read modules directory") {
        let domain = module.expect("module entry").path().join("domain");
        if domain.is_dir() {
            rs_files(&domain, &mut domain_files);
        }
    }
    assert!(
        !domain_files.is_empty(),
        "expected to find domain files under src/modules/*/domain"
    );

    // The single sanctioned exception (ADR 0017). When Wave 3 (TUIC-02) renames it to
    // `tui/domain/input_buffer.rs`, update this allow-list to the new path.
    let allowed = Path::new("tui").join("domain").join("view_state.rs");
    for file in &domain_files {
        let source = std::fs::read_to_string(file).expect("read domain file");
        // Match the bare crate-path tokens, not just the `use` forms, so a fully-qualified
        // `ratatui::Style` / `tui_textarea::TextArea` cannot re-breach without a `use`. The small
        // false-positive risk (a comment naming the crate) is fail-loud and easily reworded.
        let couples = source.contains("ratatui::")
            || source.contains("use ratatui")
            || source.contains("tui_textarea::")
            || source.contains("use tui_textarea");
        let is_allowed = file.ends_with(&allowed);
        assert!(
            !couples || is_allowed,
            "domain file {} imports a UI crate (ratatui/tui_textarea); only InputBuffer's home may (ADR 0017)",
            file.display()
        );
    }
}
