use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{
    EnableBracketedPaste, EnableMouseCapture, Event, EventStream, KeyCode, KeyModifiers,
};
use ratatui::backend::Backend;
use ratatui::layout::Rect;
use ratatui::{DefaultTerminal, Terminal};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{self, Interval};
use tokio_stream::StreamExt;

use crate::modules::agent::application::agent_loop::{AgentLoop, TurnOutcome};
use crate::modules::agent::application::approval_policy::Approval;
use crate::modules::memory::application::distill::Distiller;
use crate::modules::memory::application::memory_port::MemoryPort;
use crate::modules::memory::application::shared_memory::SharedMemory;
use crate::modules::memory::domain::project_id::project_id_from_path;
use crate::modules::memory::infrastructure::sqlite_shared_memory::SqliteSharedMemory;
use crate::modules::provider::application::completion_provider::CompletionProvider;
use crate::modules::provider::application::secret_store::SecretStore;
use crate::modules::provider::infrastructure::factory::{api_key_from_env, build_provider};
use crate::modules::session::application::session_store::SessionStore;
use crate::modules::session::domain::session::derive_title;
use crate::modules::sync::application::sync_service::SyncService;
use crate::modules::sync::infrastructure::git_cli::GitCli;
use crate::modules::tools::infrastructure::sandbox::Sandbox;
use crate::modules::tui::application::command::{self, Command};
use crate::modules::tui::application::effect::Effect;
use crate::modules::tui::application::msg::{Msg, StreamKind};
use crate::modules::tui::application::update::update;
use crate::modules::tui::domain::model::{Model, Motion};
use crate::modules::tui::domain::transcript::{NoticeLevel, Transcript, TranscriptItem};
use crate::modules::tui::domain::view_state::{PendingPlan, Picker, PickerKind, SelectionState};
use crate::modules::tui::infrastructure::bridge::{Bridge, CancelToken, EngineMsg};
use crate::modules::tui::infrastructure::clipboard::{self, ClipboardContent};
use crate::modules::tui::infrastructure::input;
use crate::modules::tui::infrastructure::terminal_guard::TerminalGuard;
use crate::modules::tui::infrastructure::text;
use crate::modules::tui::infrastructure::theme;
use crate::modules::tui::infrastructure::view::{frame_regions, view};
use crate::modules::tui::infrastructure::widgets::{editor, selection_overlay};
use crate::shared::infra::config;
use crate::shared::kernel::approval_mode::ApprovalMode;
use crate::shared::kernel::conversation::Conversation;
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::message::Message;
use crate::shared::kernel::provider::{
    AuthMethod, Credential, Effort, ProviderKind, ProviderProfile, Secret,
};
use crate::shared::kernel::role::Role;

/// The agent-turn future, boxed and `!Send`. Driven as a `select!` arm — never spawned — so no
/// `Send`/`'static` bound is needed and the engine borrows stay plain references.
type TurnFuture<'a> = Pin<Box<dyn Future<Output = Result<TurnOutcome, AgentError>> + 'a>>;

const FRAME_INTERVAL: Duration = Duration::from_millis(120);

/// Minimum spacing between redraws while a turn streams. Finer than `FRAME_INTERVAL` so streamed text
/// flows at ~30 fps instead of appearing in coarse 120 ms blocks. It only paces draws that are already
/// being driven by incoming deltas, so an idle TUI still ticks at `FRAME_INTERVAL` and burns no extra CPU.
const STREAM_FRAME: Duration = Duration::from_millis(33);

/// Everything the runtime needs to rebuild the provider adapter for a live `/models`/`/effort`/
/// `/provider` change: the HTTP client, the secret store, the full provider catalog, the active id, the
/// active provider's cached credential (so a rebuild needs no keyring round-trip), and the thinking/
/// effort dials. Effort is captured at adapter construction, so changing effort — or the active
/// provider — means rebuilding the `Arc`. The credential is `None` during first-run onboarding (the
/// harness booted with no usable key); it is set the moment a provider is switched to or saved.
pub struct ProviderSwap {
    client: reqwest::Client,
    secrets: Box<dyn SecretStore>,
    providers: Vec<ProviderProfile>,
    active: String,
    credential: Option<Credential>,
    thinking: bool,
    effort: Effort,
}

impl ProviderSwap {
    pub fn new(
        client: reqwest::Client,
        secrets: Box<dyn SecretStore>,
        providers: Vec<ProviderProfile>,
        active: String,
        credential: Option<Credential>,
        thinking: bool,
        effort: Effort,
    ) -> Self {
        Self {
            client,
            secrets,
            providers,
            active,
            credential,
            thinking,
            effort,
        }
    }

    fn active_profile(&self) -> Option<&ProviderProfile> {
        self.providers.iter().find(|p| p.id == self.active)
    }

    fn active_profile_mut(&mut self) -> Option<&mut ProviderProfile> {
        let active = self.active.clone();
        self.providers.iter_mut().find(|p| p.id == active)
    }

    /// The configured provider ids, in catalog order — the `/provider` picker's options.
    pub fn provider_ids(&self) -> Vec<String> {
        self.providers.iter().map(|p| p.id.clone()).collect()
    }

    /// Build an adapter for a specific profile/credential/effort without committing any state, so a
    /// failed rebuild leaves the current provider untouched.
    fn build(
        &self,
        profile: &ProviderProfile,
        credential: &Credential,
        effort: Effort,
    ) -> Result<Arc<dyn CompletionProvider>, AgentError> {
        build_provider(
            self.client.clone(),
            profile,
            credential.clone(),
            self.thinking,
            effort,
        )
    }

    /// Resolve a provider's credential: the stored one if present, else an env-var key (the same
    /// migration/CI path as startup, e.g. `NVIDIA_API_KEY` / `ANTHROPIC_API_KEY` / `KIRI_<ID>_API_KEY`).
    /// Without this fallback a provider whose key lives only in an env var could not be switched to live.
    fn resolve_credential(&self, profile: &ProviderProfile) -> Result<Credential, AgentError> {
        if let Some(credential) = self.secrets.get(&profile.id)? {
            return Ok(credential);
        }
        if let Some(key) = api_key_from_env(profile) {
            let credential = Credential::ApiKey {
                key: Secret::new(key),
            };
            // Best-effort persist so a later switch needs no env var; a store failure is non-fatal —
            // the credential still works for this swap.
            let _ = self.secrets.set(&profile.id, &credential);
            return Ok(credential);
        }
        Err(AgentError::Provider(format!(
            "no credential for provider '{}'. Configure it via /provider or set its API-key env var.",
            profile.id
        )))
    }

    /// Rebuild the active provider with a new `effort`, committing the effort only on success. Without a
    /// live credential (first-run onboarding) there is nothing to rebuild, so it surfaces a clear error
    /// and leaves the effort dial untouched rather than panicking or silently diverging.
    fn rebuild_with_effort(
        &mut self,
        effort: Effort,
    ) -> Result<Arc<dyn CompletionProvider>, AgentError> {
        let Some(credential) = self.credential.clone() else {
            return Err(AgentError::Provider(
                "configure um provider com /provider antes de mudar o esforço".to_string(),
            ));
        };
        let profile = self
            .active_profile()
            .ok_or_else(|| AgentError::Provider("no active provider configured".to_string()))?;
        let provider = self.build(profile, &credential, effort)?;
        self.effort = effort;
        Ok(provider)
    }

