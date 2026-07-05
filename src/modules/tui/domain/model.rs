use std::time::Instant;

use crate::shared::kernel::approval_mode::ApprovalMode;
use crate::shared::kernel::provider::{Effort, ProviderProfile, Secret};

use super::command_menu::{CommandMenu, CustomCommandEntry};
use super::history::History;
use super::input_buffer::{ImageAttachment, InputBuffer};
use super::modal::{PendingApproval, PendingPlan};
use super::picker::Picker;
use super::scroll::Scroll;
use super::selection::ScreenSelection;
use super::transcript::{NoticeLevel, Transcript, TranscriptItem};
use super::wizard::ProviderWizard;

/// Whether motion is fully expressed or frozen to its final frame. The session preference is resolved
/// once from the environment by the runtime (the I/O stays out of the domain); the view additionally
/// folds in per-frame geometry (a short/narrow terminal degrades to `Reduced`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Motion {
    #[default]
    Full,
    Reduced,
}

impl Motion {
    /// Fold in a per-frame reason to reduce (e.g. a cramped terminal): once reduced, always reduced.
    pub fn and_reduce_if(self, reduce: bool) -> Motion {
        if reduce || self == Motion::Reduced {
            Motion::Reduced
        } else {
            Motion::Full
        }
    }

    pub fn is_reduced(self) -> bool {
        self == Motion::Reduced
    }
}

/// The status line's data: the model id, the active workspace, the reasoning effort, and the live turn
/// indicators.
#[derive(Debug, Default)]
pub struct Status {
    pub model: String,
    pub workspace: String,
    /// The active provider id; updated by a live `/provider` swap.
    pub provider: String,
    /// The active reasoning effort; updated by a live `/effort` swap.
    pub effort: Effort,
    pub streaming: bool,
    pub elapsed_secs: u64,
    pub spinner_frame: usize,
}

impl Status {
    /// Elapsed time as a compact label: seconds under a minute, `Mm Ss` once it reaches one. The raw
    /// seconds field stays the single source of truth; this is a render-only projection.
    pub fn elapsed_label(&self) -> String {
        if self.elapsed_secs < 60 {
            format!("{}s", self.elapsed_secs)
        } else {
            format!("{}m {}s", self.elapsed_secs / 60, self.elapsed_secs % 60)
        }
    }
}

/// The animation/timing cluster: the motion preference plus every wall-clock instant the render reads,
/// grouped so a lifecycle transition resets them together instead of stranding one stale instant. The
/// runtime stamps `render_at`/`last_event_at`; the rest are derived render state.
#[derive(Debug, Default)]
pub struct Timeline {
    /// Whether motion is expressed or frozen; resolved once from the environment at startup.
    pub motion: Motion,
    /// The wall-clock instant of the current frame, stamped by the runtime before each update/draw. All
    /// time-derived rendering (the cooling reveal, the cursor pulse) reads this rather than calling the
    /// clock in the pure view, so a frame is a deterministic function of the model.
    pub render_at: Option<Instant>,
    /// Landing instants of the completed lines of the active streaming answer (one per `\n`), stamped with
    /// `render_at`. Drives the cooling-steel reveal; cleared at each turn and answer boundary.
    pub stream_landings: Vec<Instant>,
    /// When the last turn settled (`TurnEnded`), stamped with `render_at`. Drives the one-shot temper
    /// quench on the idle gate; cleared when a new turn begins.
    pub turn_settled_at: Option<Instant>,
    /// When the shell opened, stamped by the runtime at startup. Drives the splash breath-in and the
    /// living-cursor pulse; a keypress backdates it to fast-forward the splash for frequent users.
    pub opened_at: Option<Instant>,
    /// Timestamp of the last Ctrl+C press, for double-tap-to-quit detection.
    pub last_ctrl_c: Option<Instant>,
    /// Timestamp of the last Esc press, for double-tap-to-cancel detection while busy.
    pub last_esc: Option<Instant>,
    /// The instant the current input event arrived, stamped by the runtime right after reading it. Used as
    /// the clock for multi-click detection — `render_at` is stamped before the event await and would be
    /// stale, so it cannot time clicks.
    pub last_event_at: Option<Instant>,
}

