//! The TUI front-end facade. Holds the assembled [`Tui`], the [`RunLoop`] run-state struct, the
//! event-loop driver [`Tui::run`] with its effect dispatcher [`RunLoop::apply_effect`], and the borrowed
//! loop-handle context structs ([`UiDriver`]/[`EngineHandles`]). Each concern lives in a submodule behind
//! this facade: provider swapping, the per-turn driver, session persistence, distillation, `/sync`, and
//! render/clipboard glue. Re-exports keep `runtime::Tui`/`runtime::ProviderSwap`/`runtime::SyncContext`
//! stable for `app::wire`.

mod distill;
mod provider_swap;
mod render;
mod session_ops;
mod sync;
mod turn;

pub use provider_swap::ProviderSwap;
pub use sync::SyncContext;

use session_ops::SessionCursor;

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{EnableBracketedPaste, EnableMouseCapture, EventStream};
use ratatui::DefaultTerminal;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{self, Interval};
use tokio_stream::StreamExt;

use crate::modules::agent::application::agent_loop::AgentLoop;
use crate::modules::agent::application::approval_policy::Approval;
use crate::modules::memory::application::memory_port::Memory;
use crate::modules::session::application::session_store::SessionStore;
use crate::modules::tools::application::sandbox::Sandbox;
use crate::modules::tools::infrastructure::sandbox::FsSandbox;
use crate::modules::tui::application::command::{self, Command};
use crate::modules::tui::application::effect::Effect;
use crate::modules::tui::application::update::update;
use crate::modules::tui::domain::model::Model;
use crate::modules::tui::domain::transcript::TranscriptItem;
use crate::modules::tui::infrastructure::bridge::{Bridge, CancelToken, EngineMsg};
use crate::modules::tui::infrastructure::input;
use crate::modules::tui::infrastructure::terminal_guard::TerminalGuard;
use crate::modules::tui::infrastructure::text;
use crate::modules::tui::infrastructure::theme;
use crate::shared::kernel::conversation::Conversation;
use crate::shared::kernel::message::Message;
use crate::shared::kernel::provider::ProviderProfile;

/// The frame cadence: an idle TUI ticks at this rate, and the spinner advances one step per interval.
pub(super) const FRAME_INTERVAL: Duration = Duration::from_millis(120);

/// Minimum spacing between redraws while a turn streams. Finer than `FRAME_INTERVAL` so streamed text
/// flows at ~30 fps instead of appearing in coarse 120 ms blocks. It only paces draws that are already
/// being driven by incoming deltas, so an idle TUI still ticks at `FRAME_INTERVAL` and burns no extra CPU.
pub(super) const STREAM_FRAME: Duration = Duration::from_millis(33);

/// The full-screen TUI frontend: owns the engine handles and the UI model, runs the render/input loop,
/// and drives one agent turn at a time. The sole frontend, assembled in `app::wire`.
pub struct Tui {
    agent_loop: AgentLoop,
    sandbox: FsSandbox,
    conversation: Conversation,
    model: Model,
    seed: Option<String>,
    /// Kept so `/new` can rebuild a fresh conversation with the same system prompt. Owned because it
    /// may carry a per-session memory digest composed at wire time, not just the static base prompt.
    system_prompt: String,
    /// The inputs to rebuild the provider on a live `/effort` swap.
    provider_swap: ProviderSwap,
    /// The global config file, written on a live `/models`/`/effort` change.
    config_path: PathBuf,
    /// The wire-built sync ports + paths for a live `/sync` push, so the front-end constructs no adapter.
    sync_context: SyncContext,
    /// Persists the conversation across runs. Inert (`is_available() == false`) when sessions are
    /// disabled or the store failed to initialize — every call is then a graceful no-op.
    session_store: Arc<dyn SessionStore>,
    /// The durable memory, used to drive the end-of-session distillation. Inert scopes make it a no-op.
    memory: Arc<dyn Memory>,
    /// The workspace id sessions are keyed by; recomputed on `/cd`.
    project_id: String,
}