    /// Switch the active provider to `id`: look up its profile + stored credential, build the adapter,
    /// and commit (active id + cached credential) only on success. Returns the new adapter and the
    /// target model id. An unknown id or a missing credential is a clear error.
    fn switch_to(&mut self, id: &str) -> Result<(Arc<dyn CompletionProvider>, String), AgentError> {
        let profile = self
            .providers
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| AgentError::Provider(format!("provider '{id}' is not configured")))?
            .clone();
        let credential = self.resolve_credential(&profile)?;
        let provider = self.build(&profile, &credential, self.effort)?;
        self.active = id.to_string();
        self.credential = Some(credential);
        Ok((provider, profile.model))
    }

    /// Store a new provider's credential, build its adapter, add-or-replace it in the catalog, and make
    /// it active — all committed only if the credential stores and the adapter builds. Returns the new
    /// adapter and its model.
    fn add_and_activate(
        &mut self,
        profile: ProviderProfile,
        credential: Credential,
    ) -> Result<(Arc<dyn CompletionProvider>, String), AgentError> {
        // Build first (validates the profile/credential), then store the secret — so a build failure
        // never leaves an orphaned credential in the keyring for a provider that was not added.
        let provider = self.build(&profile, &credential, self.effort)?;
        self.secrets.set(&profile.id, &credential)?;
        let id = profile.id.clone();
        let model = profile.model.clone();
        self.providers.retain(|p| p.id != id);
        self.providers.push(profile);
        self.active = id;
        self.credential = Some(credential);
        Ok((provider, model))
    }
}

/// The full-screen TUI frontend: owns the engine handles and the UI model, runs the render/input loop,
/// and drives one agent turn at a time. The sole frontend, assembled in `app::wire`.
pub struct Tui {
    agent_loop: AgentLoop,
    sandbox: Sandbox,
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
    /// Persists the conversation across runs. Inert (`is_available() == false`) when sessions are
    /// disabled or the store failed to initialize — every call is then a graceful no-op.
    session_store: Arc<dyn SessionStore>,
    /// The durable memory, used to drive the end-of-session distillation. Inert scopes make it a no-op.
    memory: Arc<dyn MemoryPort>,
    /// The workspace id sessions are keyed by; recomputed on `/cd`.
    project_id: String,
}