impl Timeline {
    /// A turn began: clear the streaming-reveal landings and the settled marker so no instant from the
    /// previous turn leaks into the new one.
    pub fn begin_turn(&mut self) {
        self.stream_landings.clear();
        self.turn_settled_at = None;
    }

    /// A turn settled: the streaming reveal is done, and the idle gate quenches from the current frame.
    /// Resets the per-turn render instants atomically so none is stranded across the boundary.
    pub fn settle_turn(&mut self) {
        self.stream_landings.clear();
        self.turn_settled_at = self.render_at;
    }

    /// Drop the streaming-reveal landings at an answer boundary (a fresh assistant item), keeping the
    /// lifecycle instants intact.
    pub fn reset_stream(&mut self) {
        self.stream_landings.clear();
    }
}

/// The mouse text-selection cluster: the active screen selection plus the last mouse-down used for
/// multi-click detection, grouped so a navigation transition clears them together.
#[derive(Debug, Default)]
pub struct Selection {
    /// The active screen text selection (mouse drag / multi-click), or `None`. The overlay paints it and
    /// the runtime scrapes the rendered cells to copy; `None` by default keeps every idle frame identical.
    pub active: Option<ScreenSelection>,
    /// Instant + cell of the last mouse-down and its running multiplicity (1=char, 2=word, 3+=line), for
    /// double/triple-click detection.
    pub last_click: Option<(Instant, (u16, u16), u8)>,
}

/// The whole TUI state — a pure value mutated only by `update`. The runtime renders it and feeds it
/// messages; it never holds engine handles (channels/conversation live in the runtime).
#[derive(Debug, Default)]
pub struct Model {
    pub transcript: Transcript,
    pub input: InputBuffer,
    pub history: History,
    pub scroll: Scroll,
    pub status: Status,
    /// A confirmation awaiting an answer; while set, keys answer it instead of editing.
    pub pending_approval: Option<PendingApproval>,
    /// A finished plan awaiting the user's decision; while set, keys drive the plan box.
    pub pending_plan: Option<PendingPlan>,
    /// An open single-choice picker (`/models` / `/effort` / `/provider`); while set, keys drive it.
    pub picker: Option<Picker>,
    /// The active provider's model catalog, offered by the `/models` picker.
    pub models: Vec<String>,
    /// The configured provider ids, offered by the `/provider` picker.
    pub providers: Vec<String>,
    /// Full profiles parallel to `providers`; used by the action sub-menu to display details and
    /// pre-populate the edit wizard. Kept in sync by the runtime alongside `providers`.
    pub provider_profiles: Vec<ProviderProfile>,
    /// The ids of the workspace's recent sessions, parallel to the `/sessions` picker rows, so the
    /// keymap maps a highlighted row back to a session id without coupling the domain to the session
    /// store. Filled by the runtime just before opening the picker.
    pub session_ids: Vec<String>,
    /// The open add-provider wizard, or `None`. While set, keys drive its steps.
    pub wizard: Option<ProviderWizard>,
    /// The API key typed in the wizard, staged for the runtime to store in the credential store. Held as a
    /// `Secret` (redacted in `Debug`) and taken on `SaveProvider`, so the key never rides in an effect
    /// or the transcript.
    pub pending_credential: Option<Secret>,
    /// The live slash-command preview, open while the input starts with `/` and has no whitespace yet.
    pub command_menu: Option<CommandMenu>,
    /// Extension-provided custom commands (ADR 0021), shown in the live preview alongside the built-ins.
    pub custom_commands: Vec<CustomCommandEntry>,
    /// Every extension command token (canonical name + aliases) mapped straight to its expanded prompt
    /// body, so submit-time lookup is a single hit regardless of which alias was typed.
    pub custom_command_bodies: std::collections::HashMap<String, String>,
    /// The formatted `/rules` display text (id, layer, always-on) for the loaded extension rules. `None`
    /// when none were found.
    pub rules_display: Option<String>,
    /// The formatted `/commands` display text (name, aliases, layer, source path) for the loaded custom
    /// commands. `None` when none were found.
    pub commands_display: Option<String>,
    /// The formatted `/agents` display text (id, layer, source path). `None` when none were found.
    pub agents_display: Option<String>,
    /// The formatted `/skills` display text (id, tags, layer, source path). `None` when none were found.
    pub skills_display: Option<String>,
    /// Images pasted from the clipboard, staged for the next prompt and drained on submit.
    pub attachments: Vec<ImageAttachment>,
    /// When set, tool outputs and edit diffs render in full instead of a bounded preview. Toggled
    /// with Ctrl+O.
    pub expand_tools: bool,
    /// The pre-formatted `/instructions` display text (paths header + merged content). `None` when no
    /// instructions file was found at boot.
    pub instructions_display: Option<String>,
    /// A turn is running (the agent loop future is armed).
    pub busy: bool,
    pub should_quit: bool,
    /// How tool calls are gated; cycled with Shift+Tab, read at the start of each turn.
    pub approval_mode: ApprovalMode,
    /// The animation/timing cluster: the motion preference plus every wall-clock instant the render reads.
    pub timeline: Timeline,
    /// The mouse text-selection cluster: the active screen selection plus the last mouse-down.
    pub selection: Selection,
    /// True until a usable provider credential exists. Raised at cold start when wiring fell back to the
    /// null provider (no stored credential / no env key, or a blank active model); cleared when
    /// onboarding saves a provider. Gates prompt submission and re-opens onboarding instead of stranding
    /// the user against the null provider.
    pub unconfigured: bool,
    /// Which pane currently has keyboard focus.
    pub focused_pane: PaneFocus,
    /// The index of the selected transcript item when in transcript focus mode.
    pub selected_item: Option<usize>,
    /// Individual tool indices that are manually expanded.
    pub expanded_tools_indices: std::collections::HashSet<usize>,
    /// Active search query in the transcript history.
    pub search_query: Option<String>,
    /// Transcript item indices that match the active search query.
    pub search_results: Vec<usize>,
    /// The current highlighted search result index in `search_results`.
    pub active_search_match: usize,
}