/// A non-fatal degradation observed while wiring the harness (memory/session/embeddings/provider
/// unavailable). Carried out of `app::wire` and surfaced in-transcript at boot instead of `eprintln!`,
/// which the alternate-screen TUI would otherwise hide.
pub struct BootNotice {
    message: String,
}

impl BootNotice {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Surface the wire-time boot degradations in the transcript as info-level notices (rendered in the
/// warning-amber tier). Info, not Error: a degradation is non-fatal, so it must not trip the editor's
/// error gate (`editor::gate_state`) or read as a hard failure.
fn surface_boot_notices(model: &mut Model, notices: &[BootNotice]) {
    for notice in notices {
        model.notify_info(notice.message.as_str());
    }
}

/// The wire-time inputs to [`Tui::new`], grouped so the constructor takes a single argument (no
/// argument-count lint). Assembled in `app::wire`, the one place the adapters are chosen.
pub struct TuiParams {
    pub agent_loop: AgentLoop,
    pub sandbox: FsSandbox,
    pub system_prompt: String,
    pub seed: Option<String>,
    pub provider_swap: ProviderSwap,
    pub config_path: PathBuf,
    pub sync_context: SyncContext,
    pub needs_onboarding: bool,
    pub session_store: Arc<dyn SessionStore>,
    pub memory: Arc<dyn Memory>,
    pub project_id: String,
    pub boot_notices: Vec<BootNotice>,
}

/// The long-lived owned run state, aggregated so the per-turn driver and the effect handlers are
/// `&mut self` methods that take only the effect payload plus the borrowed loop handles — never the owned
/// state by argument. This is what keeps every handler under the argument-count lint with no `#[allow]`.
pub(super) struct RunLoop {
    agent_loop: AgentLoop,
    sandbox: FsSandbox,
    conversation: Conversation,
    model: Model,
    system_prompt: String,
    provider_swap: ProviderSwap,
    config_path: PathBuf,
    sync_context: SyncContext,
    session_store: Arc<dyn SessionStore>,
    memory: Arc<dyn Memory>,
    project_id: String,
    cursor: SessionCursor,
}

/// The live loop handles threaded into the per-turn driver and distillation: the terminal to draw on, the
/// crossterm event stream, and the frame ticker. Disjoint borrows, grouped so the handlers stay small.
pub(super) struct UiDriver<'a> {
    pub(super) terminal: &'a mut DefaultTerminal,
    pub(super) events: &'a mut EventStream,
    pub(super) ticker: &'a mut Interval,
}

/// The engine-side handles for the per-turn driver: the bridge the agent loop reports through, the
/// receiver of its messages, the cooperative cancel token, and the slot for a pending approval's reply.
pub(super) struct EngineHandles<'a> {
    pub(super) bridge: &'a mut Bridge,
    pub(super) engine_rx: &'a mut mpsc::UnboundedReceiver<EngineMsg>,
    pub(super) cancel: &'a CancelToken,
    pub(super) pending_reply: &'a mut Option<oneshot::Sender<Approval>>,
}

impl Tui {
    pub fn new(params: TuiParams) -> Self {
        let TuiParams {
            agent_loop,
            sandbox,
            system_prompt,
            seed,
            provider_swap,
            config_path,
            sync_context,
            needs_onboarding,
            session_store,
            memory,
            project_id,
            boot_notices,
        } = params;
        let workspace = text::display_path(sandbox.root());
        let (model_id, models) = provider_swap
            .active_profile()
            .map(|p| (p.model.clone(), p.models.clone()))
            .unwrap_or_default();
        let mut model = Model::new(model_id, workspace)
            .with_provider_catalog(models, provider_swap.effort)
            .with_providers(provider_swap.active.clone(), provider_swap.provider_ids());
        // Surface the wire-time degradations first, so the onboarding welcome (the call to action) lands
        // last when both are present.
        surface_boot_notices(&mut model, &boot_notices);
        // No usable credential at boot: come up in onboarding (welcome wizard + submit gate) instead of
        // crashing, so the user can configure a provider with zero env vars.
        if needs_onboarding {
            model.enter_onboarding();
        }
        Self {
            agent_loop,
            sandbox,
            conversation: Conversation::new(system_prompt.clone()),
            model,
            seed,
            system_prompt,
            provider_swap,
            config_path,
            sync_context,
            session_store,
            memory,
            project_id,
        }
    }