impl Tui {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        agent_loop: AgentLoop,
        sandbox: Sandbox,
        system_prompt: String,
        seed: Option<String>,
        provider_swap: ProviderSwap,
        config_path: PathBuf,
        needs_onboarding: bool,
        session_store: Arc<dyn SessionStore>,
        memory: Arc<dyn MemoryPort>,
        project_id: String,
    ) -> Self {
        let workspace = text::display_path(sandbox.root());
        let (model_id, models) = provider_swap
            .active_profile()
            .map(|p| (p.model.clone(), p.models.clone()))
            .unwrap_or_default();
        let mut model = Model::new(model_id, workspace)
            .with_provider_catalog(models, provider_swap.effort)
            .with_providers(provider_swap.active.clone(), provider_swap.provider_ids());
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
            session_store,
            memory,
            project_id,
        }
    }

    pub async fn run(self) -> Result<()> {
        let Tui {
            mut agent_loop,
            mut sandbox,
            mut conversation,
            mut model,
            seed,
            system_prompt,
            mut provider_swap,
            config_path,
            session_store,
            memory,
            mut project_id,
        } = self;

        // Session persistence cursor: the id of the row backing the current conversation (lazily created
        // on the first flush, so an empty session never hits the DB) and how many non-system messages
        // have already been written, so each flush appends only the new tail.
        let mut session_id: Option<String> = None;
        let mut persisted_len: usize = 0;

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
        model.motion = resolve_motion();
        // Stamp the open instant for the splash breath-in and the cursor pulse (clock stays out of the
        // domain constructor).
        model.opened_at = Some(Instant::now());

        let (engine_tx, mut engine_rx) = mpsc::unbounded_channel::<EngineMsg>();
        let cancel = CancelToken::new();
        let mut bridge = Bridge::new(engine_tx, cancel.clone());
        let mut pending_reply: Option<oneshot::Sender<Approval>> = None;
        let mut events = EventStream::new();
        let mut ticker = time::interval(FRAME_INTERVAL);

        // An initial prompt from the CLI runs as the first turn.
        if let Some(line) = seed.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
            match command::parse(&line) {
                Some(Command::Quit) => model.should_quit = true,
                // A non-quit command as the CLI seed is ignored; the seed is meant to be a prompt.
                Some(_) => {}
                // Onboarding: there is no usable provider yet, so the seed can't run against the null
                // provider. Surface it and let the user configure a provider via the welcome wizard.
                None if model.unconfigured => {
                    model.transcript.push(TranscriptItem::Notice(
                        NoticeLevel::Info,
                        "configure um provider antes de enviar — a mensagem inicial foi ignorada"
                            .to_string(),
                    ));
                }
                None => {
                    model.history.record(&line);
                    model.transcript.push(TranscriptItem::User(line.clone()));
                    model.busy = true;
                    conversation.push(Message::user(line));
                    drive_turn(
                        &agent_loop,
                        &mut conversation,
                        &sandbox,
                        &mut bridge,
                        &mut model,
                        &mut engine_rx,
                        &cancel,
                        &mut pending_reply,
                        &mut terminal,
                        &mut events,
                        &mut ticker,
                    )
                    .await?;
                    flush_session(
                        session_store.as_ref(),
                        &mut session_id,
                        &mut persisted_len,
                        &project_id,
                        &conversation,
                        &mut model,
                    )
                    .await;
                }
            }
        }

        while !model.should_quit {
            model.render_at = Some(Instant::now());
            draw_and_copy(&mut terminal, &mut model)?;

            // Resolve one input into a message, then handle it outside the select so the engine
            // handles are unambiguously free when a turn is armed.
            let msg = tokio::select! {
                biased;
                maybe = events.next() => match maybe {
                    Some(Ok(event)) => {
                        // Stamp arrival time for multi-click detection (before the reducer reads it).
                        model.last_event_at = Some(Instant::now());
                        input::to_msg(event)
                    }
                    Some(Err(_)) => None,
                    None => {
                        model.should_quit = true;
                        None
                    }
                },
                _ = ticker.tick() => None,
            };
            let Some(msg) = msg else {
                continue;
            };

            for effect in update(&mut model, msg) {
                match effect {
                    Effect::SubmitPrompt { text, images } => {
                        let message = if images.is_empty() {
                            Message::user(text)
                        } else {
                            Message::user_multimodal(text, images)
                        };
                        conversation.push(message);
                        drive_turn(
                            &agent_loop,
                            &mut conversation,
                            &sandbox,
                            &mut bridge,
                            &mut model,
                            &mut engine_rx,
                            &cancel,
                            &mut pending_reply,
                            &mut terminal,
                            &mut events,
                            &mut ticker,
                        )
                        .await?;
                        flush_session(
                            session_store.as_ref(),
                            &mut session_id,
                            &mut persisted_len,
                            &project_id,
                            &conversation,
                            &mut model,
                        )
                        .await;
                    }
                    Effect::CopyToClipboard(text) => copy_to_clipboard(&mut model, &text),
                    Effect::PasteClipboard => paste_from_clipboard(&mut model),
                    Effect::PlaceCursor { col, row } => {
                        place_cursor(&mut model, &terminal, col, row)
                    }
                    Effect::Quit => model.should_quit = true,
                    Effect::NewSession => {
                        // Learn from the session being discarded before it is gone.
                        drive_distillation(
                            &agent_loop,
                            &memory,
                            &project_id,
                            &conversation,
                            &mut model,
                            &mut terminal,
                            &mut events,
                            &mut ticker,
                        )
                        .await;
                        conversation = Conversation::new(system_prompt.clone());
                        // Detach from the persisted row: the next turn lazily creates a fresh session.
                        session_id = None;
                        persisted_len = 0;
                        model.transcript = Transcript::default();
                        model.attachments.clear();
                        model.scroll.pin();
                        model.transcript.push(TranscriptItem::Notice(
                            NoticeLevel::Info,
                            "nova sessão".to_string(),
                        ));
                    }
                    Effect::ListSessions => {
                        list_sessions(session_store.as_ref(), &project_id, &mut model).await;
                    }
                    Effect::SyncPush => {
                        sync_push(&config_path, &mut model, &mut terminal).await;
                    }
                    Effect::ResumeLast => {
                        // On an inert store (init never ran, so no `sessions` table) latest_for_project
                        // raises a raw "no such table" error; guard it the same way /sessions does so the
                        // user sees the clean degraded-mode notice, not a leaked SQL detail.
                        if !session_store.is_available() {
                            model.transcript.push(TranscriptItem::Notice(
                                NoticeLevel::Info,
                                "persistência de sessão indisponível".to_string(),
                            ));
                        } else {
                            match session_store.latest_for_project(&project_id).await {
                                Ok(Some(summary)) => {
                                    // Learn from the current session before switching away from it.
                                    drive_distillation(
                                        &agent_loop,
                                        &memory,
                                        &project_id,
                                        &conversation,
                                        &mut model,
                                        &mut terminal,
                                        &mut events,
                                        &mut ticker,
                                    )
                                    .await;
                                    open_session(
                                        session_store.as_ref(),
                                        &system_prompt,
                                        &mut conversation,
                                        &mut model,
                                        &mut session_id,
                                        &mut persisted_len,
                                        &project_id,
                                        &summary.id,
                                    )
                                    .await;
                                }
                                Ok(None) => model.transcript.push(TranscriptItem::Notice(
                                    NoticeLevel::Info,
                                    "nenhuma sessão anterior neste workspace".to_string(),
                                )),
                                Err(error) => model.transcript.push(TranscriptItem::Notice(
                                    NoticeLevel::Error,
                                    format!("não foi possível ler as sessões: {error}"),
                                )),
                            }
                        }
                    }
                    Effect::OpenSession(id) => {
                        // Learn from the current session before switching away from it.
                        drive_distillation(
                            &agent_loop,
                            &memory,
                            &project_id,
                            &conversation,
                            &mut model,
                            &mut terminal,
                            &mut events,
                            &mut ticker,
                        )
                        .await;
                        open_session(
                            session_store.as_ref(),
                            &system_prompt,
                            &mut conversation,
                            &mut model,
                            &mut session_id,
                            &mut persisted_len,
                            &project_id,
                            &id,
                        )
                        .await;
                    }
                    Effect::ChangeWorkspace(path) => match sandbox.relocated(&path) {
                        Ok(new_sandbox) => {
                            // Learn from the old project's session before re-keying to the new workspace.
                            drive_distillation(
                                &agent_loop,
                                &memory,
                                &project_id,
                                &conversation,
                                &mut model,
                                &mut terminal,
                                &mut events,
                                &mut ticker,
                            )
                            .await;
                            model.status.workspace = text::display_path(new_sandbox.root());
                            // Sessions are keyed by workspace: the current one belongs to the old
                            // project, so detach and re-key. The next turn starts a fresh session under
                            // the new project_id.
                            let root = new_sandbox.root();
                            let canonical_root =
                                root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
                            project_id = project_id_from_path(&canonical_root);
                            session_id = None;
                            persisted_len = 0;
                            sandbox = new_sandbox;
                            model.transcript.push(TranscriptItem::Notice(
                                NoticeLevel::Info,
                                format!("workspace: {}", model.status.workspace),
                            ));
                        }
                        Err(error) => model.transcript.push(TranscriptItem::Notice(
                            NoticeLevel::Error,
                            format!("erro: {error:#}"),
                        )),
                    },
                    Effect::ApprovePlan(mode) => {
                        model.approval_mode = mode;
                        let notice = if mode == ApprovalMode::Auto {
                            "▶ executando o plano (auto)"
                        } else {
                            "▶ executando o plano"
                        };
                        model.transcript.push(TranscriptItem::Notice(
                            NoticeLevel::Info,
                            notice.to_string(),
                        ));
                        model.busy = true;
                        conversation.push(Message::user(
                            "Plano aprovado. Prossiga com a execução.".to_string(),
                        ));
                        drive_turn(
                            &agent_loop,
                            &mut conversation,
                            &sandbox,
                            &mut bridge,
                            &mut model,
                            &mut engine_rx,
                            &cancel,
                            &mut pending_reply,
                            &mut terminal,
                            &mut events,
                            &mut ticker,
                        )
                        .await?;
                        flush_session(
                            session_store.as_ref(),
                            &mut session_id,
                            &mut persisted_len,
                            &project_id,
                            &conversation,
                            &mut model,
                        )
                        .await;
                    }
                    Effect::SetModel(model_id) => {
                        // A model change is just the per-turn `model` field — no provider rebuild. Apply
                        // it live, reflect it in the status line, and persist (best-effort) to the global
                        // config; a write failure is surfaced but the live change stands.
                        if let Some(profile) = provider_swap.active_profile_mut() {
                            profile.model = model_id.clone();
                        }
                        agent_loop.set_model(model_id.clone());
                        model.status.model = model_id.clone();
                        model.transcript.push(TranscriptItem::Notice(
                            NoticeLevel::Info,
                            format!("modelo: {model_id}"),
                        ));
                        if let Err(error) = config::persist_active_model(
                            &config_path,
                            &provider_swap.active,
                            &model_id,
                        ) {
                            model.transcript.push(TranscriptItem::Notice(
                                NoticeLevel::Error,
                                format!("não persistiu o modelo: {error:#}"),
                            ));
                        }
                    }
                    Effect::SetEffort(effort) => {
                        // Effort is baked into the provider at construction, so rebuild and swap it in.
                        // Build with the new effort first; commit (status + cached effort + persist) only
                        // if the rebuild succeeds, so a failure leaves the current provider untouched.
                        let is_anthropic = provider_swap.active_profile().map(|p| p.kind)
                            == Some(ProviderKind::Anthropic);
                        match provider_swap.rebuild_with_effort(effort) {
                            Ok(provider) => {
                                agent_loop.set_provider(provider);
                                model.status.effort = effort;
                                // The Anthropic adapter ignores effort today — surface that rather than
                                // silently appearing to change nothing.
                                let note = if is_anthropic {
                                    format!(
                                        "esforço: {} — nota: ainda não afeta modelos Claude",
                                        effort.label()
                                    )
                                } else {
                                    format!("esforço: {}", effort.label())
                                };
                                model
                                    .transcript
                                    .push(TranscriptItem::Notice(NoticeLevel::Info, note));
                                if let Err(error) = config::persist_effort(&config_path, effort) {
                                    model.transcript.push(TranscriptItem::Notice(
                                        NoticeLevel::Error,
                                        format!("não persistiu o esforço: {error:#}"),
                                    ));
                                }
                            }
                            Err(error) => model.transcript.push(TranscriptItem::Notice(
                                NoticeLevel::Error,
                                format!("não foi possível aplicar o esforço: {error:#}"),
                            )),
                        }
                    }
                    Effect::SetProvider(id) => {
                        // Switch the active provider: rebuild its adapter with its stored credential and
                        // swap it in, also adopting its model. Commit + persist only on success; a missing
                        // credential or unknown id is surfaced, never a silent no-op.
                        match provider_swap.switch_to(&id) {
                            Ok((provider, target_model)) => {
                                agent_loop.set_provider(provider);
                                agent_loop.set_model(target_model.clone());
                                model.status.model = target_model.clone();
                                model.status.provider = id.clone();
                                model.models = provider_swap
                                    .active_profile()
                                    .map(|p| p.models.clone())
                                    .unwrap_or_default();
                                model.transcript.push(TranscriptItem::Notice(
                                    NoticeLevel::Info,
                                    format!("provider: {id} ({target_model})"),
                                ));
                                if let Err(error) =
                                    config::persist_active_provider(&config_path, &id)
                                {
                                    model.transcript.push(TranscriptItem::Notice(
                                        NoticeLevel::Error,
                                        format!("não persistiu o provider ativo: {error:#}"),
                                    ));
                                }
                            }
                            Err(error) => model.transcript.push(TranscriptItem::Notice(
                                NoticeLevel::Error,
                                format!("não foi possível trocar de provider: {error:#}"),
                            )),
                        }
                    }
                    Effect::SaveProvider {
                        id,
                        kind,
                        base_url,
                        model: model_id,
                        models,
                    } => {
                        // The wizard staged the typed key as a Secret out of the effect; take it here.
                        let Some(key) = model.pending_credential.take() else {
                            model.transcript.push(TranscriptItem::Notice(
                                NoticeLevel::Error,
                                "chave ausente; provider não foi salvo".to_string(),
                            ));
                            continue;
                        };
                        let credential = Credential::ApiKey { key };
                        let profile = ProviderProfile {
                            id: id.clone(),
                            kind,
                            base_url,
                            model: model_id.clone(),
                            models: models.clone(),
                            auth: AuthMethod::ApiKey,
                        };
                        match provider_swap.add_and_activate(profile.clone(), credential) {
                            Ok((provider, target_model)) => {
                                agent_loop.set_provider(provider);
                                agent_loop.set_model(target_model.clone());
                                // Onboarding (or a re-add) succeeded: a real adapter is live, so drop the
                                // submit gate and let the user into the normal chat.
                                model.unconfigured = false;
                                model.status.model = target_model;
                                model.status.provider = id.clone();
                                model.models = models;
                                model.providers = provider_swap.provider_ids();
                                model.transcript.push(TranscriptItem::Notice(
                                    NoticeLevel::Info,
                                    format!("provider '{id}' adicionado e ativo"),
                                ));
                                // Persist the profile (config) and the active selection; the credential
                                // already went to the keyring above.
                                if let Err(error) = config::upsert_provider(&config_path, &profile)
                                    .and_then(|()| {
                                        config::persist_active_provider(&config_path, &id)
                                    })
                                {
                                    model.transcript.push(TranscriptItem::Notice(
                                        NoticeLevel::Error,
                                        format!(
                                            "provider ativo, mas não persistiu no config: {error:#}"
                                        ),
                                    ));
                                }
                            }
                            Err(error) => model.transcript.push(TranscriptItem::Notice(
                                NoticeLevel::Error,
                                format!("não foi possível salvar o provider: {error:#}"),
                            )),
                        }
                    }
                    Effect::AnswerApproval(_) | Effect::CancelTurn => {}
                }
            }
        }

        // Distill the final session before tearing down, so the last conversation also teaches the
        // memory. Best-effort, bounded, and Ctrl+C-skippable — quit is never held hostage.
        drive_distillation(
            &agent_loop,
            &memory,
            &project_id,
            &conversation,
            &mut model,
            &mut terminal,
            &mut events,
            &mut ticker,
        )
        .await;

        Ok(())
    }
}

