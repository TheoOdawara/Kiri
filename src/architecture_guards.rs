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

/// Recursively flatten a `use` tree into every concrete import path it names, as segment lists (e.g.
/// `use tokio::{process::Command as Cmd, time};` yields `[["tokio","process","Command"], ["tokio","time"]]`).
/// Deliberately keeps the PRE-rename identifier for `UseTree::Rename` (`as` only changes the local
/// binding, never what the import actually points at), so `use std::process::Command as Cmd;` is recorded
/// as `std::process::Command`, not `std::process::Cmd` — an alias cannot hide the real path from the
/// banned-prefix check below. A glob (`use tokio::process::*;`) records the globbed-into prefix itself,
/// since the import of the module is the violation regardless of which items are pulled in.
fn flatten_use_tree(tree: &syn::UseTree, prefix: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
    match tree {
        syn::UseTree::Path(p) => {
            prefix.push(p.ident.to_string());
            flatten_use_tree(&p.tree, prefix, out);
            prefix.pop();
        }
        syn::UseTree::Name(n) => {
            let mut full = prefix.clone();
            full.push(n.ident.to_string());
            out.push(full);
        }
        syn::UseTree::Rename(r) => {
            let mut full = prefix.clone();
            full.push(r.ident.to_string());
            out.push(full);
        }
        syn::UseTree::Glob(_) => out.push(prefix.clone()),
        syn::UseTree::Group(g) => {
            for item in &g.items {
                flatten_use_tree(item, prefix, out);
            }
        }
    }
}

/// Walks a parsed file's `use` imports (via [`flatten_use_tree`]) AND every `syn::Path` occurring anywhere
/// else (function calls, types, …), recording every one whose leading segments match a banned prefix.
/// Two independent checks are needed because a boundary can be crossed two ways: an imported item used
/// under a local (possibly renamed) binding — caught by the flattened `use` check — or a fully-qualified
/// reference with no `use` at all, e.g. `tokio::process::Command::new(...)` inline — caught by `visit_path`,
/// which `syn`'s default recursion reaches for every path in the file, included nested `mod` blocks.
/// Operates on the real AST, so whitespace/formatting inside a path, or a literal appearing only in a
/// comment or string, cannot produce a match — closing the two concrete false-positive/false-negative
/// modes a plain substring grep had (issue #50).
///
/// Two residual, undefeated gaps (documented, not silently claimed as closed):
///
/// - **Macro expansion.** `syn::parse_file` does not expand macros. A `macro_rules!` DEFINITION's body is
///   arbitrary token soup, not necessarily valid Rust syntax on its own, so this cannot see a banned path
///   hidden inside one; the same is true of any invocation of an external macro that itself expands to a
///   banned call. Full resistance would need actual macro expansion (e.g. `cargo expand`), a materially
///   larger undertaking than this guard warrants — no file in this codebase currently defines a hooks/mcp-
///   context macro, so this is theoretical, not a live gap.
/// - **Cross-file re-export.** This walks one file at a time, per `assert_process_io_confined`'s loop,
///   skipping only `infrastructure/`. If an `infrastructure/` file re-exported a banned path under an
///   innocuous local name (`pub use tokio::process::Command as SafeName;`), an `application/`-layer file
///   importing THAT re-export (`use crate::modules::hooks::infrastructure::x::SafeName;`) would flatten to
///   local module segments only — never `tokio`/`std`/`rmcp` — and would not be flagged, even though the
///   boundary is genuinely crossed. Closing this needs cross-file symbol resolution (effectively a mini
///   name-resolution pass), disproportionate for this guard; no such re-export exists in this codebase
///   today, so this is also theoretical, not a live gap.
struct BannedPathVisitor {
    /// Two-segment prefixes, e.g. `("tokio", "process")`.
    banned_pairs: &'static [(&'static str, &'static str)],
    /// One-segment prefixes, e.g. `"rmcp"`.
    banned_roots: &'static [&'static str],
    findings: Vec<String>,
}