/// Which pane has keyboard input focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaneFocus {
    #[default]
    Input,
    Transcript,
}

/// The single modal currently awaiting the user, in precedence order, borrowed from the model. Resolved
/// once by [`Model::active_modal`] so the view's render dispatch and its region sizing never re-derive the
/// `plan ▸ approval ▸ picker ▸ wizard` order independently.
pub enum ActiveModal<'a> {
    Plan(&'a PendingPlan),
    Approval(&'a PendingApproval),
    Picker(&'a Picker),
    Wizard(&'a ProviderWizard),
}

impl Model {
    /// The modal awaiting the user, in precedence order: a finished plan, then a tool approval, then an
    /// open picker, then the add-provider wizard. The single source both the render dispatch and the
    /// region sizing read, so they can never disagree on which modal is showing.
    pub fn active_modal(&self) -> Option<ActiveModal<'_>> {
        if let Some(plan) = &self.pending_plan {
            Some(ActiveModal::Plan(plan))
        } else if let Some(approval) = &self.pending_approval {
            Some(ActiveModal::Approval(approval))
        } else if let Some(picker) = &self.picker {
            Some(ActiveModal::Picker(picker))
        } else {
            self.wizard.as_ref().map(ActiveModal::Wizard)
        }
    }

    /// Whether a modal (a tool approval, a finished plan, or an open picker/wizard) is awaiting the user.
    /// While true the transcript and header recede so the decision pulls focus by depth.
    pub fn has_modal(&self) -> bool {
        self.active_modal().is_some()
    }

    pub fn new(model: String, workspace: String) -> Self {
        Self {
            status: Status {
                model,
                workspace,
                ..Status::default()
            },
            ..Self::default()
        }
    }

    /// Seed the provider-swap surface: the active provider's model catalog (offered by `/models`) and
    /// the current reasoning effort (the `/effort` picker's starting point + the status display).
    pub fn with_provider_catalog(mut self, models: Vec<String>, effort: Effort) -> Self {
        self.models = models;
        self.status.effort = effort;
        self
    }

    /// Seed the `/provider` picker: the active provider id (status display), the configured id catalog,
    /// and the full profiles (for the action sub-menu and edit wizard).
    pub fn with_providers(
        mut self,
        active: String,
        providers: Vec<String>,
        profiles: Vec<ProviderProfile>,
    ) -> Self {
        self.status.provider = active;
        self.providers = providers;
        self.provider_profiles = profiles;
        self
    }

    /// Seed the instructions display text for the `/instructions` command.
    pub fn with_instructions(mut self, display: Option<String>) -> Self {
        self.instructions_display = display;
        self
    }

    /// Seed the extension-provided custom commands: the preview entries, the token→body lookup used at
    /// submit time, and the `/commands` display text.
    pub fn with_custom_commands(
        mut self,
        entries: Vec<CustomCommandEntry>,
        bodies: std::collections::HashMap<String, String>,
        display: Option<String>,
    ) -> Self {
        self.custom_commands = entries;
        self.custom_command_bodies = bodies;
        self.commands_display = display;
        self
    }

    /// Seed the rules display text for the `/rules` command.
    pub fn with_rules(mut self, display: Option<String>) -> Self {
        self.rules_display = display;
        self
    }

    /// Seed the agents display text for the `/agents` command.
    pub fn with_agents(mut self, display: Option<String>) -> Self {
        self.agents_display = display;
        self
    }

    /// Seed the skills display text for the `/skills` command.
    pub fn with_skills(mut self, display: Option<String>) -> Self {
        self.skills_display = display;
        self
    }

    /// Drop any active screen selection — the user navigated away (typed, scrolled, resized, or started a
    /// new session). Cheap and idempotent.
    pub fn clear_screen_selection(&mut self) {
        self.selection.active = None;
    }

    /// Push an out-of-band notice at `level` — the single constructor for transcript notices, so no call
    /// site open-codes `TranscriptItem::Notice`. The base method also serves the one dynamic-level caller.
    pub fn notify(&mut self, level: NoticeLevel, message: impl Into<String>) {
        self.transcript
            .push(TranscriptItem::Notice(level, message.into()));
    }

    /// Push an info-level notice.
    pub fn notify_info(&mut self, message: impl Into<String>) {
        self.notify(NoticeLevel::Info, message);
    }

    /// Push an error-level notice.
    pub fn notify_error(&mut self, message: impl Into<String>) {
        self.notify(NoticeLevel::Error, message);
    }

    /// Enter first-run onboarding: raise the submit gate, open the welcome wizard (NVIDIA preselected),
    /// and post the welcome notice. A pure model mutation the runtime calls from `Tui::new` when wiring
    /// fell back to the null provider.
    pub fn enter_onboarding(&mut self) {
        self.unconfigured = true;
        self.wizard = Some(ProviderWizard::onboarding());
        self.notify_info(ONBOARDING_WELCOME);
    }
}

/// The welcome line shown when the harness boots with no provider configured.
const ONBOARDING_WELCOME: &str = "Bem-vindo ao Kiri. Escolha um provider e informe sua API key para começar — \
     nenhuma variável de ambiente é necessária.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_label_formats_seconds_below_a_minute() {
        let s = Status {
            elapsed_secs: 0,
            ..Status::default()
        };
        assert_eq!(s.elapsed_label(), "0s");
        let s = Status {
            elapsed_secs: 59,
            ..Status::default()
        };
        assert_eq!(s.elapsed_label(), "59s");
    }

    #[test]
    fn elapsed_label_formats_minutes_and_seconds_at_and_above_a_minute() {
        let s = Status {
            elapsed_secs: 60,
            ..Status::default()
        };
        assert_eq!(s.elapsed_label(), "1m 0s");
        let s = Status {
            elapsed_secs: 125,
            ..Status::default()
        };
        assert_eq!(s.elapsed_label(), "2m 5s");
    }

    #[test]
    fn notify_info_pushes_an_info_notice() {
        let mut m = Model::default();
        m.notify_info("hello");
        assert_eq!(
            m.transcript.items().last(),
            Some(&TranscriptItem::Notice(
                NoticeLevel::Info,
                "hello".to_string()
            ))
        );
    }

    #[test]
    fn notify_error_pushes_an_error_notice() {
        let mut m = Model::default();
        m.notify_error("boom");
        assert_eq!(
            m.transcript.items().last(),
            Some(&TranscriptItem::Notice(
                NoticeLevel::Error,
                "boom".to_string()
            ))
        );
    }

    #[test]
    fn notify_pushes_with_the_given_level() {
        let mut m = Model::default();
        m.notify(NoticeLevel::Error, "dynamic");
        assert_eq!(
            m.transcript.items().last(),
            Some(&TranscriptItem::Notice(
                NoticeLevel::Error,
                "dynamic".to_string()
            ))
        );
    }

    #[test]
    fn enter_onboarding_opens_the_nvidia_wizard_and_raises_the_gate() {
        use crate::modules::tui::domain::transcript::TranscriptItem;
        use crate::shared::kernel::provider::ProviderKind;

        let mut m = Model::default();
        m.enter_onboarding();

        assert!(m.unconfigured, "the submit gate must be raised");
        assert!(m.has_modal(), "the onboarding wizard is a modal");
        let wizard = m.wizard.as_ref().expect("onboarding opens the wizard");
        assert!(wizard.onboarding);
        assert_eq!(wizard.kind(), ProviderKind::Nvidia);
        assert!(
            m.transcript.items().iter().any(
                |item| matches!(item, TranscriptItem::Notice(_, text) if text.contains("Bem-vindo"))
            ),
            "a welcome notice must be posted"
        );
    }

    #[test]
    fn active_modal_orders_plan_over_approval_over_picker_over_wizard() {
        use crate::modules::tui::domain::picker::PickerKind;

        let mut m = Model {
            pending_plan: Some(PendingPlan::default()),
            pending_approval: Some(PendingApproval::new("p".to_string(), true)),
            picker: Some(Picker::new(
                PickerKind::Models,
                "m",
                "a",
                vec!["x".to_string()],
                0,
            )),
            wizard: Some(ProviderWizard::new()),
            ..Model::default()
        };
        // Peel the modals off in precedence order; each step must surface the next one down.
        assert!(matches!(m.active_modal(), Some(ActiveModal::Plan(_))));
        m.pending_plan = None;
        assert!(matches!(m.active_modal(), Some(ActiveModal::Approval(_))));
        m.pending_approval = None;
        assert!(matches!(m.active_modal(), Some(ActiveModal::Picker(_))));
        m.picker = None;
        assert!(matches!(m.active_modal(), Some(ActiveModal::Wizard(_))));
        m.wizard = None;
        assert!(m.active_modal().is_none());
    }

    #[test]
    fn timeline_reset_clears_all_render_instants() {
        let now = Instant::now();
        let mut t = Timeline {
            render_at: Some(now),
            stream_landings: vec![now, now],
            turn_settled_at: Some(now),
            ..Timeline::default()
        };
        // begin_turn drops the streaming landings and the settled marker so no per-turn instant leaks
        // into the new turn.
        t.begin_turn();
        assert!(t.stream_landings.is_empty());
        assert!(t.turn_settled_at.is_none());

        // settle_turn re-derives the settled marker from the current frame and clears the landings
        // atomically, leaving none stranded across the boundary.
        t.render_at = Some(now);
        t.stream_landings = vec![now];
        t.settle_turn();
        assert!(t.stream_landings.is_empty());
        assert_eq!(t.turn_settled_at, Some(now));
    }
}
