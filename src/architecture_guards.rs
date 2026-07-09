//! Structural guards for the domain-purity invariants (ADR 0003), walking the source tree so those rules
//! cannot silently rot. The inward import-direction rule (application/domain must not import
//! infrastructure) is a convention these guards do NOT yet enforce; do not claim it here until one does.

#![cfg(test)]

use std::path::{Path, PathBuf};

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

/// Recursive, so a future nested `domain/<sub>/foo.rs` cannot silently escape either purity guard.
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

/// ADR 0017: `InputBuffer` is the *only* sanctioned `domain → UI-crate` coupling.
#[test]
fn only_input_buffer_couples_domain_to_ui_crates() {
    let allowed = Path::new("tui").join("domain").join("input_buffer.rs");
    for file in &domain_files() {
        let source = std::fs::read_to_string(file).expect("read domain file");
        // Bare crate-path tokens, not just `use` forms, so a fully-qualified `ratatui::Style` cannot
        // re-breach. A comment naming the crate false-positives, but fails loud and is easily reworded.
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

/// ADR 0020: a `.env` may be loaded ONLY from the trusted `~/.kiri`. The argless `dotenvy::dotenv()`
/// reads the cwd — a hostile repo the user `cd`s into, injecting `KIRI_SANDBOX*` / `*_API_KEY`. The
/// sanctioned path is `dotenvy::from_path` in `config::load_global_env`.
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

/// Flatten a `use` tree into every concrete import path it names, as segment lists. Keeps the PRE-rename
/// identifier (`as` rebinds locally, it does not change what the import points at), so an alias cannot
/// hide the real path from the banned-prefix check. A glob records the globbed-into prefix: importing the
/// module is the violation, whichever items are pulled in.
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

/// Records every path whose leading segments match a banned prefix. Checks `use` imports AND every other
/// `syn::Path`, because a boundary can be crossed either through a (possibly renamed) local binding or
/// through an inline fully-qualified call with no `use` at all. Walking the real AST means a banned
/// literal inside a comment or string cannot match (issue #50).
///
/// Two residual gaps, documented rather than silently claimed as closed — both theoretical today, since
/// this codebase contains neither construct:
///
/// - **Macro expansion.** `syn::parse_file` does not expand macros, so a banned path inside a
///   `macro_rules!` body is invisible. Closing it needs real expansion (`cargo expand`).
/// - **Cross-file re-export.** This walks one file at a time. An `infrastructure/` file re-exporting
///   `tokio::process::Command as SafeName` would let an `application/` importer flatten to local segments
///   only. Closing it needs cross-file name resolution.
struct BannedPathVisitor {
    /// e.g. `("tokio", "process")`.
    banned_pairs: &'static [(&'static str, &'static str)],
    /// e.g. `"rmcp"`.
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

/// Whether `cfg_token` (e.g. `"cfg(unix)"`) appears in `source` with `function_name` naming the item it
/// gates within a short following window — a much cheaper proxy for "this cfg attribute is on the
/// function we care about" than a full parse, but one a bare `source.contains(cfg_token)` doesn't give:
/// an unrelated `#[cfg(unix)]` elsewhere in the file (a `#[cfg(unix)] #[test]`, say) would satisfy the
/// bare check without saying anything about the owner-only writer specifically.
fn cfg_gates_function(source: &str, cfg_token: &str, function_name: &str) -> bool {
    const WINDOW: usize = 80;
    source.match_indices(cfg_token).any(|(pos, _)| {
        let end = (pos + cfg_token.len() + WINDOW).min(source.len());
        source[pos..end].contains(function_name)
    })
}

/// ADR 0027: every harness-owned "owner-only WRAPPER" — a function callers use to get platform-
/// appropriate private-file semantics, as opposed to `fs.rs`'s `write_atomic_owner_only`, a Unix-only
/// building block those wrappers call directly on Unix and skip entirely (falling back to the ordinary,
/// still-atomic `write_atomic`/`write_atomic_sync`) on other platforms — must have BOTH a `#[cfg(unix)]`
/// branch (real `0600`/`0700` mode bits) and a `#[cfg(not(unix))]` branch (Windows: accepted DACL
/// inheritance from the parent dir, still crash-atomic). Dropping the non-Unix fallback would be invisible
/// on this project's macOS dev/CI hosts — only the Unix branch needs to compile there — so this is a
/// source-scan guard, not something the compiler alone would catch. Each check is tied to the specific
/// function name via [`cfg_gates_function`], not a bare substring search, so an unrelated `#[cfg(unix)]`
/// elsewhere in the same file (e.g. a `#[cfg(unix)]`-gated test) cannot satisfy it.
#[test]
fn owner_only_writers_have_both_platform_branches() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let wrappers = [
        (
            manifest
                .join("src")
                .join("shared")
                .join("infra")
                .join("config")
                .join("writers.rs"),
            "ensure_private_dir",
        ),
        (
            manifest
                .join("src")
                .join("modules")
                .join("provider")
                .join("infrastructure")
                .join("secrets")
                .join("file_store.rs"),
            "write_owner_only",
        ),
        (
            manifest
                .join("src")
                .join("modules")
                .join("extensions")
                .join("infrastructure")
                .join("trust_store.rs"),
            "write_owner_only",
        ),
        (
            manifest
                .join("src")
                .join("modules")
                .join("sync")
                .join("infrastructure")
                .join("memory_ndjson.rs"),
            "write_owner_only",
        ),
    ];
    for (file, function_name) in &wrappers {
        assert!(
            file.is_file(),
            "expected an owner-only writer wrapper at {}",
            file.display()
        );
        let source = std::fs::read_to_string(file)
            .unwrap_or_else(|error| panic!("read {}: {error}", file.display()));
        assert!(
            cfg_gates_function(&source, "cfg(unix)", function_name),
            "{} must have a #[cfg(unix)] `{function_name}` branch (ADR 0027)",
            file.display()
        );
        assert!(
            cfg_gates_function(&source, "cfg(not(unix))", function_name),
            "{} must have a #[cfg(not(unix))] `{function_name}` fallback branch — its absence would \
             compile fine on this project's macOS hosts while silently dropping the documented Windows \
             behavior (ADR 0027)",
            file.display()
        );
    }

    // `fs.rs` is the Unix-only owner-only building block the wrappers above call directly — it needs no
    // `#[cfg(not(unix))]` sibling of its own (the wrappers' fallback is calling a DIFFERENT, already
    // platform-agnostic function, not a counterpart defined here), but it must exist and stay Unix-gated.
    let fs_rs = manifest
        .join("src")
        .join("shared")
        .join("infra")
        .join("fs.rs");
    let source = std::fs::read_to_string(&fs_rs)
        .unwrap_or_else(|error| panic!("read {}: {error}", fs_rs.display()));
    assert!(
        source.contains("cfg(unix)") && source.contains("write_atomic_owner_only"),
        "{} must still define the Unix-gated write_atomic_owner_only building block (ADR 0027)",
        fs_rs.display()
    );
}