impl BannedPathVisitor {
    fn check(&mut self, segments: &[String], context: &str) {
        if segments.len() >= 2 {
            for (a, b) in self.banned_pairs {
                if segments[0] == *a && segments[1] == *b {
                    self.findings.push(format!("{context} references {a}::{b}"));
                }
            }
        }
        if let Some(first) = segments.first() {
            for root in self.banned_roots {
                if first == root {
                    self.findings.push(format!("{context} references {root}"));
                }
            }
        }
    }
}

impl<'ast> syn::visit::Visit<'ast> for BannedPathVisitor {
    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        let mut flattened = Vec::new();
        flatten_use_tree(&node.tree, &mut Vec::new(), &mut flattened);
        for segments in &flattened {
            self.check(segments, "a `use` import");
        }
        syn::visit::visit_item_use(self, node);
    }

    fn visit_path(&mut self, node: &'ast syn::Path) {
        let segments: Vec<String> = node.segments.iter().map(|s| s.ident.to_string()).collect();
        self.check(&segments, "a fully-qualified path");
        syn::visit::visit_path(self, node);
    }
}

/// Assert every non-infrastructure `*.rs` under `context_root` is free of every banned path, via
/// [`BannedPathVisitor`]. Shared by the `hooks` and `mcp` guards below (ADR 0021).
fn assert_process_io_confined(
    context_root: &Path,
    banned_pairs: &'static [(&'static str, &'static str)],
    banned_roots: &'static [&'static str],
    adr_note: &str,
) {
    assert!(
        context_root.is_dir(),
        "expected the bounded context at {}",
        context_root.display()
    );
    let mut files = Vec::new();
    rs_files(context_root, &mut files);
    for file in &files {
        if file.components().any(|c| c.as_os_str() == "infrastructure") {
            continue;
        }
        let source = std::fs::read_to_string(file).expect("read context file");
        let parsed = syn::parse_file(&source)
            .unwrap_or_else(|error| panic!("parse {}: {error}", file.display()));
        let mut visitor = BannedPathVisitor {
            banned_pairs,
            banned_roots,
            findings: Vec::new(),
        };
        syn::visit::visit_file(&mut visitor, &parsed);
        assert!(
            visitor.findings.is_empty(),
            "{} must not touch process/protocol I/O directly ({adr_note}): {:?}",
            file.display(),
            visitor.findings
        );
    }
}

/// ADR 0021: the `hooks` context's process I/O is confined to `hooks/infrastructure/` — a hook's shell
/// command must run only through the sanctioned adapter (`ShellHookRunner`, over the sandbox's existing
/// confined-exec surface), never a raw process spawn from `hooks/application/`. Mirrors
/// `domain_is_free_of_io`'s shape but for a different boundary within the same context. AST-based (issue
/// #50): resistant to an aliased import (`use tokio::process::Command as Cmd;`) or an inline
/// fully-qualified reference with no `use` at all — see [`BannedPathVisitor`] for exactly what this does
/// and does not defeat.
#[test]
fn hooks_process_io_confined_to_infrastructure() {
    let hooks_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("modules")
        .join("hooks");
    assert_process_io_confined(
        &hooks_root,
        &[("tokio", "process"), ("std", "process")],
        &[],
        "ADR 0021 — process I/O must stay in hooks/infrastructure/",
    );
}

/// ADR 0021: the `mcp` context's process/network I/O is confined to `mcp/infrastructure/` — spawning an
/// MCP server or speaking the protocol over its stdio must run only through the sanctioned adapter
/// (`RmcpConnection`, over the official `rmcp` SDK), never a raw process spawn or an `rmcp` reference from
/// `mcp/application/`. Mirrors `hooks_process_io_confined_to_infrastructure` for the sibling active
/// capability; same AST-based resistance (issue #50).
#[test]
fn mcp_process_io_confined_to_infrastructure() {
    let mcp_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("modules")
        .join("mcp");
    assert_process_io_confined(
        &mcp_root,
        &[("tokio", "process"), ("std", "process")],
        &["rmcp"],
        "ADR 0021 — process/rmcp I/O must stay in mcp/infrastructure/",
    );
}