/// Draw a frame and, if a copy was requested, scrape the just-rendered selection to the OS clipboard.
/// The caller stamps `model.render_at` first (so line landings share the frame instant). Returns the
/// draw error so each caller chooses how to handle it (the main loop propagates; the turn loop must
/// break, never `?`, so its cleanup still runs).
fn draw_and_copy(terminal: &mut DefaultTerminal, model: &mut Model) -> io::Result<()> {
    // Lift the pending copy out first (ScreenSelection is `Copy`), so no `&model` borrow is held across
    // the draw and the post-draw mutation below type-checks.
    let pending = model.selection.filter(|s| s.state != SelectionState::Idle);
    let completed = terminal.draw(|frame| view(model, frame))?;
    if let Some(sel) = pending {
        // `completed` borrows the terminal, not the model, so scraping it and then mutating the model
        // below is disjoint — no explicit drop needed.
        let text = selection_overlay::scrape(completed.buffer, &sel, completed.area);
        copy_to_clipboard(model, &text);
        // Mouse-release keeps the highlight (just settle the state); Ctrl+C drops it so the next Ctrl+C
        // is free to cancel/quit. Either way the request is consumed exactly once.
        match sel.state {
            SelectionState::CopyAndClear => model.selection = None,
            _ => {
                if let Some(s) = model.selection.as_mut() {
                    s.state = SelectionState::Idle;
                }
            }
        }
    }
    Ok(())
}

/// Copy text to the OS clipboard, surfacing a failure as a transcript notice — copy is a direct user
/// intent, so it must never fail silently. An empty text is a no-op (the clipboard is left untouched).
fn copy_to_clipboard(model: &mut Model, text: &str) {
    if let Err(error) = clipboard::copy_text(text) {
        model.transcript.push(TranscriptItem::Notice(
            NoticeLevel::Error,
            format!("falha ao copiar para a área de transferência: {error}"),
        ));
    }
}

