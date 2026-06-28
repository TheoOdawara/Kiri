//! End-of-session distillation: run the distiller while keeping the UI responsive (a spinner ticks and
//! Ctrl+C skips). Best-effort and bounded — a skip or failure surfaces a Notice and never blocks quit.

use std::time::Instant;

use crossterm::event::{Event, KeyCode, KeyModifiers};
use tokio_stream::StreamExt;

use crate::modules::memory::application::distill::{DistillReport, Distiller};
use crate::shared::kernel::conversation::Conversation;
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::message::Message;
use crate::shared::kernel::role::Role;

use super::render::draw_and_copy;
use super::turn::spinner_frame;
use super::{RunLoop, UiDriver};

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
    Done(Result<DistillReport, AgentError>),
    Skip,
    Tick,
}

impl RunLoop {
    /// Run the end-of-session distillation while keeping the UI responsive: a spinner ticks and Ctrl+C
    /// skips. Best-effort and bounded — the distiller's own timeout caps the wait, a skip or failure
    /// surfaces a Notice and never blocks the caller (a `/new`, a session switch, or quit). The
    /// conversation is read only and already persisted, so distillation never risks the session's data.
    pub(super) async fn drive_distillation(&mut self, ui: &mut UiDriver<'_>) {
        if !should_distill(&self.conversation) {
            return;
        }
        // Both scopes inert (memory disabled or failed): there is nothing to write to, so skip the LLM call.
        if !self.memory.project_memory_available() && !self.memory.shared_memory_available() {
            return;
        }

        let provider = self.agent_loop.provider();
        let model_id = self.agent_loop.model().to_string();
        let distiller = Distiller::new(self.memory.clone(), self.project_id.to_string());
        let messages: Vec<Message> = self.conversation.messages().to_vec();

        self.model
            .notify_info("destilando memórias da sessão… (^C pula)");
        self.model.busy = true;
        let started = Instant::now();
        self.model.timeline.render_at = Some(started);
        // Best-effort pre-op repaint to show the "distilling…" notice before the blocking call; the loop
        // redraws on its next iteration, so a failed draw here must not block the distillation.
        let _ = draw_and_copy(ui.terminal, &mut self.model);

        let outcome = {
            let mut distillation =
                Box::pin(distiller.distill(provider.as_ref(), &model_id, &messages));
            loop {
                let step = tokio::select! {
                    biased;
                    maybe = ui.events.next() => match maybe {
                        Some(Ok(event)) if is_ctrl_c(&event) => DistillStep::Skip,
                        // Other input is ignored during the (brief) distillation.
                        _ => DistillStep::Tick,
                    },
                    _ = ui.ticker.tick() => DistillStep::Tick,
                    done = &mut distillation => DistillStep::Done(done),
                };
                match step {
                    DistillStep::Done(result) => break Some(result),
                    DistillStep::Skip => break None,
                    DistillStep::Tick => {
                        self.model.status.spinner_frame = spinner_frame(started.elapsed());
                        self.model.timeline.render_at = Some(Instant::now());
                        // A draw failure ends the best-effort distillation rather than looping blind.
                        if draw_and_copy(ui.terminal, &mut self.model).is_err() {
                            break None;
                        }
                    }
                }
            }
        };

        self.model.busy = false;
        match outcome {
            None => self.model.notify_info("destilação pulada"),
            Some(Ok(report)) if report.written > 0 => self.model.notify_info(format!(
                "memória atualizada: {} aprendizado(s)",
                report.written
            )),
            // Nothing worth keeping: stay quiet rather than add noise on every /new.
            Some(Ok(_)) => {}
            Some(Err(error)) => self
                .model
                .notify_info(format!("destilação não concluída: {error}")),
        }
    }
}
