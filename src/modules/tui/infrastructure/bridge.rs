use std::cell::Cell;
use std::io;
use std::rc::Rc;

use tokio::sync::{mpsc, oneshot};

use crate::modules::agent::application::approval_policy::{Approval, ApprovalPolicy};
use crate::modules::agent::application::presenter::Presenter;
use crate::modules::agent::domain::stream_event::StreamEvent;
use crate::modules::provider::application::completion_provider::EventSink;
use crate::modules::tools::application::tool::Confirmation;
use crate::modules::tui::domain::view_state::PendingApproval;
use crate::shared::kernel::error::AgentError;

/// What the engine ports emit to the TUI runtime over the channel. An approval carries its reply
/// channel: the engine confirms tool calls one at a time, so there is never more than one pending and
/// no correlation id is needed.
pub enum EngineMsg {
    Began,
    Reasoning(String),
    Content(String),
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
pub struct CancelToken(Rc<Cell<bool>>);

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
    pub fn new(tx: mpsc::UnboundedSender<EngineMsg>, cancel: CancelToken) -> Self {
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
    fn begin_turn(&mut self) {
        let _ = self.push(EngineMsg::Began);
    }

    fn finish_turn(&mut self) -> Result<(), AgentError> {
        self.push(EngineMsg::Finished)
    }
}

#[async_trait::async_trait(?Send)]
impl ApprovalPolicy for Bridge {
    async fn decide(&mut self, confirmation: &Confirmation) -> Approval {
        self.request(confirmation.prompt.clone(), confirmation.default_accept)
            .await
    }

    async fn confirm_continue(&mut self, minutes: u64) -> Approval {
        self.request(format!("Execução já dura ~{minutes}min. Continuar?"), true)
            .await
    }
}