/// Resolve a composer click to a logical cursor move, against the freshly rendered geometry. The runtime
/// owns the only honest source of the editor's rect — it recomputes it from the current terminal size and
/// model, exactly as the last frame did. A click outside the box, or a wrapped/scrolled layout the widget
/// renders ambiguously, resolves to `None` and leaves the cursor put (never mis-placed).
fn place_cursor<B: Backend>(model: &mut Model, terminal: &Terminal<B>, col: u16, row: u16) {
    // Without the terminal size the geometry is unknown, so the click cannot be mapped — a safe no-op
    // (the user can still navigate by key); nothing actionable is dropped silently.
    let Ok(size) = terminal.size() else { return };
    let area = Rect::new(0, 0, size.width, size.height);
    let editor_area = editor::content_rect(frame_regions(area, model).input);
    if let Some((r, c)) = editor::click_to_cursor(&model.input, editor_area, col, row) {
        model.input.set_cursor(r, c);
    }
}

/// Read the OS clipboard and route it into the buffer: an image becomes a staged attachment, text is
/// inserted at the cursor. Best-effort — an empty or unreadable clipboard is a no-op.
fn paste_from_clipboard(model: &mut Model) {
    // `update` for these messages produces no effects (they only mutate the model), so the returned
    // Vec is intentionally discarded — there is nothing for the runtime to perform.
    match clipboard::read() {
        ClipboardContent::Image(attachment) => {
            let _ = update(model, Msg::ImageAttached(attachment));
        }
        ClipboardContent::Text(text) => {
            let _ = update(model, Msg::Paste(text));
        }
        ClipboardContent::Empty => {}
    }
}

/// One step the turn loop's `select!` produced.
enum Step {
    Done(Result<TurnOutcome, AgentError>),
    Apply(Msg),
    Idle,
}

/// Whether applying `msg` must force an immediate redraw. Stream deltas and the periodic tick are
/// throttled to at most one draw per `FRAME_INTERVAL`, so a burst of tokens coalesces into a single
/// re-render; every structural change (tool lines, approvals, turn boundaries, user input) draws at
/// once for responsiveness.
fn forces_draw(msg: &Msg) -> bool {
    !matches!(msg, Msg::StreamDelta(..) | Msg::Tick)
}

/// The spinner frame index for an elapsed time: one step per `FRAME_INTERVAL`. Wrapping into the glyph
/// table is the renderer's job (`% SPINNER.len()`). Pure, so the animation cadence is unit-testable and
/// is driven by wall clock rather than message arrival.
fn spinner_frame(elapsed: Duration) -> usize {
    (elapsed.as_millis() / FRAME_INTERVAL.as_millis()) as usize
}

/// Resolve the session-wide motion preference from the environment: any non-empty `KIRI_REDUCED_MOTION`
/// or `NO_COLOR` freezes motion to a steady, layout-identical UI; otherwise it is fully expressed.
fn resolve_motion() -> Motion {
    let set = |key: &str| std::env::var_os(key).is_some_and(|v| !v.is_empty());
    if set("KIRI_REDUCED_MOTION") || set("NO_COLOR") {
        Motion::Reduced
    } else {
        Motion::Full
    }
}

/// Drive one agent turn to completion while keeping the UI live: stream deltas render, approvals show
/// a prompt, and ^C cancels cooperatively. The agent future borrows `conversation`/`sandbox`/`bridge`
/// only inside the inner block, so the caller may start another turn afterward.
#[allow(clippy::too_many_arguments)]
async fn drive_turn(
    agent_loop: &AgentLoop,
    conversation: &mut Conversation,
    sandbox: &Sandbox,
    bridge: &mut Bridge,
    model: &mut Model,
    engine_rx: &mut mpsc::UnboundedReceiver<EngineMsg>,
    cancel: &CancelToken,
    pending_reply: &mut Option<oneshot::Sender<Approval>>,
    terminal: &mut DefaultTerminal,
    events: &mut EventStream,
    ticker: &mut Interval,
) -> Result<()> {
    cancel.reset();
    let started = Instant::now();
    // The approval mode is fixed for this turn; cycling it mid-turn applies to the next one.
    let mode = model.approval_mode;

    let result = {
        let mut turn: TurnFuture = Box::pin(agent_loop.run(conversation, sandbox, mode, bridge));
        let mut last_draw = Instant::now();
        loop {
            let step = tokio::select! {
                biased;
                maybe = events.next() => match maybe {
                    Some(Ok(event)) => {
                        model.last_event_at = Some(Instant::now());
                        input::to_msg(event).map(Step::Apply).unwrap_or(Step::Idle)
                    }
                    _ => Step::Idle,
                },
                Some(engine) = engine_rx.recv() => Step::Apply(engine_msg(engine, pending_reply)),
                _ = ticker.tick() => Step::Apply(Msg::Tick),
                outcome = &mut turn => Step::Done(outcome),
            };

            // Stamp the frame before applying the step, so line landings (in `update`) and the draw that
            // shows them share one instant — a freshly landed line starts at age zero (forge-warm).
            model.render_at = Some(Instant::now());

            let mut done: Option<_> = None;
            // Forced steps redraw immediately; throttled ones (stream deltas, ticks) wait for the frame.
            let mut force = false;
            match step {
                Step::Done(outcome) => {
                    done = Some(outcome);
                    force = true;
                }
                Step::Idle => {}
                Step::Apply(msg) => {
                    force = forces_draw(&msg);
                    for effect in update(model, msg) {
                        match effect {
                            Effect::AnswerApproval(decision) => {
                                if let Some(reply) = pending_reply.take() {
                                    // Best-effort: the engine awaits this reply, but if the turn future
                                    // was already dropped (cancel/quit) the receiver is gone — a failed
                                    // send is then expected and harmless.
                                    let _ = reply.send(decision);
                                }
                            }
                            Effect::CancelTurn => {
                                cancel.cancel();
                                // Break the select! loop immediately — dropping the turn future
                                // kills any running child process (kill_on_drop on run_command).
                                done = Some(Ok(TurnOutcome::Aborted));
                                force = true;
                            }
                            Effect::Quit => {
                                model.should_quit = true;
                                cancel.cancel();
                                done = Some(Ok(TurnOutcome::Aborted));
                                force = true;
                            }
                            // Clipboard chords stay live during a turn (composing the next prompt).
                            Effect::CopyToClipboard(text) => copy_to_clipboard(model, &text),
                            Effect::PasteClipboard => paste_from_clipboard(model),
                            Effect::PlaceCursor { col, row } => {
                                place_cursor(model, terminal, col, row)
                            }
                            // A picker/wizard cannot open mid-turn, so these never arrive here.
                            Effect::SubmitPrompt { .. }
                            | Effect::NewSession
                            | Effect::ResumeLast
                            | Effect::ListSessions
                            | Effect::OpenSession(_)
                            | Effect::SyncPush
                            | Effect::ChangeWorkspace(_)
                            | Effect::ApprovePlan(_)
                            | Effect::SetModel(_)
                            | Effect::SetEffort(_)
                            | Effect::SetProvider(_)
                            | Effect::SaveProvider { .. } => {}
                        }
                    }
                    // Coalesce a burst: drain every engine message already queued before drawing, so
                    // many tokens that arrived together become one re-render instead of one per token.
                    // These messages only mutate the model (no effects); a structural one among them
                    // (tool line, approval, turn boundary) still forces an immediate draw.
                    while let Ok(engine) = engine_rx.try_recv() {
                        let queued = engine_msg(engine, pending_reply);
                        force |= forces_draw(&queued);
                        let _ = update(model, queued);
                    }
                }
            }
            model.status.elapsed_secs = started.elapsed().as_secs();
            // The spinner animates by wall clock, so its rate is independent of message cadence — it
            // spins during the wait for the first token and during tool execution, not only while
            // content streams.
            model.status.spinner_frame = spinner_frame(started.elapsed());
            // Draw on a forced step or once the stream-frame budget elapsed. Incoming deltas pace the
            // redraws at ~30 fps (smooth, no coarse blocks), while the 120ms ticker still guarantees a
            // periodic draw during a quiet wait. Coalescing keeps it to one transcript re-render per
            // frame rather than one per token (the cause of the lag).
            // A draw failure must NOT `?`-propagate out of this loop: that would skip `on_turn_end` and
            // leave `model.busy` stuck true, silently deadening every future submit. End the turn with
            // the error instead, so cleanup always runs.
            if force || last_draw.elapsed() >= STREAM_FRAME {
                // break-not-`?`: a draw failure must still run `on_turn_end` so `busy` resets.
                if let Err(error) = draw_and_copy(terminal, model) {
                    break Err(AgentError::Io(error));
                }
                last_draw = Instant::now();
            }
            if let Some(outcome) = done {
                break outcome;
            }
        }
    };

    // Drain any deltas/notices buffered when the turn future resolved, so nothing is lost. These
    // messages only mutate the model (no effects), so the returned Vec is intentionally discarded.
    while let Ok(engine) = engine_rx.try_recv() {
        let _ = update(model, engine_msg(engine, pending_reply));
    }

    let cancelled = cancel.is_cancelled();
    on_turn_end(result, cancelled, model, conversation);
    cancel.reset();
    *pending_reply = None;
    Ok(())
}

