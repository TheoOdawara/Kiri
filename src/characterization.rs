//! Characterization snapshot of the tool surface, captured against the pre-refactor code.
//!
//! The hexagonal refactor moves the tool schemas and the pt-BR confirmation phrasing out of one
//! `services/tools.rs` file into one `impl Tool` per tool plus a `ToolRegistry`. This snapshot
//! freezes the *exact* current output (full schema JSON + every confirmation string + its
//! default-accept flag); any drift during that move breaks the build instead of slipping through.
//! When the registry replaces `tool_definitions`/`confirmation_prompt`, only the call sites in
//! `current_snapshot` change — the frozen `snapshots/characterization.json` stays byte-identical.

#![cfg(test)]

use std::fs;
use std::sync::Arc;

use regex::Regex;
use serde_json::{Value, json};

use crate::modules::tools::application::registry::ToolRegistry;
use crate::modules::tools::infrastructure::fs::default_fs_tools;
use crate::modules::tools::infrastructure::sandbox::Sandbox;
use crate::modules::tools::infrastructure::sensitive::SensitiveMatcher;
use crate::shared::kernel::tool_call::{FunctionCall, ToolCall};
use crate::shared::test_support::TempDir;

fn call(name: &str, args: Value) -> ToolCall {
    ToolCall {
        id: "c".to_string(),
        kind: "function".to_string(),
        function: FunctionCall {
            name: name.to_string(),
            arguments: args.to_string(),
        },
    }
}

/// One confirmation observation: (tool, args) → the exact prompt and default-accept the CLI shows.
fn confirmation_row(
    registry: &ToolRegistry,
    sandbox: &Sandbox,
    label: &str,
    name: &str,
    args: Value,
) -> Value {
    let confirmation = registry.confirm(sandbox, &call(name, args.clone()));
    json!({
        "label": label,
        "tool": name,
        "args": args,
        "prompt": confirmation.as_ref().map(|c| c.prompt.clone()),
        "default_accept": confirmation.as_ref().map(|c| c.default_accept),
    })
}

/// The full, exact tool surface as it behaves today: every schema + every confirmation variant.
fn current_snapshot() -> Value {
    let dir = TempDir::new("snap");
    let sandbox = Sandbox::new(&dir.path, SensitiveMatcher::empty()).unwrap();
    let registry = ToolRegistry::new(default_fs_tools(
        Arc::from(Vec::<Regex>::new()),
        Arc::from(Vec::<Regex>::new()),
        false,
    ));
    // Pre-seed a file so the overwrite/edit/delete variants resolve against an existing path.
    fs::write(dir.path.join("exists.txt"), b"data").unwrap();

    let r = &registry;
    let s = &sandbox;
    let confirmations = vec![
        confirmation_row(r, s, "read", "read_file", json!({ "path": "a.txt" })),
        confirmation_row(
            r,
            s,
            "read_absolute",
            "read_file",
            json!({ "path": "/etc/hosts" }),
        ),
        confirmation_row(r, s, "list", "list_dir", json!({})),
        confirmation_row(r, s, "search", "search", json!({ "query": "q" })),
        confirmation_row(
            r,
            s,
            "write_new",
            "write_file",
            json!({ "path": "new.txt", "content": "x" }),
        ),
        confirmation_row(
            r,
            s,
            "write_overwrite",
            "write_file",
            json!({ "path": "exists.txt", "content": "x" }),
        ),
        confirmation_row(
            r,
            s,
            "write_mkdir",
            "write_file",
            json!({ "path": "a/b/c.txt", "content": "x" }),
        ),
        confirmation_row(
            r,
            s,
            "edit",
            "edit_file",
            json!({ "path": "exists.txt", "old_string": "d", "new_string": "e" }),
        ),
        confirmation_row(
            r,
            s,
            "delete_file",
            "delete_file",
            json!({ "path": "exists.txt" }),
        ),
        confirmation_row(
            r,
            s,
            "create_dir",
            "create_dir",
            json!({ "path": "newdir" }),
        ),
        confirmation_row(r, s, "delete_dir", "delete_dir", json!({ "path": "d" })),
        confirmation_row(
            r,
            s,
            "move_clean",
            "move_path",
            json!({ "source": "exists.txt", "destination": "b.txt" }),
        ),
        confirmation_row(
            r,
            s,
            "move_mkdir",
            "move_path",
            json!({ "source": "exists.txt", "destination": "x/y/c.txt" }),
        ),
    ];

    json!({
        "tool_definitions": serde_json::to_value(registry.schemas()).unwrap(),
        "confirmations": confirmations,
    })
}

/// The frozen surface, captured against the pre-refactor tool layer (`services/tools.rs`).
const FROZEN: &str = include_str!("snapshots/characterization.json");

#[test]
fn tool_surface_matches_frozen_snapshot() {
    let frozen: Value = serde_json::from_str(FROZEN).expect("snapshot is valid JSON");
    // Structural `Value` equality — key order is irrelevant, so the refactor is free to reorder
    // fields as long as the schemas, prompts and default-accept flags stay byte-identical.
    assert_eq!(
        current_snapshot(),
        frozen,
        "tool surface drifted from the frozen characterization snapshot"
    );
}
