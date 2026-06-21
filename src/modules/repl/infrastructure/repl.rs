use anyhow::Result;

use crate::modules::agent::application::presenter::Presenter;
use crate::modules::agent::application::run_turn::{RunTurn, TurnOutcome};
use crate::modules::agent::domain::conversation::Conversation;
use crate::modules::agent::domain::message::Message;
use crate::modules::repl::infrastructure::terminal::Terminal;
use crate::modules::tools::infrastructure::sandbox::{
    Sandbox, expand_user_path, is_absolute_target,
};

/// The interactive REPL driving adapter: read user input, handle slash commands (`/exit`, `/sair`,
/// `/cd`), and drive one `RunTurn` per message. Owns the terminal, the active (movable) sandbox, and
/// the conversation.
pub struct Repl {
    run_turn: RunTurn,
    sandbox: Sandbox,
    conversation: Conversation,
    terminal: Terminal,
    seed: Option<String>,
}

impl Repl {
    pub fn new(
        run_turn: RunTurn,
        sandbox: Sandbox,
        system_prompt: &str,
        seed: Option<String>,
    ) -> Self {
        Self {
            run_turn,
            sandbox,
            conversation: Conversation::new(system_prompt),
            terminal: Terminal::new(),
            seed,
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        self.show_workspace();

        loop {
            let input = match self.seed.take() {
                Some(prompt) => prompt,
                None => {
                    self.terminal.prompt("\nvocê › ")?;
                    match self.terminal.read_line().await? {
                        Some(line) => line,
                        None => break,
                    }
                }
            };

            let input = input.trim();
            if input.is_empty() {
                continue;
            }
            if matches!(input, "/exit" | "/sair") {
                break;
            }
            if input == "/cd" {
                self.show_workspace();
                continue;
            }
            if let Some(arg) = input.strip_prefix("/cd ") {
                self.change_workspace(arg.trim());
                continue;
            }

            self.conversation.push(Message::user(input));
            match self
                .run_turn
                .run(&mut self.conversation, &self.sandbox, &mut self.terminal)
                .await
            {
                Ok(TurnOutcome::Completed) => {}
                Ok(TurnOutcome::Aborted) => break, // stdin closed at a prompt: end the session
                Err(error) => {
                    eprintln!("erro: {error}");
                    self.conversation.rollback_dangling_user();
                }
            }
        }

        Ok(())
    }

    fn show_workspace(&mut self) {
        self.terminal
            .notice(&format!("workspace: {}", self.sandbox.root().display()));
    }

    /// Move the active workspace to `arg` (relative to the current root, or an absolute / `~` path).
    fn change_workspace(&mut self, arg: &str) {
        let target = if is_absolute_target(arg) {
            expand_user_path(arg)
        } else {
            self.sandbox.root().join(arg)
        };
        match Sandbox::new(&target) {
            Ok(new_sandbox) => {
                self.sandbox = new_sandbox;
                self.show_workspace();
            }
            Err(error) => eprintln!("erro: {error:#}"),
        }
    }
}