    pub async fn run(self) -> Result<()> {
        let Tui {
            agent_loop,
            sandbox,
            conversation,
            mut model,
            seed,
            system_prompt,
            provider_swap,
            config_path,
            sync_context,
            session_store,
            memory,
            project_id,
        } = self;

        let mut terminal = ratatui::init();
        let _guard = TerminalGuard;
        // Best-effort: bracketed paste / mouse capture are nice-to-have enhancements; a terminal that
        // rejects them still runs fully. The TerminalGuard disables them symmetrically on exit.
        let _ = crossterm::execute!(io::stdout(), EnableBracketedPaste, EnableMouseCapture);

        // The editor widget owns its own styling; paint it with the brand theme once at startup. The
        // editor's own selection shares the screen-selection highlight, so the two read identically.
        let cursor = ratatui::style::Style::default()
            .fg(theme::VOID)
            .bg(theme::HIGHLIGHT);
        model
            .input
            .set_styles(theme::base(), cursor, theme::selection());
        // Resolve the motion preference once: reading the environment is infrastructure's job, kept out
        // of the pure domain. The view folds in per-frame geometry on top of this.
        model.timeline.motion = render::resolve_motion();
        // Stamp the open instant for the splash breath-in and the cursor pulse (clock stays out of the
        // domain constructor).
        model.timeline.opened_at = Some(Instant::now());

        let (engine_tx, mut engine_rx) = mpsc::unbounded_channel::<EngineMsg>();
        let cancel = CancelToken::new();
        let mut bridge = Bridge::new(engine_tx, cancel.clone());
        let mut pending_reply: Option<oneshot::Sender<Approval>> = None;
        let mut events = EventStream::new();
        let mut ticker = time::interval(FRAME_INTERVAL);

        // Aggregate the long-lived owned state once; the handlers drive it as `&mut self` methods.
        let mut run_loop = RunLoop {
            agent_loop,
            sandbox,
            conversation,
            model,
            system_prompt,
            provider_swap,
            config_path,
            sync_context,
            session_store,
            memory,
            project_id,
            // Session persistence cursor: the row backing the current conversation (lazily created on the
            // first flush, so an empty session never hits the DB) and how many non-system messages have
            // already been written, so each flush appends only the new tail.
            cursor: SessionCursor {
                session_id: None,
                persisted_len: 0,
            },
        };
        let mut ui = UiDriver {
            terminal: &mut terminal,
            events: &mut events,
            ticker: &mut ticker,
        };
        let mut engine = EngineHandles {
            bridge: &mut bridge,
            engine_rx: &mut engine_rx,
            cancel: &cancel,
            pending_reply: &mut pending_reply,
        };

        // An initial prompt from the CLI runs as the first turn.
        if let Some(line) = seed.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
            match command::parse(&line) {
                Some(Command::Quit) => run_loop.model.should_quit = true,
                // A non-quit command as the CLI seed is ignored; the seed is meant to be a prompt.
                Some(_) => {}
                // Onboarding: there is no usable provider yet, so the seed can't run against the null
                // provider. Surface it and let the user configure a provider via the welcome wizard.
                None if run_loop.model.unconfigured => {
                    run_loop.model.notify_info(
                        "configure um provider antes de enviar — a mensagem inicial foi ignorada",
                    );
                }
                None => {
                    run_loop.model.history.record(&line);
                    run_loop
                        .model
                        .transcript
                        .push(TranscriptItem::User(line.clone()));
                    run_loop.model.busy = true;
                    run_loop.conversation.push(Message::user(line));
                    run_loop.drive_turn(&mut ui, &mut engine).await?;
                    run_loop.flush_session().await;
                }
            }
        }

