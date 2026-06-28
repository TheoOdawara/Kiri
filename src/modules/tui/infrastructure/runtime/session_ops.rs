//! Session persistence orchestration and the session-management effect handlers: flush the conversation
//! tail, open/resume a session, list sessions for the picker, change workspace, and start a new session.

use crate::modules::memory::domain::project_id::project_id_from_path;
use crate::modules::session::domain::session::{UNTITLED_SESSION_LABEL, derive_title};
use crate::modules::tools::application::sandbox::Sandbox;
use crate::modules::tui::domain::picker::{Picker, PickerKind};
use crate::modules::tui::domain::transcript::{NoticeLevel, Transcript, TranscriptItem};
use crate::modules::tui::infrastructure::text;
use crate::shared::kernel::conversation::Conversation;
use crate::shared::kernel::message::Message;
use crate::shared::kernel::role::Role;

use super::{RunLoop, UiDriver};

/// Session persistence cursor: the id of the row backing the current conversation (lazily created on the
/// first flush, so an empty session never hits the DB) and how many non-system messages have already
/// been written, so each flush appends only the new tail.
pub(super) struct SessionCursor {
    pub(super) session_id: Option<String>,
    pub(super) persisted_len: usize,
}

/// How many recent sessions the `/sessions` picker lists.
const SESSION_LIST_LIMIT: usize = 20;

/// Length of an RFC3339 timestamp truncated to minute precision: `YYYY-MM-DD HH:MM` is 16 chars.
const MINUTE_PRECISION_LEN: usize = 16;

