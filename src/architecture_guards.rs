//! Crate-level structural guards for the modular-hexagonal domain-purity invariants (ADR 0003). These
//! walk the source tree and fail the build if a `domain` layer re-breaches its purity — coupling to a
//! UI crate (ADR 0017) or doing fs/net/db I/O — so those rules cannot silently rot. The inward
//! import-direction rule (application/domain must not import infrastructure) is a convention these guards
//! do NOT yet enforce; do not claim it here until a guard backs it.

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

/// Every `*.rs` under each `src/modules/*/domain/`, recursively — so a future nested
/// `domain/<sub>/foo.rs` cannot silently escape either domain-purity guard.
fn domain_files() -> Vec<PathBuf> {
    let modules = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("modules");
    let mut files = Vec::new();
    for module in std::fs::read_dir(&modules).expect("read modules directory") {
        let domain = module.expect("module entry").path().join("domain");
        if domain.is_dir() {
            rs_files(&domain, &mut files);
        }
    }
    assert!(
        !files.is_empty(),
        "expected to find domain files under src/modules/*/domain"
    );
    files
}

/// ADR 0017: `InputBuffer` (its home is `tui/domain/input_buffer.rs`) is the *only* sanctioned
/// `domain → UI-crate` coupling. Walk every `*.rs` under each `src/modules/*/domain/` recursively — so a
/// future nested `domain/<sub>/foo.rs` cannot silently re-breach — and assert none except that one file
/// imports `ratatui`/`tui_textarea`.
#[test]
fn only_input_buffer_couples_domain_to_ui_crates() {
    // The single sanctioned exception (ADR 0017): `InputBuffer`'s home, `tui/domain/input_buffer.rs`.
    let allowed = Path::new("tui").join("domain").join("input_buffer.rs");
    for file in &domain_files() {
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

/// ADR 0020: the untrusted-project isolation invariant — a `.env` may be loaded ONLY from the trusted
/// `~/.kiri` dir, never the cwd. The argless `dotenvy::dotenv()` reads a `.env` from the current working
/// directory (a hostile repo the user `cd`s into), which would let it inject security-relevant env
/// (`KIRI_SANDBOX*`, `KIRI_PATH`, `*_API_KEY`, …). Fail the build if that cwd variant reappears anywhere
/// under `src/`; the sanctioned path is `dotenvy::from_path(~/.kiri/.env)` in `config::load_global_env`.
#[test]
fn no_cwd_dotenv_load() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rs_files(&src, &mut files);
    // Build the needle by concatenation so this guard's own literal does not self-match.
    let needle = concat!("dotenvy::", "dotenv(");
    for file in &files {
        if file.ends_with("architecture_guards.rs") {
            continue;
        }
        let source = std::fs::read_to_string(file).expect("read src file");
        assert!(
            !source.contains(needle),
            "{} loads a `.env` from the cwd; use dotenvy::from_path(~/.kiri/.env) (ADR 0020)",
            file.display()
        );
    }
}

/// ADR 0003: `domain` is pure data + rules — no filesystem, network, or database I/O. Walk every
/// `*.rs` under each `src/modules/*/domain/` and assert none references an I/O facility (`std::fs`,
/// `tokio::fs`, `std::net`, `reqwest`, `rusqlite`). There is NO exception: unlike the UI-crate coupling
/// (which `InputBuffer` is sanctioned for), no domain file may do I/O. Closes ARCH-03: the prior guard
/// checked only domain→UI, so a domain file could `use std::fs` and the build stayed green.
#[test]
fn domain_is_free_of_io() {
    for file in &domain_files() {
        let source = std::fs::read_to_string(file).expect("read domain file");
        // Match the bare crate-path tokens (not just `use` forms) so a fully-qualified call cannot slip
        // through. The small false-positive risk (a token named in a comment) is fail-loud and reworded.
        for needle in [
            "std::fs",
            "tokio::fs",
            "std::net",
            "reqwest::",
            "use reqwest",
            "rusqlite::",
            "use rusqlite",
        ] {
            assert!(
                !source.contains(needle),
                "domain file {} references I/O ({needle:?}); domain must be pure (ADR 0003)",
                file.display()
            );
        }
    }
}