/// Persist the conversation's new tail to the session store, lazily creating the session row on the first
/// non-empty flush (so an empty session never touches the DB). The system message (index 0) is never
/// stored — it is regenerated per run from the current memory digest. Best-effort: an unavailable store is
/// a silent no-op, and a write failure surfaces a Notice without ever losing the in-memory conversation.
/// Called after a turn settles (post-rollback), so the DB mirrors the resumable in-memory state.
async fn flush_session(
    store: &dyn SessionStore,
    session_id: &mut Option<String>,
    persisted_len: &mut usize,
    project_id: &str,
    conversation: &Conversation,
    model: &mut Model,
) {
    if !store.is_available() {
        return;
    }
    let messages = conversation.messages();
    // The body excludes the system seed at index 0.
    let body = &messages[1..];
    // Clamp the cursor: a rollback can shrink the body below it. In practice the rolled-back messages
    // were never persisted (we only flush after a turn settles), so this only guards against a panic.
    let cursor = (*persisted_len).min(body.len());
    if body.len() <= cursor {
        return;
    }
    let delta = &body[cursor..];

    let id = match session_id {
        Some(id) => id.clone(),
        None => match store.create(project_id).await {
            Ok(session) => {
                *session_id = Some(session.id.clone());
                session.id
            }
            Err(error) => {
                model.transcript.push(TranscriptItem::Notice(
                    NoticeLevel::Error,
                    format!("não persistiu a sessão: {error}"),
                ));
                return;
            }
        },
    };

    let first_flush = cursor == 0;
    if let Err(error) = store.append_messages(&id, delta).await {
        model.transcript.push(TranscriptItem::Notice(
            NoticeLevel::Error,
            format!("não persistiu a sessão: {error}"),
        ));
        return;
    }
    if first_flush
        && let Some(title_source) = body
            .iter()
            .find(|m| m.role == Role::User)
            .and_then(|m| m.content.as_deref())
    {
        // Title is cosmetic (the `/sessions` label); a failure must not fail the flush, and the messages
        // are already saved, so a derive/store failure is safely ignored.
        let _ = store.set_title(&id, &derive_title(title_source)).await;
    }
    *persisted_len = body.len();
}

/// How many recent sessions the `/sessions` picker lists.
const SESSION_LIST_LIMIT: usize = 20;

/// Trim an RFC3339 timestamp to `YYYY-MM-DD HH:MM` for the compact session-list label.
fn short_timestamp(raw: &str) -> String {
    raw.get(..16).unwrap_or(raw).replace('T', " ")
}

/// Query the workspace's recent sessions and open the `/sessions` picker, recording the parallel id list
/// the keymap resolves against. An unavailable store or an empty list surfaces a Notice and opens nothing.
async fn list_sessions(store: &dyn SessionStore, project_id: &str, model: &mut Model) {
    if !store.is_available() {
        model.transcript.push(TranscriptItem::Notice(
            NoticeLevel::Info,
            "persistência de sessão indisponível".to_string(),
        ));
        return;
    }
    match store.list_for_project(project_id, SESSION_LIST_LIMIT).await {
        Ok(sessions) if !sessions.is_empty() => {
            model.session_ids = sessions.iter().map(|s| s.id.clone()).collect();
            let options = sessions
                .iter()
                .map(|s| {
                    let title = if s.title.trim().is_empty() {
                        "(sem título)"
                    } else {
                        s.title.trim()
                    };
                    format!(
                        "{title} · {} · {} msgs",
                        short_timestamp(&s.updated_at),
                        s.message_count
                    )
                })
                .collect();
            model.picker = Some(Picker::new(
                PickerKind::Sessions,
                "sessão",
                "Escolha uma sessão para retomar:",
                options,
                0,
            ));
        }
        Ok(_) => model.transcript.push(TranscriptItem::Notice(
            NoticeLevel::Info,
            "nenhuma sessão anterior neste workspace".to_string(),
        )),
        Err(error) => model.transcript.push(TranscriptItem::Notice(
            NoticeLevel::Error,
            format!("não foi possível listar as sessões: {error}"),
        )),
    }
}