#[cfg(test)]
mod banned_path_visitor_tests {
    use super::{BannedPathVisitor, flatten_use_tree};

    fn findings_for(source: &str) -> Vec<String> {
        let parsed = syn::parse_file(source).expect("parse test snippet");
        let mut visitor = BannedPathVisitor {
            banned_pairs: &[("tokio", "process"), ("std", "process")],
            banned_roots: &["rmcp"],
            findings: Vec::new(),
        };
        syn::visit::Visit::visit_file(&mut visitor, &parsed);
        visitor.findings
    }

    #[test]
    fn catches_a_renamed_use_import_inside_a_brace_group() {
        // The actual evasion the OLD literal `source.contains("tokio::process")` grep missed: a plain
        // `use tokio::process::Command as Cmd;` still contains the contiguous substring "tokio::process"
        // (verified — that form alone would NOT have evaded the old grep). Wrapping it in a brace group,
        // `use tokio::{process::Command as Cmd};`, splits "tokio" and "process" across a `{`, so the
        // contiguous substring never appears in the source text at all — while `Cmd::new(...)` never
        // mentions the real name either. The AST-based flatten reconstructs the true path regardless.
        let findings =
            findings_for("use tokio::{process::Command as Cmd};\nfn spawn() { Cmd::new(\"sh\"); }");
        assert!(
            findings.iter().any(|f| f.contains("tokio::process")),
            "a brace-grouped, renamed import must still be caught even though no contiguous \
             \"tokio::process\" substring exists anywhere in the source: {findings:?}"
        );
    }

    #[test]
    fn catches_an_inline_fully_qualified_path_with_no_use() {
        let findings = findings_for("fn spawn() { std::process::Command::new(\"sh\"); }");
        assert!(
            findings.iter().any(|f| f.contains("std::process")),
            "a fully-qualified reference with no `use` at all must still be caught: {findings:?}"
        );
    }

    #[test]
    fn catches_an_rmcp_glob_import() {
        let findings = findings_for("use rmcp::*;\nfn f() {}");
        assert!(
            findings.iter().any(|f| f.contains("rmcp")),
            "a glob import of a banned root must still be caught: {findings:?}"
        );
    }

    #[test]
    fn does_not_flag_an_unrelated_import_or_a_matching_word_in_a_comment_or_string() {
        // The literal-grep predecessor of this guard could false-positive on a comment/string containing
        // the banned text; the AST-based version only ever inspects real `use`/path syntax.
        let findings = findings_for(
            "// this comment mentions tokio::process and Command::new but is not code\n\
             use std::collections::HashMap;\n\
             fn f() -> &'static str { \"tokio::process::Command::new\" }",
        );
        assert!(
            findings.is_empty(),
            "a comment/string containing the banned text must never be flagged: {findings:?}"
        );
    }

    #[test]
    fn flatten_use_tree_expands_a_grouped_use_and_keeps_the_pre_rename_name() {
        let file =
            syn::parse_file("use tokio::{process::Command as Cmd, time::sleep};").expect("parse");
        let syn::Item::Use(item_use) = &file.items[0] else {
            panic!("expected a use item");
        };
        let mut out = Vec::new();
        flatten_use_tree(&item_use.tree, &mut Vec::new(), &mut out);
        assert!(
            out.contains(&vec![
                "tokio".to_string(),
                "process".to_string(),
                "Command".to_string()
            ]),
            "the pre-rename path must be recorded, not the alias: {out:?}"
        );
        assert!(
            out.contains(&vec![
                "tokio".to_string(),
                "time".to_string(),
                "sleep".to_string()
            ]),
            "every branch of a grouped use must be expanded: {out:?}"
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
