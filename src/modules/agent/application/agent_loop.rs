use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::modules::agent::application::approval_policy::{
    Approval, ApprovalPolicy, CheckpointReason,
};
use crate::modules::agent::application::presenter::Presenter;
use crate::modules::agent::application::tool_observer::ToolObserver;
use crate::modules::provider::application::completion_provider::{
    CompletionProvider, EventSink, TurnRequest,
};
use crate::modules::tools::application::plan::{PRESENT_PLAN, extract_plan};
use crate::modules::tools::application::registry::ToolRegistry;
use crate::modules::tools::application::sandbox::Sandbox;
use crate::modules::tools::application::tool::ToolOutcome;
use crate::shared::kernel::approval_mode::ApprovalMode;
use crate::shared::kernel::conversation::Conversation;
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::message::Message;
use crate::shared::kernel::tool_call::ToolCall;

/// Protocol strings the model reads back in the conversation history, so they stay English — unlike the
/// user-facing pt-BR confirmation prompts in the `Bridge` adapter.
const TOOL_RESULT_PLAN_PRESENTED: &str = "Plan presented to the user for approval.";
const TOOL_RESULT_PLAN_BLOCKED: &str = "ignored: present_plan ends the turn";
const TOOL_RESULT_IGNORED_SESSION_ENDED: &str = "ignored: session ended";
const TOOL_RESULT_IGNORED_CHECKPOINT: &str = "ignored: execution interrupted at the checkpoint";
const TOOL_RESULT_IGNORED_USER_ABORT: &str = "ignored: interrupted by the user";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome {
    Completed,
    /// The carried text is the finished plan, surfaced for approval before anything is executed.
    PlanProposed(String),
    Aborted,
}

/// The reported duration covers only the execution, never the time the user spent at an approval prompt.
async fn timed<Fut: std::future::Future<Output = ToolOutcome>>(
    run: Fut,
) -> (ToolOutcome, Duration) {
    let start = Instant::now();
    let outcome = run.await;
    (outcome, start.elapsed())
}

/// A round that ends early must still leave the assistant `tool_calls` message fully answered, or the
/// exchange is invalid and `/resume` replays it into a provider 400.
fn answer_unanswered(conversation: &mut Conversation, calls: &[ToolCall], message: &str) {
    for call in calls {
        conversation.push(Message::tool_result(call.id.as_str(), message.to_string()));
    }
}

/// For one user turn: stream the assistant, then while it requests tools, confirm each call through the
/// UI, execute the approved ones, and feed the results back — guarded by a checkpoint against runaways.
pub struct AgentLoop {
    provider: Arc<dyn CompletionProvider>,
    registry: ToolRegistry,
    model: String,
    checkpoint_budget: Duration,
    max_tool_calls: usize,
}

impl AgentLoop {
    pub fn new(
        provider: Arc<dyn CompletionProvider>,
        registry: ToolRegistry,
        model: String,
        checkpoint_budget: Duration,
        max_tool_calls: usize,
    ) -> Self {
        Self {
            provider,
            registry,
            model,
            checkpoint_budget,
            max_tool_calls,
        }
    }

    /// A live `/provider` or `/effort` change rebuilds the Arc, since effort is captured at construction.
    /// Called only between turns; `run` borrows `&self`, so a swap cannot race an in-flight turn.
    pub fn set_provider(&mut self, provider: Arc<dyn CompletionProvider>) {
        self.provider = provider;
    }

    /// Read per turn, so this takes effect on the next one with no provider rebuild.
    pub fn set_model(&mut self, model: String) {
        self.model = model;
    }

