use std::cell::Cell;
use std::io;
use std::rc::Rc;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};

use crate::modules::agent::application::approval_policy::{
    Approval, ApprovalPolicy, CheckpointReason,
};
use crate::modules::agent::application::presenter::Presenter;
use crate::modules::agent::application::tool_observer::ToolObserver;
use crate::modules::provider::application::completion_provider::EventSink;
use crate::modules::tools::application::tool::{Confirmation, ToolOutcome};
use crate::modules::tui::domain::modal::PendingApproval;
use crate::modules::tui::domain::transcript::{ToolDiff, ToolStatus};
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::stream_event::StreamEvent;
use crate::shared::kernel::tool_call::ToolCall;

/// What the engine ports emit to the TUI runtime over the channel. An approval carries its reply
/// channel: the engine confirms tool calls one at a time, so there is never more than one pending and
/// no correlation id is needed.
pub(crate) enum EngineMsg {
    Began,
    Reasoning(String),
    Content(String),
    ToolStarted {
        command: String,
        diff: Option<ToolDiff>,
        is_run_command: bool,
    },
    ToolFinished {
        status: ToolStatus,
        output: String,
        elapsed: Duration,
    },
    Finished,
    Approval {
        pending: PendingApproval,
        reply: oneshot::Sender<Approval>,
    },
}

/// A cooperative cancellation flag. Single-threaded, so `Rc<Cell>` is sound and needs no dependency.
/// Checked on each stream delta in `EventSink::on_event`; when set, `on_event` returns `Err`, which
/// unwinds the provider's `complete()` through its existing `?` so the turn's normal error/rollback
/// path runs.
#[derive(Clone, Default)]
pub(crate) struct CancelToken(Rc<Cell<bool>>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.set(true);
    }

    pub fn reset(&self) {
        self.0.set(false);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.get()
    }
}

/// The single new adapter: implements the three engine UI ports over channels. The synchronous ports
/// push `EngineMsg`s (never blocking); the async approval ports send a request carrying a `oneshot`
/// and await the reply.
pub struct Bridge {
    tx: mpsc::UnboundedSender<EngineMsg>,
    cancel: CancelToken,
}

impl Bridge {
    pub(crate) fn new(tx: mpsc::UnboundedSender<EngineMsg>, cancel: CancelToken) -> Self {
        Self { tx, cancel }
    }

    fn push(&self, msg: EngineMsg) -> Result<(), AgentError> {
        self.tx
            .send(msg)
            .map_err(|_| AgentError::Io(io::Error::from(io::ErrorKind::BrokenPipe)))
    }

    async fn request(&self, prompt: String, default_accept: bool) -> Approval {
        let (reply, rx) = oneshot::channel();
        let pending = PendingApproval::new(prompt, default_accept);
        if self.push(EngineMsg::Approval { pending, reply }).is_err() {
            return Approval::Aborted;
        }
        rx.await.unwrap_or(Approval::Aborted)
    }
}

impl EventSink for Bridge {
    fn on_event(&mut self, event: StreamEvent) -> Result<(), AgentError> {
        if self.cancel.is_cancelled() {
            return Err(AgentError::Io(io::Error::from(io::ErrorKind::Interrupted)));
        }
        match event {
            StreamEvent::Reasoning(text) => self.push(EngineMsg::Reasoning(text)),
            StreamEvent::Content(text) => self.push(EngineMsg::Content(text)),
        }
    }
}

impl Presenter for Bridge {
    fn begin_round(&mut self) {
        // These synchronous ports return `()` and cannot propagate. `push` fails only when the
        // runtime's receiver has been dropped — i.e. the app is already tearing down — so a dropped
        // send here is benign by construction. The same holds for the ToolObserver sends below.
        let _ = self.push(EngineMsg::Began);
    }

    fn finish_round(&mut self) -> Result<(), AgentError> {
        self.push(EngineMsg::Finished)
    }
}

impl ToolObserver for Bridge {
    fn tool_started(&mut self, call: &ToolCall, command: &str) {
        // Best-effort; a dropped send means the runtime is gone (see `begin_round`).
        let _ = self.push(EngineMsg::ToolStarted {
            command: command.to_string(),
            diff: edit_diff(call),
            is_run_command: call.function.name == "run_command",
        });
    }

    fn tool_finished(&mut self, _call: &ToolCall, outcome: &ToolOutcome, elapsed: Duration) {
        let (status, output) = display_outcome(outcome);
        // Best-effort; a dropped send means the runtime is gone (see `begin_round`).
        let _ = self.push(EngineMsg::ToolFinished {
            status,
            output,
            elapsed,
        });
    }
}

/// Map a tool outcome to its display status and text. The model still receives the original outcome
/// via the conversation; this projection only feeds the transcript.
fn display_outcome(outcome: &ToolOutcome) -> (ToolStatus, String) {
    match outcome {
        ToolOutcome::Ok(text) => (ToolStatus::Ok, text.clone()),
        ToolOutcome::Error(error) => (ToolStatus::Error, error.clone()),
        ToolOutcome::Declined => (ToolStatus::Declined, String::new()),
    }
}

/// Extract an `edit_file` call's old/new text for an inline diff, straight from the call arguments —
/// the tool is not involved, so the adapter stays decoupled from the tool internals.
fn edit_diff(call: &ToolCall) -> Option<ToolDiff> {
    if call.function.name != "edit_file" {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(&call.function.arguments).ok()?;
    let old = value.get("old_string")?.as_str()?.to_string();
    let new = value.get("new_string")?.as_str()?.to_string();
    Some(ToolDiff { old, new })
}

#[async_trait::async_trait(?Send)]
impl ApprovalPolicy for Bridge {
    async fn decide(&mut self, confirmation: &Confirmation) -> Approval {
        self.request(confirmation.prompt.clone(), confirmation.default_accept)
            .await
    }

    async fn confirm_continue(&mut self, reason: CheckpointReason) -> Approval {
        let prompt = match reason {
            CheckpointReason::Elapsed { minutes } => {
                format!("Execução já dura ~{minutes}min. Continuar?")
            }
            CheckpointReason::CallCount { calls } => {
                format!("Já são {calls} chamadas de ferramenta neste turno. Continuar?")
            }
        };
        self.request(prompt, true).await
    }
}
