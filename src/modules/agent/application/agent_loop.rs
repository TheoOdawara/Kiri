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

/// Whether a user turn ran to completion, proposed a plan for approval, or the user ended the session
/// at a prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome {
    Completed,
    /// A plan-mode turn called `present_plan`: the carried text is the finished plan, surfaced for the
    /// user's approval before anything is executed.
    PlanProposed(String),
    Aborted,
}

/// The outcome of a runaway checkpoint's shared approval arms: either the turn continues (the user
/// approved, optionally switching to auto) or it ends with the carried outcome (declined or aborted).
/// The caller layers on its own end-of-turn bookkeeping (the within-round case answers the remaining
/// calls before returning).
enum CheckpointFlow {
    Continue,
    End(TurnOutcome),
}

/// Run a tool execution to completion, returning its outcome alongside how long only the execution
/// took (so the reported duration excludes any time the user spent at an approval prompt). Async so
/// tools that spawn processes can await them; the wall clock still measures only the await span.
async fn timed<Fut: std::future::Future<Output = ToolOutcome>>(
    run: Fut,
) -> (ToolOutcome, Duration) {
    let start = Instant::now();
    let outcome = run.await;
    (outcome, start.elapsed())
}

/// Push a tool result for every call in `calls`, so a round that ends early (user abort or a declined
/// runaway checkpoint) still leaves the assistant `tool_calls` message fully answered — a valid,
/// persistable OpenAI tool exchange that `/resume` can replay without the provider rejecting it (400).
fn answer_unanswered(conversation: &mut Conversation, calls: &[ToolCall], message: &str) {
    for call in calls {
        conversation.push(Message::tool_result(call.id.as_str(), message.to_string()));
    }
}