/// Trim an RFC3339 timestamp to `YYYY-MM-DD HH:MM` for the compact session-list label.
fn short_timestamp(raw: &str) -> String {
    raw.get(..MINUTE_PRECISION_LEN)
        .unwrap_or(raw)
        .replace('T', " ")
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

impl RunLoop {
    /// Persist the conversation's new tail to the session store, lazily creating the session row on the
    /// first non-empty flush (so an empty session never touches the DB). The system message (index 0) is
    /// never stored — it is regenerated per run from the current memory digest. Best-effort: an
    /// unavailable store is a silent no-op, and a write failure surfaces a Notice without ever losing the
    /// in-memory conversation. Called after a turn settles (post-rollback), so the DB mirrors the
    /// resumable in-memory state.
    pub(super) async fn flush_session(&mut self) {
        if !self.session_store.is_available() {
            return;
        }
        let messages = self.conversation.messages();
        // The body excludes the system seed at index 0.
        let body = &messages[1..];
        // Clamp the cursor: a rollback can shrink the body below it. In practice the rolled-back messages
        // were never persisted (we only flush after a turn settles), so this only guards against a panic.
        let cursor = self.cursor.persisted_len.min(body.len());
        if body.len() <= cursor {
            return;
        }
        let delta = &body[cursor..];

        let id = match self.cursor.session_id.clone() {
            Some(id) => id,
            None => match self.session_store.create(&self.project_id).await {
                Ok(session) => {
                    self.cursor.session_id = Some(session.id.clone());
                    session.id
                }
                Err(error) => {
                    self.model
                        .notify_error(format!("não persistiu a sessão: {error}"));
                    return;
                }
            },
        };

        let first_flush = cursor == 0;
        if let Err(error) = self.session_store.append_messages(&id, delta).await {
            self.model
                .notify_error(format!("não persistiu a sessão: {error}"));
            return;
        }
        if first_flush
            && let Some(title_source) = body
                .iter()
                .find(|m| m.role == Role::User)
                .and_then(|m| m.content.as_deref())
        {
            // Title is cosmetic (the `/sessions` label); a failure must not fail the flush, and the
            // messages are already saved, so a derive/store failure is safely ignored.
            let _ = self
                .session_store
                .set_title(&id, &derive_title(title_source))
                .await;
        }
        self.cursor.persisted_len = body.len();
    }

    /// Query the workspace's recent sessions and open the `/sessions` picker, recording the parallel id
    /// list the keymap resolves against. An unavailable store or an empty list surfaces a Notice and
    /// opens nothing.
    pub(super) async fn list_sessions(&mut self) {
        if !self.session_store.is_available() {
            self.model
                .notify_info("persistência de sessão indisponível");
            return;
        }
        match self
            .session_store
            .list_for_project(&self.project_id, SESSION_LIST_LIMIT)
            .await
        {
            Ok(sessions) if !sessions.is_empty() => {
                self.model.session_ids = sessions.iter().map(|s| s.id.clone()).collect();
                let options = sessions
                    .iter()
                    .map(|s| {
                        let title = if s.title.trim().is_empty() {
                            UNTITLED_SESSION_LABEL
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
                self.model.picker = Some(Picker::new(
                    PickerKind::Sessions,
                    "sessão",
                    "Escolha uma sessão para retomar:",
                    options,
                    0,
                ));
            }
            Ok(_) => self
                .model
                .notify_info("nenhuma sessão anterior neste workspace"),
            Err(error) => self
                .model
                .notify_error(format!("não foi possível listar as sessões: {error}")),
        }
    }

    /// Reopen the most recent session for the active workspace (`/resume`): on an inert store, surface
    /// the clean degraded-mode notice (never a leaked SQL detail); otherwise resolve the latest id and
    /// behave like `open_session`, or notice when there is none.
    pub(super) async fn resume_last(&mut self, ui: &mut UiDriver<'_>) {
        // On an inert store (init never ran, so no `sessions` table) latest_for_project raises a raw
        // "no such table" error; guard it the same way /sessions does so the user sees the clean
        // degraded-mode notice, not a leaked SQL detail.
        if !self.session_store.is_available() {
            self.model
                .notify_info("persistência de sessão indisponível");
            return;
        }
        match self
            .session_store
            .latest_for_project(&self.project_id)
            .await
        {
            Ok(Some(summary)) => self.open_session(&summary.id, ui).await,
            Ok(None) => self
                .model
                .notify_info("nenhuma sessão anterior neste workspace"),
            Err(error) => self
                .model
                .notify_error(format!("não foi possível ler as sessões: {error}")),
        }
    }

    /// Finalize the current session (distill, then flush), then load `target_id` and rebuild the
    /// conversation and transcript from it. The system prompt is the current one (a fresh memory digest),
    /// correct because stored messages exclude the system seed. A missing/failed load surfaces a Notice
    /// and leaves the current state intact.
    pub(super) async fn open_session(&mut self, target_id: &str, ui: &mut UiDriver<'_>) {
        // Learn from the current session before switching away from it.
        self.drive_distillation(ui).await;
        // Persist the current session's tail before switching away, so nothing is lost.
        self.flush_session().await;
        match self.session_store.load(target_id).await {
            Ok(Some(session)) => {
                let mut fresh = Conversation::new(self.system_prompt.clone());
                for message in &session.messages {
                    fresh.push(message.clone());
                }
                self.model.transcript = rebuild_transcript(&session.messages);
                self.model.attachments.clear();
                self.model.scroll.pin();
                let title = if session.title.trim().is_empty() {
                    UNTITLED_SESSION_LABEL.to_string()
                } else {
                    session.title.clone()
                };
                self.model.notify_info(format!("sessão retomada: {title}"));
                if session.skipped_messages > 0 {
                    self.model.notify_error(format!(
                        "{} mensagem(ns) corrompida(s) ignorada(s) ao retomar a sessão",
                        session.skipped_messages
                    ));
                }
                self.conversation = fresh;
                self.cursor.session_id = Some(session.id);
                self.cursor.persisted_len = session.messages.len();
            }
            Ok(None) => self.model.notify_error("sessão não encontrada"),
            Err(error) => self
                .model
                .notify_error(format!("não foi possível abrir a sessão: {error}")),
        }
    }

    /// Move the active workspace (`/cd`): relocate the sandbox, distill the old project's session, then
    /// re-key sessions to the new workspace (detach the cursor; the next turn starts a fresh session under
    /// the new project_id). A relocation failure surfaces a Notice and changes nothing.
    pub(super) async fn change_workspace(&mut self, path: String, ui: &mut UiDriver<'_>) {
        match self.sandbox.relocated(&path) {
            Ok(new_sandbox) => {
                // Learn from the old project's session before re-keying to the new workspace.
                self.drive_distillation(ui).await;
                self.model.status.workspace = text::display_path(new_sandbox.root());
                // Sessions are keyed by workspace: the current one belongs to the old project, so detach
                // and re-key. The next turn starts a fresh session under the new project_id.
                let root = new_sandbox.root();
                // canonicalize fails only for a missing/permission-denied path; the literal root is a safe
                // fallback for project-id keying (the sandbox already proved the dir exists and is usable).
                let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
                self.project_id = project_id_from_path(&canonical_root);
                self.cursor.session_id = None;
                self.cursor.persisted_len = 0;
                self.sandbox = new_sandbox;
                self.model
                    .notify_info(format!("workspace: {}", self.model.status.workspace));
            }
            Err(error) => self.model.notify_error(format!("erro: {error:#}")),
        }
    }

    /// Discard the conversation and start a fresh session: distill what is being discarded, rebuild a
    /// fresh conversation with the same system prompt, detach the persistence cursor, and reset the view.
    pub(super) async fn new_session(&mut self, ui: &mut UiDriver<'_>) {
        // Learn from the session being discarded before it is gone.
        self.drive_distillation(ui).await;
        self.conversation = Conversation::new(self.system_prompt.clone());
        // Detach from the persisted row: the next turn lazily creates a fresh session.
        self.cursor.session_id = None;
        self.cursor.persisted_len = 0;
        self.model.transcript = Transcript::default();
        self.model.attachments.clear();
        self.model.scroll.pin();
        self.model.notify_info("nova sessão");
    }
}