/// Finalize the current session, then load `target_id` and rebuild the conversation and transcript from
/// it. The system prompt is the current one (a fresh memory digest), correct because stored messages
/// exclude the system seed. A missing/failed load surfaces a Notice and leaves the current state intact.
#[allow(clippy::too_many_arguments)]
async fn open_session(
    store: &dyn SessionStore,
    system_prompt: &str,
    conversation: &mut Conversation,
    model: &mut Model,
    session_id: &mut Option<String>,
    persisted_len: &mut usize,
    project_id: &str,
    target_id: &str,
) {
    // Persist the current session's tail before switching away, so nothing is lost.
    flush_session(
        store,
        session_id,
        persisted_len,
        project_id,
        conversation,
        model,
    )
    .await;
    match store.load(target_id).await {
        Ok(Some(session)) => {
            let mut fresh = Conversation::new(system_prompt.to_string());
            for message in &session.messages {
                fresh.push(message.clone());
            }
            model.transcript = rebuild_transcript(&session.messages);
            model.attachments.clear();
            model.scroll.pin();
            let title = if session.title.trim().is_empty() {
                "(sem título)".to_string()
            } else {
                session.title.clone()
            };
            model.transcript.push(TranscriptItem::Notice(
                NoticeLevel::Info,
                format!("sessão retomada: {title}"),
            ));
            *conversation = fresh;
            *session_id = Some(session.id);
            *persisted_len = session.messages.len();
        }
        Ok(None) => model.transcript.push(TranscriptItem::Notice(
            NoticeLevel::Error,
            "sessão não encontrada".to_string(),
        )),
        Err(error) => model.transcript.push(TranscriptItem::Notice(
            NoticeLevel::Error,
            format!("não foi possível abrir a sessão: {error}"),
        )),
    }
}

/// Project a loaded conversation back into a display transcript: user and assistant text become their
/// items; an assistant turn that only called tools becomes a compact notice; tool results and the system
/// seed are omitted (verbose / never stored). A render-only projection — the conversation stays the
/// source of truth.
fn rebuild_transcript(messages: &[Message]) -> Transcript {
    let mut transcript = Transcript::default();
    for message in messages {
        match message.role {
            Role::User => {
                if let Some(content) = message.content.as_deref().filter(|c| !c.trim().is_empty()) {
                    transcript.push(TranscriptItem::User(content.to_string()));
                }
            }
            Role::Assistant => {
                if let Some(content) = message.content.as_deref().filter(|c| !c.trim().is_empty()) {
                    transcript.push(TranscriptItem::Assistant(content.to_string()));
                } else if !message.tool_calls.is_empty() {
                    transcript.push(TranscriptItem::Notice(
                        NoticeLevel::Info,
                        format!("· {} ferramenta(s) executada(s)", message.tool_calls.len()),
                    ));
                }
            }
            Role::Tool | Role::System => {}
        }
    }
    transcript
}

/// Push the portable profile (config + shared memory) to the configured private repo via `/sync`. Shows
/// a "syncing" notice and draws it before the (network-bound, timeout-bounded) push, then reports the
/// result. The global dir is derived from the global config path (`~/.kiri/config.toml`).
async fn sync_push(config_path: &Path, model: &mut Model, terminal: &mut DefaultTerminal) {
    let Some(global_dir) = config_path.parent().map(Path::to_path_buf) else {
        model.transcript.push(TranscriptItem::Notice(
            NoticeLevel::Error,
            "caminho de config inválido para sync".to_string(),
        ));
        return;
    };
    model.transcript.push(TranscriptItem::Notice(
        NoticeLevel::Info,
        "sincronizando (push)…".to_string(),
    ));
    model.render_at = Some(Instant::now());
    let _ = draw_and_copy(terminal, model);

    let shared_db = global_dir.join("memory").join("shared.db");
    // Open a handle to the shared store and inject it as the port. A store failure surfaces as a
    // Notice rather than aborting the session.
    let memory = match SqliteSharedMemory::new(shared_db) {
        Ok(store) => match store.init().await {
            Ok(()) => store,
            Err(error) => {
                model.transcript.push(TranscriptItem::Notice(
                    NoticeLevel::Error,
                    format!("sync falhou: {error}"),
                ));
                return;
            }
        },
        Err(error) => {
            model.transcript.push(TranscriptItem::Notice(
                NoticeLevel::Error,
                format!("sync falhou: {error}"),
            ));
            return;
        }
    };
    let git = GitCli;
    let service = SyncService::new(&git, global_dir, config_path.to_path_buf(), &memory);
    match service.push().await {
        Ok(summary) => model.transcript.push(TranscriptItem::Notice(
            NoticeLevel::Info,
            format!("sync: {summary}"),
        )),
        Err(error) => model.transcript.push(TranscriptItem::Notice(
            NoticeLevel::Error,
            format!("sync falhou: {error}"),
        )),
    }
}

/// Whether a session is worth distilling: it must hold at least one user message and one non-empty
/// assistant reply, so an empty or aborted session never spends an LLM call on noise.
fn should_distill(conversation: &Conversation) -> bool {
    let mut has_user = false;
    let mut has_assistant = false;
    for message in conversation.messages() {
        match message.role {
            Role::User => has_user = true,
            Role::Assistant
                if message
                    .content
                    .as_deref()
                    .is_some_and(|c| !c.trim().is_empty()) =>
            {
                has_assistant = true
            }
            _ => {}
        }
    }
    has_user && has_assistant
}