    /// For an out-of-turn call like the end-of-session distillation. Clones the `Arc` so the caller drives
    /// `complete` without borrowing the loop, always seeing the latest adapter after a swap.
    pub fn provider(&self) -> Arc<dyn CompletionProvider> {
        self.provider.clone()
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// The conversation must already hold the user message. On a provider failure the caller renders the
    /// error and rolls back the dangling user message; `Aborted` means the user ended the session at a
    /// prompt. `io` is the engine's single UI surface, satisfied by the `Bridge` adapter in production.
    pub async fn run<IO: EventSink + Presenter + ApprovalPolicy + ToolObserver>(
        &self,
        conversation: &mut Conversation,
        sandbox: &dyn Sandbox,
        mode: ApprovalMode,
        io: &mut IO,
    ) -> Result<TurnOutcome, AgentError> {
        let mut mode = mode;
        // The turn's ORIGIN mode, and the authority for every plan-mode restriction below. A checkpoint's
        // "keep going, don't ask again" can flip the live `mode` to `Auto` mid-turn, but a turn that began
        // in Plan must keep refusing a plan_check-blacklisted run_command (rm, git commit, installs)
        // outright — never downgrade to Auto's live-confirmation gate (issue #28). The schema, the
        // plan-mode reminder, and the `present_plan` interception are all sticky to this, not to `mode`.
        let started_in_plan = mode == ApprovalMode::Plan;
        // Computed once from the ORIGIN mode: recomputing on a mid-turn `ApprovedAuto` would hand the turn
        // the destructive tools plan mode withheld.
        let schemas = self.registry.schemas_for(mode);
        let mut checkpoint = Instant::now();
        let mut calls_since_checkpoint: usize = 0;
        loop {
            io.begin_round();
            // Sticky (see `started_in_plan`): losing the reminder mid-turn makes the model silently stop
            // calling `present_plan`.
            let turn_messages = if started_in_plan {
                let mut msgs = conversation.messages().to_vec();
                msgs.push(Message::system(
                    "CRITICAL: The active approval mode is PLAN. You are restricted to read-only tools and run_command. \
                     Do NOT write file edits or code changes in your text response. Instead, investigate using read-only \
                     tools, design the plan, and submit it using the `present_plan` tool. Calling `present_plan` is the \
                     only way to propose your plan."
                ));
                msgs
            } else {
                conversation.messages().to_vec()
            };
            let result = self
                .provider
                .complete(
                    TurnRequest {
                        messages: &turn_messages,
                        model: &self.model,
                        tools: &schemas,
                    },
                    io,
                )
                .await;
            // Render cleanup must run before `?` propagates a provider error, so its Result is dropped: a
            // failed send means the runtime's receiver is already gone (the app is tearing down).
            let _ = io.finish_round();
            let turn = result?;

            if turn.tool_calls.is_empty() {
                let mut message = Message::assistant_text(turn.content);
                if let Some(thinking) = turn.thinking {
                    message = message.with_thinking(thinking);
                }
                conversation.push(message);
                return Ok(TurnOutcome::Completed);
            }

            let calls = turn.tool_calls;
            let content = turn.content;
            let narration = (!content.is_empty()).then_some(content.clone());
            let mut assistant_message = Message::assistant_tool_calls(narration, calls.clone());
            if let Some(thinking) = turn.thinking {
                assistant_message = assistant_message.with_thinking(thinking);
            }
            conversation.push(assistant_message);

            // `present_plan` is the explicit "plan is ready" signal: surface it and end the turn without
            // executing anything. Every call still gets a tool result, or the round is not a valid tool
            // exchange. Gated on `started_in_plan`, not `mode` — otherwise it would fall through to
            // `decide_and_run` and execute as an ordinary tool, echoing the plan back.
            if started_in_plan
                && let Some(plan_call) =
                    calls.iter().find(|call| call.function.name == PRESENT_PLAN)
            {
                let plan = extract_plan(plan_call).unwrap_or(content);
                for call in &calls {
                    let response_message = if call.function.name == PRESENT_PLAN {
                        TOOL_RESULT_PLAN_PRESENTED
                    } else {
                        TOOL_RESULT_PLAN_BLOCKED
                    };
                    conversation.push(Message::tool_result(
                        call.id.as_str(),
                        response_message.to_string(),
                    ));
                }
                return Ok(TurnOutcome::PlanProposed(plan));
            }

            for (index, call) in calls.iter().enumerate() {
                // Enforced WITHIN the round: one assistant message can carry unboundedly many tool calls,
                // so checking only between rounds would let a prompt-injected burst run unattended.
                if calls_since_checkpoint >= self.max_tool_calls {
                    let decision = io
                        .confirm_continue(CheckpointReason::CallCount {
                            calls: calls_since_checkpoint,
                        })
                        .await;
                    match self.checkpoint_transition(
                        decision,
                        &mut checkpoint,
                        &mut calls_since_checkpoint,
                        &mut mode,
                    ) {
                        None => {}
                        Some(TurnOutcome::Aborted) => {
                            answer_unanswered(
                                conversation,
                                &calls[index..],
                                TOOL_RESULT_IGNORED_SESSION_ENDED,
                            );
                            return Ok(TurnOutcome::Aborted);
                        }
                        Some(outcome) => {
                            answer_unanswered(
                                conversation,
                                &calls[index..],
                                TOOL_RESULT_IGNORED_CHECKPOINT,
                            );
                            return Ok(outcome);
                        }
                    }
                }

                // Shown in every mode, so the user sees each action even under auto.
                let command = self
                    .registry
                    .command_line(sandbox, call)
                    .unwrap_or_else(|| call.function.name.clone());

                let result = self
                    .decide_and_run(sandbox, call, &command, &mut mode, started_in_plan, io)
                    .await;

                let Some((outcome, elapsed)) = result else {
                    // Answer this call and every remaining one, or the exchange is invalid and unpersistable.
                    answer_unanswered(
                        conversation,
                        &calls[index..],
                        TOOL_RESULT_IGNORED_USER_ABORT,
                    );
                    return Ok(TurnOutcome::Aborted);
                };

                io.tool_finished(call, &outcome, elapsed);
                conversation.push(Message::tool_result(
                    call.id.as_str(),
                    outcome.into_message_content(),
                ));
                calls_since_checkpoint += 1;
            }

            // The call-cap leg here catches accumulation ACROSS rounds; the loop above catches it within one.
            if checkpoint.elapsed() >= self.checkpoint_budget
                || calls_since_checkpoint >= self.max_tool_calls
            {
                let reason = if calls_since_checkpoint >= self.max_tool_calls {
                    CheckpointReason::CallCount {
                        calls: calls_since_checkpoint,
                    }
                } else {
                    CheckpointReason::Elapsed {
                        minutes: checkpoint.elapsed().as_secs() / 60,
                    }
                };
                let decision = io.confirm_continue(reason).await;
                match self.checkpoint_transition(
                    decision,
                    &mut checkpoint,
                    &mut calls_since_checkpoint,
                    &mut mode,
                ) {
                    None => {}
                    Some(outcome) => return Ok(outcome),
                }
            }
        }
    }

    /// Emits `tool_started` just before running, so the transcript records every attempt — including
    /// declined and plan-blocked calls. `None` means the user aborted here, signalling `run` to answer the
    /// remaining calls and end the turn.
    async fn decide_and_run<IO: EventSink + Presenter + ApprovalPolicy + ToolObserver>(
        &self,
        sandbox: &dyn Sandbox,
        call: &ToolCall,
        command: &str,
        mode: &mut ApprovalMode,
        started_in_plan: bool,
        io: &mut IO,
    ) -> Option<(ToolOutcome, Duration)> {
        match *mode {
            // An Auto turn that STARTED in Plan keeps the plan-mode blacklist (see `started_in_plan`).
            // The schema freeze already withholds write_file/delete_file, but run_command is advertised in
            // Plan too, so without this a blacklisted command would silently downgrade from "refused" to
            // "confirm-prompted" the instant the checkpoint fires.
            ApprovalMode::Auto if started_in_plan => {
                self.plan_checked_run(sandbox, call, command, io).await
            }
            // Runs without asking, EXCEPT high-blast-radius tools and out-of-root targets. On platforms
            // with no OS sandbox this is the only thing stopping an unattended — or prompt-injected — turn
            // from destroying data or reaching outside the workspace.
            ApprovalMode::Auto => self.run_gated(sandbox, call, command, io).await,
            // Non-plannable tools are withheld from the schema; if the model names one anyway, refuse it
            // without touching the filesystem.
            ApprovalMode::Plan if !self.registry.is_plannable(&call.function.name) => {
                io.tool_started(call, command);
                Some((
                    ToolOutcome::Error(format!(
                        "'{}' is blocked in plan mode (not available for planning)",
                        call.function.name
                    )),
                    Duration::ZERO,
                ))
            }
            // SEC-01: a plannable tool is not a free pass. The same gate Auto enforces applies here, so a
            // prompt-injected plan turn cannot read `~/.ssh/id_rsa` back to the model or run an arbitrary
            // command unattended. In-root reads and searches still run free.
            ApprovalMode::Plan => self.plan_checked_run(sandbox, call, command, io).await,
            ApprovalMode::Default => match self.registry.confirm(sandbox, call) {
                Some(confirmation) => match io.decide(&confirmation).await {
                    Approval::Approved => {
                        io.tool_started(call, command);
                        Some(timed(self.registry.execute(sandbox, call)).await)
                    }
                    Approval::ApprovedAuto => {
                        *mode = ApprovalMode::Auto;
                        io.tool_started(call, command);
                        Some(timed(self.registry.execute(sandbox, call)).await)
                    }
                    Approval::Declined => {
                        io.tool_started(call, command);
                        Some((ToolOutcome::Declined, Duration::ZERO))
                    }
                    Approval::Aborted => None,
                },
                None => {
                    io.tool_started(call, command);
                    Some(timed(self.registry.execute(sandbox, call)).await)
                }
            },
        }
    }

    /// The auto-mode confirmation gate: a high-blast-radius tool or an out-of-root target still requires a
    /// live confirmation. Shared by `Auto` and (after `plan_check`) `Plan`, so plan mode never executes an
    /// out-of-root read without the same gate (SEC-01). `ApprovedAuto` is treated as `Approved` — the
    /// caller owns any mode transition. `None` means the user aborted at this call.
    async fn run_gated<IO: EventSink + Presenter + ApprovalPolicy + ToolObserver>(
        &self,
        sandbox: &dyn Sandbox,
        call: &ToolCall,
        command: &str,
        io: &mut IO,
    ) -> Option<(ToolOutcome, Duration)> {
        match self.registry.confirm(sandbox, call) {
            Some(confirmation)
                if self.registry.confirm_in_auto(&call.function.name)
                    || !confirmation.default_accept =>
            {
                match io.decide(&confirmation).await {
                    Approval::Approved | Approval::ApprovedAuto => {
                        io.tool_started(call, command);
                        Some(timed(self.registry.execute(sandbox, call)).await)
                    }
                    Approval::Declined => {
                        io.tool_started(call, command);
                        Some((ToolOutcome::Declined, Duration::ZERO))
                    }
                    Approval::Aborted => None,
                }
            }
            _ => {
                io.tool_started(call, command);
                Some(timed(self.registry.execute(sandbox, call)).await)
            }
        }
    }

    /// Shared by `Plan` and by `Auto` for a turn that started in `Plan`: both must refuse a blocked call
    /// outright — no filesystem touch, no confirmation prompt — never fall back to Auto's ordinary gate.
    async fn plan_checked_run<IO: EventSink + Presenter + ApprovalPolicy + ToolObserver>(
        &self,
        sandbox: &dyn Sandbox,
        call: &ToolCall,
        command: &str,
        io: &mut IO,
    ) -> Option<(ToolOutcome, Duration)> {
        if let Some(reason) = self.registry.plan_check(sandbox, call) {
            io.tool_started(call, command);
            Some((ToolOutcome::Error(reason), Duration::ZERO))
        } else {
            self.run_gated(sandbox, call, command, io).await
        }
    }

    /// `None` continues the turn: `Approved` resets the checkpoint clock and counter, `ApprovedAuto` also
    /// switches to `Auto`. `Some(outcome)` ends the turn, and the caller adds its own bookkeeping.
    fn checkpoint_transition(
        &self,
        decision: Approval,
        checkpoint: &mut Instant,
        calls_since_checkpoint: &mut usize,
        mode: &mut ApprovalMode,
    ) -> Option<TurnOutcome> {
        match decision {
            Approval::Approved => {
                *checkpoint = Instant::now();
                *calls_since_checkpoint = 0;
                None
            }
            // "Keep going, and don't ask again": resume and run the rest of the turn unattended.
            Approval::ApprovedAuto => {
                *mode = ApprovalMode::Auto;
                *checkpoint = Instant::now();
                *calls_since_checkpoint = 0;
                None
            }
            Approval::Declined => Some(TurnOutcome::Completed),
            Approval::Aborted => Some(TurnOutcome::Aborted),
        }
    }
}

#[cfg(test)]
mod tests;
