//! Fires ADR 0021 hooks at session/turn boundaries: fire-and-forget, notice-only — a failing or slow
//! hook never blocks or fails the session/turn it fired from. Global hooks always run; a project hook
//! runs only when the trust gate currently approves its exact content, re-checked on every firing (so an
//! edited or revoked hook silently stops running, no restart needed).
//!
//! `PreToolUse`/`PostToolUse` are discovered and gated like every other hook, but have no dispatcher here
//! yet: `ToolObserver`'s callbacks are synchronous, so firing one there needs new spawn+channel plumbing
//! into `Bridge` — a follow-up, not folded into this pass.

use std::sync::Arc;

use crate::modules::extensions::application::catalog::ExtensionCatalog;
use crate::modules::extensions::domain::gate;
use crate::modules::extensions::domain::resource::HookEvent;
use crate::modules::extensions::domain::scope::Layer;
use crate::modules::extensions::infrastructure::trust_store::ExtensionsTrustStore;
use crate::modules::hooks::application::hook_runner::HookRunner;
use crate::modules::tools::application::sandbox::Sandbox;
use crate::modules::tui::domain::model::Model;

/// The hook-dispatch dependencies, bundled so `dispatch_hooks` stays under the argument-count lint. Built
/// once in `app::wire` and cloned (all fields are `Arc`) into every place that fires hooks.
#[derive(Clone)]
pub struct HookContext {
    pub catalog: Arc<ExtensionCatalog>,
    pub runner: Arc<dyn HookRunner>,
    pub trust: Arc<ExtensionsTrustStore>,
}

/// Run every hook bound to `event`, in id order, against `sandbox`. Each run's outcome (success or
/// failure) surfaces as an info notice on `model`; this function itself never fails — a hook that times
/// out or errors is just reported, never propagated.
pub(super) async fn dispatch_hooks(
    event: HookEvent,
    hooks: &HookContext,
    sandbox: &dyn Sandbox,
    model: &mut Model,
) {
    for hook in hooks.catalog.hooks_for_event(event) {
        let approved = match hook.layer {
            Layer::Global | Layer::Bundled => true,
            Layer::Project => {
                let hash = gate::content_hash(&hook.hash_key());
                // Fail closed on a trust-store read error: treat as not-yet-approved rather than
                // propagating — a corrupt/unreadable store must never silently let a project hook run. A
                // retried `/approve-hook` surfaces the same read error directly (it reads-then-writes).
                let previously_approved = hooks
                    .trust
                    .is_approved("hook", &hook.id, &hash)
                    .unwrap_or(false);
                gate::resolve(hook.layer, previously_approved) == gate::GateState::Approved
            }
        };
        if !approved {
            continue;
        }
        let outcome = hooks.runner.run(sandbox, hook).await;
        let marker = if outcome.ok { "✓" } else { "✗" };
        model.notify_info(format!(
            "hook {marker} {}: {}",
            outcome.hook_id, outcome.summary
        ));
    }
}