/// Whether a crossterm event is Ctrl+C — the skip key during distillation.
fn is_ctrl_c(event: &Event) -> bool {
    matches!(event, Event::Key(key)
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
}

/// One step the distillation `select!` produced.
enum DistillStep {
    Done(Result<crate::modules::memory::application::distill::DistillReport, AgentError>),
    Skip,
    Tick,
}

/// Run the end-of-session distillation while keeping the UI responsive: a spinner ticks and Ctrl+C skips.
/// Best-effort and bounded — the distiller's own timeout caps the wait, a skip or failure surfaces a
/// Notice and never blocks the caller (a `/new`, a session switch, or quit). The conversation is read
/// only and already persisted, so distillation never risks the session's data.
#[allow(clippy::too_many_arguments)]
async fn drive_distillation(
    agent_loop: &AgentLoop,
    memory: &Arc<dyn MemoryPort>,
    project_id: &str,
    conversation: &Conversation,
    model: &mut Model,
    terminal: &mut DefaultTerminal,
    events: &mut EventStream,
    ticker: &mut Interval,
) {
    if !should_distill(conversation) {
        return;
    }
    // Both scopes inert (memory disabled or failed): there is nothing to write to, so skip the LLM call.
    if !memory.project_memory_available() && !memory.shared_memory_available() {
        return;
    }

    let provider = agent_loop.provider();
    let model_id = agent_loop.model().to_string();
    let distiller = Distiller::new(memory.clone(), project_id.to_string());
    let messages: Vec<Message> = conversation.messages().to_vec();

    model.transcript.push(TranscriptItem::Notice(
        NoticeLevel::Info,
        "destilando memórias da sessão… (^C pula)".to_string(),
    ));
    model.busy = true;
    let started = Instant::now();
    model.render_at = Some(started);
    let _ = draw_and_copy(terminal, model);

    let outcome = {
        let mut distillation = Box::pin(distiller.distill(provider.as_ref(), &model_id, &messages));
        loop {
            let step = tokio::select! {
                biased;
                maybe = events.next() => match maybe {
                    Some(Ok(event)) if is_ctrl_c(&event) => DistillStep::Skip,
                    // Other input is ignored during the (brief) distillation.
                    _ => DistillStep::Tick,
                },
                _ = ticker.tick() => DistillStep::Tick,
                done = &mut distillation => DistillStep::Done(done),
            };
            match step {
                DistillStep::Done(result) => break Some(result),
                DistillStep::Skip => break None,
                DistillStep::Tick => {
                    model.status.spinner_frame = spinner_frame(started.elapsed());
                    model.render_at = Some(Instant::now());
                    // A draw failure ends the best-effort distillation rather than looping blind.
                    if draw_and_copy(terminal, model).is_err() {
                        break None;
                    }
                }
            }
        }
    };

    model.busy = false;
    match outcome {
        None => model.transcript.push(TranscriptItem::Notice(
            NoticeLevel::Info,
            "destilação pulada".to_string(),
        )),
        Some(Ok(report)) if report.written > 0 => model.transcript.push(TranscriptItem::Notice(
            NoticeLevel::Info,
            format!("memória atualizada: {} aprendizado(s)", report.written),
        )),
        // Nothing worth keeping: stay quiet rather than add noise on every /new.
        Some(Ok(_)) => {}
        Some(Err(error)) => model.transcript.push(TranscriptItem::Notice(
            NoticeLevel::Info,
            format!("destilação não concluída: {error}"),
        )),
    }
}

/// Translate an engine message into a UI message, capturing an approval's reply channel on the way.
fn engine_msg(engine: EngineMsg, pending_reply: &mut Option<oneshot::Sender<Approval>>) -> Msg {
    match engine {
        EngineMsg::Began => Msg::TurnBegan,
        EngineMsg::Reasoning(text) => Msg::StreamDelta(StreamKind::Reasoning, text),
        EngineMsg::Content(text) => Msg::StreamDelta(StreamKind::Content, text),
        EngineMsg::ToolStarted { command, diff } => Msg::ToolStarted { command, diff },
        EngineMsg::ToolFinished {
            status,
            output,
            elapsed,
        } => Msg::ToolFinished {
            status,
            output,
            elapsed,
        },
        EngineMsg::Finished => Msg::TurnFinished,
        EngineMsg::Approval { pending, reply } => {
            *pending_reply = Some(reply);
            Msg::ApprovalRequested(pending)
        }
    }
}

/// Apply the turn's outcome: surface errors, roll back the conversation, and reset per-turn UI state.
/// A user cancel (^C) is reported as such, not as an error.
fn on_turn_end(
    result: Result<TurnOutcome, AgentError>,
    cancelled: bool,
    model: &mut Model,
    conversation: &mut Conversation,
) {
    match result {
        Ok(TurnOutcome::Completed) => {
            if turn_produced_nothing(conversation) {
                // A 200 with an empty assistant reply and no tool activity: the provider returned
                // nothing usable (e.g. an empty stream). Surface it — never silent.
                model.transcript.push(TranscriptItem::Notice(
                    NoticeLevel::Error,
                    "o provedor não retornou conteúdo — verifique o modelo/endpoint".to_string(),
                ));
            }
        }
        // A plan-mode turn called `present_plan`: render the finished plan and open the approval box.
        // The box appears ONLY here — never on a plain text turn — so the model may think or ask
        // questions in plan mode without prematurely triggering approval, and the plan shown is always
        // the complete tool argument, never a half-streamed transcript.
        Ok(TurnOutcome::PlanProposed(plan)) if !cancelled => {
            model.transcript.push(TranscriptItem::Assistant(plan));
            model.pending_plan = Some(PendingPlan::default());
        }
        Ok(TurnOutcome::PlanProposed(_)) => {}
        // A ^C while busy cancels just this turn: `drive_turn` sets the cancel token and synthesizes
        // `Aborted`, so `cancelled` is true here — show it and drop the dangling user message, but keep
        // the session alive. Only a genuine input-stream end (`cancelled == false`, e.g. the approval
        // channel closed) quits.
        Ok(TurnOutcome::Aborted) if cancelled => {
            model.transcript.push(TranscriptItem::Notice(
                NoticeLevel::Info,
                "⨯ cancelado".to_string(),
            ));
            conversation.rollback_dangling_user();
        }
        Ok(TurnOutcome::Aborted) => model.should_quit = true,
        Err(error) => {
            if cancelled {
                model.transcript.push(TranscriptItem::Notice(
                    NoticeLevel::Info,
                    "⨯ cancelado".to_string(),
                ));
            } else {
                model.transcript.push(TranscriptItem::Notice(
                    NoticeLevel::Error,
                    format!("erro: {error}"),
                ));
            }
            conversation.rollback_dangling_user();
            if !cancelled && matches!(error, AgentError::ProviderRejected { .. }) {
                conversation.rollback_last_assistant_turn();
                model.transcript.push(TranscriptItem::Notice(
                    NoticeLevel::Info,
                    "turno anterior descartado (request rejeitado pelo provedor)".to_string(),
                ));
            }
        }
    }
    // `TurnEnded` only resets per-turn model state (no effects); the returned Vec is intentionally
    // discarded.
    let _ = update(model, Msg::TurnEnded);
}

/// True when the turn ended with an empty assistant reply and no tool activity — the provider returned
/// a 200 with nothing usable. The agent loop appends the final assistant text even when it is blank, so
/// the trailing message is the signal: an assistant message with blank content and no tool calls. A
/// turn that ran tools (trailing `Role::Tool`) or produced real text is not "nothing".
fn turn_produced_nothing(conversation: &Conversation) -> bool {
    match conversation.messages().last() {
        Some(last) => {
            last.role == Role::Assistant
                && last.tool_calls.is_empty()
                && last.content.as_deref().unwrap_or("").trim().is_empty()
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{on_turn_end, turn_produced_nothing};
    use crate::modules::agent::application::agent_loop::TurnOutcome;
    use crate::modules::tui::domain::model::Model;
    use crate::modules::tui::domain::transcript::{NoticeLevel, TranscriptItem};
    use crate::shared::kernel::approval_mode::ApprovalMode;
    use crate::shared::kernel::conversation::Conversation;
    use crate::shared::kernel::message::Message;

    fn has_error_notice(model: &Model) -> bool {
        model
            .transcript
            .items()
            .iter()
            .any(|item| matches!(item, TranscriptItem::Notice(NoticeLevel::Error, _)))
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

        fn api_key() -> Credential {
            Credential::ApiKey {
                key: Secret::new("k"),
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
            let (_, model) = s.switch_to("claude").unwrap();
            assert_eq!(model, "claude-opus-4-8");
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
        use super::{FRAME_INTERVAL, spinner_frame};
        use std::time::Duration;
        assert_eq!(spinner_frame(Duration::ZERO), 0);
        assert_eq!(spinner_frame(FRAME_INTERVAL - Duration::from_millis(1)), 0);
        assert_eq!(spinner_frame(FRAME_INTERVAL), 1);
        assert_eq!(spinner_frame(FRAME_INTERVAL * 5), 5);
    }

    #[test]
    fn place_cursor_moves_the_edit_cursor() {
        use super::{frame_regions, place_cursor};
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
