//! The TUI keymap: pure reducers that interpret input against the `Model`. Split by responsibility into
//! `editor_input` (key/mouse dispatch + the editor), `menu` (the slash-command preview), `submit`
//! (submit + command routing + picker openers), `modals` (approval/plan/picker handlers), and `wizard`
//! (the add-provider flow). The shared `use` imports below feed every submodule via `use super::*`.

use std::time::{Duration, Instant};

use crate::modules::agent::application::approval_policy::Approval;
use crate::modules::tui::application::command::{self, Command};
use crate::modules::tui::application::effect::Effect;
use crate::modules::tui::application::msg::{Key, KeyPress, MouseKind};
use crate::modules::tui::domain::command_menu::CommandMenu;
use crate::modules::tui::domain::modal::{ApprovalOption, PlanOption};
use crate::modules::tui::domain::model::Model;
use crate::modules::tui::domain::nav::wrapping_step;
use crate::modules::tui::domain::picker::{Picker, PickerKind};
use crate::modules::tui::domain::selection::{Granularity, ScreenSelection, SelectionState};
use crate::modules::tui::domain::transcript::{NoticeLevel, TranscriptItem};
use crate::modules::tui::domain::wizard::{ADD_PROVIDER_LABEL, ProviderWizard, WizardStep};
use crate::shared::kernel::approval_mode::ApprovalMode;
use crate::shared::kernel::provider::{AuthMethod, Effort, Secret};
use tui_textarea::{Input, Key as TaKey};

/// Transcript scroll amounts, in lines, single-sourced here for the keymap (key scroll) and `update.rs`
/// (wheel scroll). A key step (arrow) moves `SCROLL_STEP`; Shift pages by `SCROLL_PAGE`; a mouse-wheel
/// notch moves `WHEEL_STEP` — deliberately gentler than a key step, since a wheel emits several events
/// per physical notch.
pub(crate) const SCROLL_STEP: u16 = 5;
/// A "page" scroll step, used for Shift+PageUp/PageDown. The transcript viewport height is not stored on
/// the model, so a fixed large step stands in; the view clamps to the available history.
pub(crate) const SCROLL_PAGE: u16 = 20;
pub(crate) const WHEEL_STEP: u16 = 3;

mod editor_input;
mod menu;
mod modals;
mod submit;
mod wizard;

#[cfg(test)]
mod tests;

// Public API consumed by `update.rs` / `runtime.rs`.
pub use editor_input::{on_key, on_mouse};
pub use menu::sync_menu;

// The cross-module handlers that crossed the old single-file boundary, re-imported into the facade so
// every submodule's `use super::*` (and the test module's) resolves the call by its bare name —
// including `on_approval_key`, which `keymap/tests.rs` calls directly.
use menu::on_menu_key;
use modals::{on_approval_key, on_picker_key, on_plan_key};
use submit::submit;
use wizard::on_wizard_key;