        while !run_loop.model.should_quit {
            run_loop.model.timeline.render_at = Some(Instant::now());
            render::draw_and_copy(ui.terminal, &mut run_loop.model)?;

            // Resolve one input into a message, then handle it outside the select so the engine
            // handles are unambiguously free when a turn is armed.
            let msg = tokio::select! {
                biased;
                maybe = ui.events.next() => match maybe {
                    Some(Ok(event)) => {
                        // Stamp arrival time for multi-click detection (before the reducer reads it).
                        run_loop.model.timeline.last_event_at = Some(Instant::now());
                        input::to_msg(event)
                    }
                    Some(Err(_)) => None,
                    None => {
                        run_loop.model.should_quit = true;
                        None
                    }
                },
                _ = ui.ticker.tick() => None,
            };
            let Some(msg) = msg else {
                continue;
            };

            for effect in update(&mut run_loop.model, msg) {
                run_loop.apply_effect(effect, &mut ui, &mut engine).await?;
            }
        }

        // Distill the final session before tearing down, so the last conversation also teaches the
        // memory. Best-effort, bounded, and Ctrl+C-skippable — quit is never held hostage.
        run_loop.drive_distillation(&mut ui).await;

        Ok(())
    }
}

impl RunLoop {
    /// Dispatch one effect the reducer requested. The long-running arms delegate to `&mut self` handler
    /// methods in the area modules; the short arms run inline. Returns `Result<()>` because some handlers
    /// (`submit_prompt`/`approve_plan`) and the inline draws propagate an I/O error; loop exit stays
    /// governed solely by `model.should_quit`, so no `ControlFlow` is needed.
    pub(super) async fn apply_effect(
        &mut self,
        effect: Effect,
        ui: &mut UiDriver<'_>,
        engine: &mut EngineHandles<'_>,
    ) -> Result<()> {
        match effect {
            Effect::SubmitPrompt { text, images } => {
                self.submit_prompt(text, images, ui, engine).await?
            }
            Effect::CopyToClipboard(text) => render::copy_to_clipboard(&mut self.model, &text),
            Effect::PasteClipboard => render::paste_from_clipboard(&mut self.model),
            Effect::PlaceCursor { col, row } => {
                render::place_cursor(&mut self.model, ui.terminal, col, row)
            }
            Effect::Quit => self.model.should_quit = true,
            Effect::NewSession => self.new_session(ui).await,
            Effect::ListSessions => self.list_sessions().await,
            Effect::SyncPush => {
                sync::sync_push(&self.sync_context, &mut self.model, ui.terminal).await
            }
            Effect::ResumeLast => self.resume_last(ui).await,
            Effect::OpenSession(id) => self.open_session(&id, ui).await,
            Effect::ChangeWorkspace(path) => self.change_workspace(path, ui).await,
            Effect::ApprovePlan(mode) => self.approve_plan(mode, ui, engine).await?,
            Effect::SetModel(model_id) => self.apply_set_model(model_id),
            Effect::SetEffort(effort) => self.apply_set_effort(effort),
            Effect::SetProvider(id) => self.apply_set_provider(id),
            Effect::SaveProvider {
                id,
                kind,
                base_url,
                model: model_id,
                models,
                auth,
            } => {
                let profile = ProviderProfile {
                    id,
                    kind,
                    base_url,
                    model: model_id,
                    models,
                    auth,
                };
                self.apply_save_provider(profile);
            }
            Effect::AnswerApproval(_) | Effect::CancelTurn => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::turn::{on_turn_end, turn_produced_nothing};
    use super::{BootNotice, surface_boot_notices};
    use crate::modules::agent::application::agent_loop::TurnOutcome;
    use crate::modules::tui::domain::model::Model;
    use crate::modules::tui::domain::transcript::{NoticeLevel, TranscriptItem};
    use crate::shared::kernel::approval_mode::ApprovalMode;
    use crate::shared::kernel::conversation::Conversation;
    use crate::shared::kernel::message::Message;

    // The front-end must not be a composition root: the sync adapter *choice* and the shared-DB path now
    // live only in `app::wire`. Guard that runtime.rs constructs no sync adapter and recomputes no path.
    #[test]
    fn runtime_has_no_sync_adapter_construction() {
        // Scan the facade AND every runtime submodule: `sync_push` now lives in `runtime/sync.rs`, so a
        // facade-only scan would no longer cover the relocated sync code and the guard would rot.
        let sources = [
            include_str!("runtime.rs"),
            include_str!("runtime/provider_swap.rs"),
            include_str!("runtime/turn.rs"),
            include_str!("runtime/session_ops.rs"),
            include_str!("runtime/distill.rs"),
            include_str!("runtime/sync.rs"),
            include_str!("runtime/render.rs"),
        ];
        // Build needles by concatenation so this guard's own literals do not self-match.
        for source in sources {
            for needle in [
                concat!("Sqlite", "SharedMemory"),
                concat!("Git", "Cli"),
                concat!("join(\"memory\")", ".join(\"shared.db\")"),
            ] {
                assert!(
                    !source.contains(needle),
                    "runtime (incl. submodules) must not construct sync adapters or recompute the shared-db path: {needle:?}"
                );
            }
        }
    }

    fn has_error_notice(model: &Model) -> bool {
        model
            .transcript
            .items()
            .iter()
            .any(|item| matches!(item, TranscriptItem::Notice(NoticeLevel::Error, _)))
    }

    #[test]
    fn boot_degradation_surfaces_as_in_transcript_notice() {
        // A wire-time degradation must reach the transcript (the alternate-screen TUI hides stderr) as a
        // non-fatal info/warning-tier notice — never an Error, which would trip the editor's error gate.
        let mut model = Model::new("m".to_string(), "/w".to_string());
        surface_boot_notices(
            &mut model,
            &[BootNotice::new(
                "session store unavailable; continuing without it",
            )],
        );
        assert!(
            matches!(
                model.transcript.items().last(),
                Some(TranscriptItem::Notice(NoticeLevel::Info, t)) if t.contains("session store unavailable")
            ),
            "the degradation must surface as an in-transcript info notice"
        );
        assert!(
            !has_error_notice(&model),
            "a non-fatal boot degradation must not be an Error notice"
        );
    }

    /// Tests for the live provider swap. The nested module can reach `ProviderSwap`'s private fields and
    /// methods (privacy is visible to descendant modules). Building an adapter does no I/O, so these run
    /// hermetically against a fake credential store.
    mod provider_swap {
        use super::super::ProviderSwap;
        use crate::modules::provider::application::secret_store::SecretStore;
        use crate::shared::kernel::error::AgentError;
        use crate::shared::kernel::provider::{
            AuthMethod, Credential, Effort, ProviderKind, ProviderProfile, Secret,
        };
        use std::collections::HashMap;

        struct FakeStore {
            creds: HashMap<String, Credential>,
        }
        impl SecretStore for FakeStore {
            fn get(&self, id: &str) -> Result<Option<Credential>, AgentError> {
                Ok(self.creds.get(id).cloned())
            }
            fn set(&self, _id: &str, _credential: &Credential) -> Result<(), AgentError> {
                Ok(())
            }
            fn delete(&self, _id: &str) -> Result<(), AgentError> {
                Ok(())
            }
        }

        fn profile(id: &str, kind: ProviderKind, model: &str) -> ProviderProfile {
            ProviderProfile {
                id: id.into(),
                kind,
                base_url: "https://example.test/v1".into(),
                model: model.into(),
                models: vec![model.into()],
                auth: AuthMethod::ApiKey,
            }
        }

        /// A keyless (`auth = "none"`) profile — a local OpenAI-compatible endpoint that needs no key.
        fn keyless_profile(id: &str, kind: ProviderKind, model: &str) -> ProviderProfile {
            ProviderProfile {
                id: id.into(),
                kind,
                base_url: "http://localhost:1234/v1".into(),
                model: model.into(),
                models: vec![model.into()],
                auth: AuthMethod::None,
            }
        }

        fn api_key() -> Credential {
            Credential::ApiKey {
                key: Secret::new("k"),
            }
        }

        /// A store whose `set` errors — used to prove a keyless save never persists a credential (it would
        /// fail here if it tried).
        struct FailingSetStore;
        impl SecretStore for FailingSetStore {
            fn get(&self, _id: &str) -> Result<Option<Credential>, AgentError> {
                Ok(None)
            }
            fn set(&self, _id: &str, _credential: &Credential) -> Result<(), AgentError> {
                Err(AgentError::Provider(
                    "set must not be called for a keyless provider".into(),
                ))
            }
            fn delete(&self, _id: &str) -> Result<(), AgentError> {
                Ok(())
            }
        }

        fn swap(
            providers: Vec<ProviderProfile>,
            active: &str,
            stored: &[(&str, Credential)],
        ) -> ProviderSwap {
            let mut creds = HashMap::new();
            for (id, credential) in stored {
                creds.insert((*id).to_string(), credential.clone());
            }
            let active_cred = creds.get(active).cloned().unwrap_or_else(api_key);
            ProviderSwap::new(
                reqwest::Client::new(),
                Box::new(FakeStore { creds }),
                providers,
                active.into(),
                Some(active_cred),
                true,
                Effort::High,
            )
        }

        #[test]
        fn switch_to_swaps_active_and_adopts_the_target_model() {
            let mut s = swap(
                vec![
                    profile("nvidia", ProviderKind::Nvidia, "m1"),
                    profile("claude", ProviderKind::Anthropic, "claude-opus-4-8"),
                ],
                "nvidia",
                &[("claude", api_key())],
            );
            let switch = s.switch_to("claude").unwrap();
            assert_eq!(switch.model, "claude-opus-4-8");
            assert!(
                switch.persist_warning.is_none(),
                "a stored credential needs no env import, so no persist warning"
            );
            assert_eq!(s.active, "claude");
        }

        #[test]
        fn switch_to_unknown_provider_errors() {
            let mut s = swap(
                vec![profile("nvidia", ProviderKind::Nvidia, "m1")],
                "nvidia",
                &[("nvidia", api_key())],
            );
            assert!(s.switch_to("ghost").is_err());
        }

        #[test]
        fn switch_to_without_a_credential_or_env_errors() {
            // A Custom kind with a unique id: no vendor env var and `KIRI_<ID>_API_KEY` is unset, so
            // there is neither a stored credential nor an env fallback.
            let mut s = swap(
                vec![
                    profile("nvidia", ProviderKind::Nvidia, "m1"),
                    profile("unit-test-custom-xyz", ProviderKind::Custom, "m2"),
                ],
                "nvidia",
                &[("nvidia", api_key())],
            );
            assert!(s.switch_to("unit-test-custom-xyz").is_err());
        }

        #[test]
        fn rebuild_with_effort_commits_the_effort_on_success() {
            let mut s = swap(
                vec![profile("nvidia", ProviderKind::Nvidia, "m1")],
                "nvidia",
                &[("nvidia", api_key())],
            );
            s.rebuild_with_effort(Effort::Max).unwrap();
            assert_eq!(s.effort, Effort::Max);
        }

        #[test]
        fn rebuild_with_effort_without_credential_errors() {
            // Onboarding state: a seeded provider but no live credential. Changing effort must error
            // clearly and leave the dial untouched, never panic.
            let mut s = ProviderSwap::new(
                reqwest::Client::new(),
                Box::new(FakeStore {
                    creds: HashMap::new(),
                }),
                vec![profile("nvidia", ProviderKind::Nvidia, "m1")],
                "nvidia".into(),
                None,
                true,
                Effort::High,
            );
            assert!(s.rebuild_with_effort(Effort::Max).is_err());
            assert_eq!(s.effort, Effort::High, "the effort dial must not change");
        }

        #[test]
        fn add_and_activate_adds_the_provider_and_selects_it() {
            let mut s = swap(
                vec![profile("nvidia", ProviderKind::Nvidia, "m1")],
                "nvidia",
                &[("nvidia", api_key())],
            );
            let (_, model) = s
                .add_and_activate(
                    profile("claude", ProviderKind::Anthropic, "claude-opus-4-8"),
                    api_key(),
                )
                .unwrap();
            assert_eq!(model, "claude-opus-4-8");
            assert_eq!(s.active, "claude");
            assert!(s.provider_ids().iter().any(|p| p == "claude"));
        }

        #[test]
        fn resolve_credential_yields_none_for_a_keyless_profile() {
            let s = swap(
                vec![profile("nvidia", ProviderKind::Nvidia, "m1")],
                "nvidia",
                &[("nvidia", api_key())],
            );
            let p = keyless_profile("lmstudio", ProviderKind::OpenAiCompatible, "gemma");
            let (credential, persist_warning) = s.resolve_credential(&p).unwrap();
            assert!(matches!(credential, Credential::None));
            assert!(persist_warning.is_none(), "keyless needs no env import");
        }

        #[test]
        fn switch_to_no_auth_provider_needs_no_credential() {
            // A keyless provider in the catalog switches with neither a stored credential nor an env key —
            // the direct fix for the reported "no credential for provider" on /provider.
            let mut s = swap(
                vec![
                    profile("nvidia", ProviderKind::Nvidia, "m1"),
                    keyless_profile("lmstudio", ProviderKind::OpenAiCompatible, "gemma"),
                ],
                "nvidia",
                &[("nvidia", api_key())],
            );
            let switch = s
                .switch_to("lmstudio")
                .expect("a keyless switch needs no credential");
            assert_eq!(switch.model, "gemma");
            assert!(switch.persist_warning.is_none());
            assert_eq!(s.active, "lmstudio");
        }

        #[test]
        fn add_and_activate_none_credential_skips_secret_store() {
            // A keyless save must succeed without calling secrets.set (which errors in this store).
            let mut s = ProviderSwap::new(
                reqwest::Client::new(),
                Box::new(FailingSetStore),
                vec![],
                String::new(),
                None,
                true,
                Effort::High,
            );
            let (_, model) = s
                .add_and_activate(
                    keyless_profile("lmstudio", ProviderKind::OpenAiCompatible, "gemma"),
                    Credential::None,
                )
                .expect("keyless add must not touch secrets.set");
            assert_eq!(model, "gemma");
            assert_eq!(s.active, "lmstudio");
        }

        #[test]
        fn rebuild_with_effort_works_for_a_keyless_active_provider() {
            // The keyless active provider caches Some(Credential::None); changing effort must rebuild,
            // never hit the "configure um provider" error a None credential would trigger.
            let mut s = ProviderSwap::new(
                reqwest::Client::new(),
                Box::new(FakeStore {
                    creds: HashMap::new(),
                }),
                vec![keyless_profile(
                    "lmstudio",
                    ProviderKind::OpenAiCompatible,
                    "gemma",
                )],
                "lmstudio".into(),
                Some(Credential::None),
                true,
                Effort::High,
            );
            s.rebuild_with_effort(Effort::Max)
                .expect("a keyless effort rebuild must succeed");
            assert_eq!(s.effort, Effort::Max);
        }
    }

    #[test]
    fn empty_completion_surfaces_a_notice_and_no_plan_box() {
        // The exact regression: a plan-mode turn whose provider returned nothing must NOT show a plan
        // box, and must surface a visible error instead of failing silently.
        let mut model = Model::new("m".to_string(), "/w".to_string());
        model.approval_mode = ApprovalMode::Plan;
        model.busy = true;
        let mut conversation = Conversation::new("system");
        conversation.push(Message::user("faça um plano"));
        conversation.push(Message::assistant_text("")); // the empty reply the loop appended

        on_turn_end(
            Ok(TurnOutcome::Completed),
            false,
            &mut model,
            &mut conversation,
        );

        assert!(
            model.pending_plan.is_none(),
            "an empty turn must not pop a phantom plan box"
        );
        assert!(
            has_error_notice(&model),
            "an empty turn must surface an error notice"
        );
    }

    #[test]
    fn a_cancel_aborts_the_turn_without_quitting() {
        // A single ^C while busy cancels just the turn: drive_turn synthesizes Aborted with the cancel
        // token set (cancelled == true). The app must NOT quit — only a genuine input-stream end does.
        let mut model = Model::new("m".to_string(), "/w".to_string());
        model.busy = true;
        let mut conversation = Conversation::new("system");
        conversation.push(Message::user("rodar algo demorado"));

        on_turn_end(
            Ok(TurnOutcome::Aborted),
            true,
            &mut model,
            &mut conversation,
        );

        assert!(
            !model.should_quit,
            "^C must cancel the turn, not quit the app"
        );
    }

    #[test]
    fn a_genuine_abort_quits() {
        // The approval channel closed (cancelled == false): this is a real session end and must quit.
        let mut model = Model::new("m".to_string(), "/w".to_string());
        let mut conversation = Conversation::new("system");
        on_turn_end(
            Ok(TurnOutcome::Aborted),
            false,
            &mut model,
            &mut conversation,
        );
        assert!(model.should_quit, "a genuine abort must quit");
    }

    #[test]
    fn present_plan_outcome_renders_the_plan_and_offers_the_box() {
        // A plan is surfaced ONLY via the explicit `present_plan` tool (TurnOutcome::PlanProposed):
        // the plan text is rendered and the approval box opens.
        let mut model = Model::new("m".to_string(), "/w".to_string());
        model.approval_mode = ApprovalMode::Plan;
        let mut conversation = Conversation::new("system");
        conversation.push(Message::user("faça um plano"));

        on_turn_end(
            Ok(TurnOutcome::PlanProposed(
                "## Plano\n1. fazer X".to_string(),
            )),
            false,
            &mut model,
            &mut conversation,
        );

        assert!(
            model.pending_plan.is_some(),
            "a proposed plan must offer the plan box"
        );
        assert!(
            model.transcript.items().iter().any(|item| matches!(
                item,
                TranscriptItem::Assistant(text) if text.contains("Plano")
            )),
            "the proposed plan text must be rendered in the transcript"
        );
        assert!(!has_error_notice(&model), "a proposed plan is not an error");
    }

    #[test]
    fn plain_plan_mode_completion_does_not_pop_the_box() {
        // A plain text turn in plan mode (the model thought aloud or asked a question, but did NOT
        // call present_plan) must NOT open the approval box — the old eager heuristic was the bug.
        let mut model = Model::new("m".to_string(), "/w".to_string());
        model.approval_mode = ApprovalMode::Plan;
        let mut conversation = Conversation::new("system");
        conversation.push(Message::user("faça um plano"));
        conversation.push(Message::assistant_text(
            "Preciso de mais detalhes: qual módulo?",
        ));

        on_turn_end(
            Ok(TurnOutcome::Completed),
            false,
            &mut model,
            &mut conversation,
        );

        assert!(
            model.pending_plan.is_none(),
            "a plain plan-mode turn must not pop the box without present_plan"
        );
        assert!(!has_error_notice(&model), "a real reply is not an error");
    }

    #[test]
    fn spinner_frame_advances_one_step_per_frame_interval() {
        use super::FRAME_INTERVAL;
        use super::turn::spinner_frame;
        use std::time::Duration;
        assert_eq!(spinner_frame(Duration::ZERO), 0);
        assert_eq!(spinner_frame(FRAME_INTERVAL - Duration::from_millis(1)), 0);
        assert_eq!(spinner_frame(FRAME_INTERVAL), 1);
        assert_eq!(spinner_frame(FRAME_INTERVAL * 5), 5);
    }

    #[test]
    fn place_cursor_moves_the_edit_cursor() {
        use super::render::place_cursor;
        use crate::modules::tui::infrastructure::view::frame_regions;
        use crate::modules::tui::infrastructure::widgets::editor;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::layout::Rect;

        let mut model = Model::new("m".to_string(), "/w".to_string());
        model.input.set("hello world".to_string()); // one short line — the unambiguous regime
        let terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();

        // Resolve the editor rect the same way the runtime will, then click two cells into the text.
        let editor_area =
            editor::content_rect(frame_regions(Rect::new(0, 0, 40, 10), &model).input);
        place_cursor(&mut model, &terminal, editor_area.x + 2, editor_area.y);
        assert_eq!(
            model.input.cursor(),
            (0, 2),
            "a click two cells into the line lands at char index 2"
        );
    }

    #[test]
    fn a_tool_only_turn_is_not_treated_as_empty() {
        // A turn that ended on a tool result (e.g. a declined checkpoint) produced activity — it is not
        // "nothing", so no spurious error notice.
        let mut conversation = Conversation::new("system");
        conversation.push(Message::user("read a.txt"));
        conversation.push(Message::tool_result("c1", "hello"));
        assert!(!turn_produced_nothing(&conversation));
    }
}