/// The agent loop. For one user turn: stream the assistant, then while it requests tools, confirm each
/// call through the UI, execute approved ones against the sandbox, feed the results back, and repeat
/// until the model stops requesting tools — guarded by a wall-clock checkpoint against runaways.
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

    /// Swap the provider adapter mid-session (a live `/provider` or `/effort` change rebuilds the Arc,
    /// since effort is captured at provider construction). Called only between turns; `run` borrows
    /// `&self`, so a swap cannot race an in-flight turn.
    pub fn set_provider(&mut self, provider: Arc<dyn CompletionProvider>) {
        self.provider = provider;
    }

    /// Swap the active model id mid-session (a live `/models` change). The model is read per turn from
    /// `self.model`, so this alone takes effect on the next turn — no provider rebuild needed.
    pub fn set_model(&mut self, model: String) {
        self.model = model;
    }

    /// The current provider adapter, for an out-of-turn call like the end-of-session distillation. Clones
    /// the `Arc` so the caller drives `complete` without borrowing the loop, and always sees the latest
    /// adapter after a live `/provider`/`/effort` swap.
    pub fn provider(&self) -> Arc<dyn CompletionProvider> {
        self.provider.clone()
    }

    /// The active model id (for the same out-of-turn calls).
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Drive one user turn to completion. The conversation must already hold the user message. On a
    /// provider failure the error is returned (the caller renders it and rolls back a dangling user
    /// message); `Aborted` means the user ended the session at a prompt. `io` is the engine's single UI
    /// surface — the `EventSink`/`Presenter`/`ApprovalPolicy` ports, all satisfied by the `Bridge`
    /// adapter in production.
    pub async fn run<IO: EventSink + Presenter + ApprovalPolicy + ToolObserver>(
        &self,
        conversation: &mut Conversation,
        sandbox: &dyn Sandbox,
        mode: ApprovalMode,
        io: &mut IO,
    ) -> Result<TurnOutcome, AgentError> {
        // The advertised tool set is fixed for the turn; in plan mode it excludes destructive tools.
        // `mode` may still tighten to `Auto` mid-turn if the user approves a call with "don't ask again".
        let mut mode = mode;
        let schemas = self.registry.schemas_for(mode);
        let mut checkpoint = Instant::now();
        let mut calls_since_checkpoint: usize = 0;
        loop {
            io.begin_turn();
            let result = self
                .provider
                .complete(
                    TurnRequest {
                        messages: conversation.messages(),
                        model: &self.model,
                        tools: &schemas,
                    },
                    io,
                )
                .await;
            // Finish rendering (erase the spinner, reset the terminal) before `?` can propagate a
            // provider error: the cleanup must run on the failure path too, so its Result is dropped.
            // A failed finish send means the runtime's receiver is gone (the app is tearing down) —
            // benign, and nothing useful could be done with the error here.
            let _ = io.finish_turn();
            let turn = result?;

            if turn.tool_calls.is_empty() {
                // Plain text turn (also covers a degenerate tool-call finish with no parsed calls).
                conversation.push(Message::assistant_text(turn.content));
                return Ok(TurnOutcome::Completed);
            }

            let calls = turn.tool_calls;
            let content = turn.content;
            let narration = (!content.is_empty()).then_some(content.clone());
            conversation.push(Message::assistant_tool_calls(narration, calls.clone()));

            // Plan mode: a `present_plan` call is the explicit "plan is ready" signal — surface the
            // plan for approval and end the planning turn without executing anything. Every call in the
            // turn gets a tool result so the round stays a valid OpenAI tool exchange (each `tool_call`
            // must be answered before the next message).
            if mode == ApprovalMode::Plan
                && let Some(plan_call) =
                    calls.iter().find(|call| call.function.name == PRESENT_PLAN)
            {
                let plan = extract_plan(plan_call).unwrap_or(content);
                for call in &calls {
                    let response_message = if call.function.name == PRESENT_PLAN {
                        "Plano apresentado ao usuário para aprovação."
                    } else {
                        "ignorada: present_plan encerra o turno"
                    };
                    conversation.push(Message::tool_result(
                        call.id.as_str(),
                        response_message.to_string(),
                    ));
                }
                return Ok(TurnOutcome::PlanProposed(plan));
            }

            for (index, call) in calls.iter().enumerate() {
                // Runaway call-cap checkpoint, enforced WITHIN the round. A single assistant message can
                // carry an unbounded number of tool calls; checking only between rounds would let one
                // round run them all (unattended in auto — e.g. a prompt-injected burst of write_file)
                // before any pause. When the cap is reached, confirm before executing the next call.
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
                        CheckpointFlow::Continue => {}
                        CheckpointFlow::End(TurnOutcome::Aborted) => {
                            answer_unanswered(
                                conversation,
                                &calls[index..],
                                "ignorada: sessão encerrada",
                            );
                            return Ok(TurnOutcome::Aborted);
                        }
                        CheckpointFlow::End(outcome) => {
                            answer_unanswered(
                                conversation,
                                &calls[index..],
                                "ignorada: execução interrompida no checkpoint",
                            );
                            return Ok(outcome);
                        }
                    }
                }

                // The display command for this call, shown in every mode so the user sees each action
                // even under auto. Falls back to the tool name if the args do not parse.
                let command = self
                    .registry
                    .command_line(sandbox, call)
                    .unwrap_or_else(|| call.function.name.clone());

                let result = self
                    .decide_and_run(sandbox, call, &command, &mut mode, io)
                    .await;

                let Some((outcome, elapsed)) = result else {
                    // The user aborted at this call: answer it and every remaining call so the assistant
                    // `tool_calls` message stays a fully-answered (valid, persistable) exchange, then end.
                    answer_unanswered(
                        conversation,
                        &calls[index..],
                        "ignorada: interrompida pelo usuário",
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

            // After the round, fire on either guard: a long wall-clock turn, or the tool-call count
            // since the last check-in. The call-cap leg here catches accumulation ACROSS rounds; the
            // same cap is also enforced WITHIN the loop above so one oversized round cannot bypass it.
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
                    CheckpointFlow::Continue => {}
                    CheckpointFlow::End(outcome) => return Ok(outcome),
                }
            }
        }
    }

    /// The per-call decision tree for one tool call, factored out of `run` to collapse its deepest
    /// nesting. Emits `tool_started` just before running (paired with `tool_finished` in `run`) so the
    /// transcript records every attempt — including declined and plan-blocked calls. Switches `mode` to
    /// `Auto` when the user approves with "don't ask again" in `Default`. Returns `None` when the user
    /// aborts at this call, signalling `run` to answer the remaining calls and end the turn.
    async fn decide_and_run<IO: EventSink + Presenter + ApprovalPolicy + ToolObserver>(
        &self,
        sandbox: &dyn Sandbox,
        call: &ToolCall,
        command: &str,
        mode: &mut ApprovalMode,
        io: &mut IO,
    ) -> Option<(ToolOutcome, Duration)> {
        match *mode {
            // Auto: run calls without asking — EXCEPT high-blast-radius tools (run_command,
            // delete_*, move_path) and any out-of-root target, which still require a live
            // confirmation. This is what keeps an unattended turn — including a prompt-injected
            // one — from silently destroying data or reaching outside the workspace, and it is
            // the only such guard on platforms without an OS sandbox.
            ApprovalMode::Auto => match self.registry.confirm(sandbox, call) {
                Some(confirmation)
                    if self.registry.confirm_in_auto(&call.function.name)
                        || !confirmation.default_accept =>
                {
                    match io.decide(&confirmation).await {
                        // Already in auto, so "approve and don't ask again" is just "approve":
                        // destructive calls keep being confirmed for the rest of the turn.
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
            },
            // Plan: non-plannable tools are withheld from the schema; if the model still names
            // one, refuse it without touching the filesystem. Plannable tools (read-only +
            // run_command) run directly so the agent can investigate while drafting its plan.
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
            ApprovalMode::Plan => {
                if let Some(reason) = self.registry.plan_check(sandbox, call) {
                    io.tool_started(call, command);
                    Some((ToolOutcome::Error(reason), Duration::ZERO))
                } else {
                    io.tool_started(call, command);
                    Some(timed(self.registry.execute(sandbox, call)).await)
                }
            }
            // Default: confirm each call through the UI before running it.
            ApprovalMode::Default => match self.registry.confirm(sandbox, call) {
                Some(confirmation) => match io.decide(&confirmation).await {
                    Approval::Approved => {
                        io.tool_started(call, command);
                        Some(timed(self.registry.execute(sandbox, call)).await)
                    }
                    // "Approve and don't ask again": run this call, then switch the rest of the
                    // turn to auto so the following calls no longer prompt.
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

    /// The shared approval arms of a runaway checkpoint, used by both the within-round and post-round
    /// guards. `Approved` resets the checkpoint clock and call counter; `ApprovedAuto` additionally
    /// switches the turn to `Auto`; `Declined`/`Aborted` end the turn (the caller adds its own
    /// end-of-turn bookkeeping — the within-round case answers the remaining calls first).
    fn checkpoint_transition(
        &self,
        decision: Approval,
        checkpoint: &mut Instant,
        calls_since_checkpoint: &mut usize,
        mode: &mut ApprovalMode,
    ) -> CheckpointFlow {
        match decision {
            Approval::Approved => {
                *checkpoint = Instant::now();
                *calls_since_checkpoint = 0;
                CheckpointFlow::Continue
            }
            // "Keep going, and don't ask again": resume and run the rest of the turn unattended.
            Approval::ApprovedAuto => {
                *mode = ApprovalMode::Auto;
                *checkpoint = Instant::now();
                *calls_since_checkpoint = 0;
                CheckpointFlow::Continue
            }
            Approval::Declined => CheckpointFlow::End(TurnOutcome::Completed),
            Approval::Aborted => CheckpointFlow::End(TurnOutcome::Aborted),
        }
    }
}

#[cfg(test)]
mod tests;
