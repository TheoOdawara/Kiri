use anyhow::Result;

use crate::modules::agent::application::agent_loop::{AgentLoop, TurnOutcome};
use crate::modules::agent::application::presenter::Presenter;
use crate::modules::agent::domain::conversation::Conversation;
use crate::modules::agent::domain::message::Message;
use crate::modules::repl::infrastructure::terminal::Terminal;
use crate::modules::tools::infrastructure::sandbox::{
    Sandbox, expand_user_path, is_absolute_target,
};

/// The interactive REPL driving adapter: read user input, handle slash commands (`/exit`, `/sair`,
/// `/cd`), and drive one `AgentLoop` per message. Owns the terminal, the active (movable) sandbox, and
/// the conversation.
pub struct Repl {
    agent_loop: AgentLoop,
    sandbox: Sandbox,
    conversation: Conversation,
    terminal: Terminal,
    seed: Option<String>,
}

impl Repl {
    pub fn new(
        agent_loop: AgentLoop,
        sandbox: Sandbox,
        system_prompt: &str,
        seed: Option<String>,
    ) -> Self {
        Self {
            agent_loop,
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

            let prompt = match classify(&input) {
                Command::Empty => continue,
                Command::Exit => break,
                Command::ShowWorkspace => {
                    self.show_workspace();
                    continue;
                }
                Command::ChangeWorkspace(arg) => {
                    self.change_workspace(arg);
                    continue;
                }
                Command::Prompt(text) => text,
            };

            self.conversation.push(Message::user(prompt));
            match self
                .agent_loop
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

/// One classified line of REPL input (borrows the trimmed input).
enum Command<'a> {
    /// Blank line: prompt again.
    Empty,
    /// `/exit` or `/sair`: end the session.
    Exit,
    /// Bare `/cd`: print the active workspace.
    ShowWorkspace,
    /// `/cd <path>`: move the active workspace.
    ChangeWorkspace(&'a str),
    /// Anything else: a turn prompt for the model.
    Prompt(&'a str),
}

/// Classify one input line by its first token. The input is trimmed first; a `/cd` argument is trimmed
/// too. A `/cd`-looking token without a space (e.g. `/cdfoo`) falls through to a model prompt.
fn classify(input: &str) -> Command<'_> {
    let input = input.trim();
    if input.is_empty() {
        return Command::Empty;
    }
    if let Some(("/cd", arg)) = input.split_once(' ') {
        let arg = arg.trim();
        return if arg.is_empty() {
            Command::ShowWorkspace
        } else {
            Command::ChangeWorkspace(arg)
        };
    }
    match input {
        "/exit" | "/sair" => Command::Exit,
        "/cd" => Command::ShowWorkspace,
        _ => Command::Prompt(input),
    }
}

#[cfg(test)]
mod tests {
    use super::{Command, classify};

    #[test]
    fn exit_aliases_end_the_session() {
        assert!(matches!(classify("/exit"), Command::Exit));
        assert!(matches!(classify("/sair"), Command::Exit));
        assert!(matches!(classify("  /exit  "), Command::Exit));
    }

    #[test]
    fn bare_cd_shows_the_workspace() {
        assert!(matches!(classify("/cd"), Command::ShowWorkspace));
        assert!(matches!(classify("/cd   "), Command::ShowWorkspace));
    }

    #[test]
    fn cd_with_argument_changes_the_workspace() {
        assert!(matches!(
            classify("/cd src"),
            Command::ChangeWorkspace("src")
        ));
        assert!(matches!(
            classify("/cd   src"),
            Command::ChangeWorkspace("src")
        ));
        assert!(matches!(
            classify("/cd ~/dev"),
            Command::ChangeWorkspace("~/dev")
        ));
    }

    #[test]
    fn empty_input_is_empty() {
        assert!(matches!(classify(""), Command::Empty));
        assert!(matches!(classify("   "), Command::Empty));
    }

    #[test]
    fn unknown_slash_token_and_text_are_prompts() {
        assert!(matches!(classify("/cdfoo"), Command::Prompt("/cdfoo")));
        assert!(matches!(
            classify("/exit now"),
            Command::Prompt("/exit now")
        ));
        assert!(matches!(classify("hello"), Command::Prompt("hello")));
        assert!(matches!(
            classify("  hello world  "),
            Command::Prompt("hello world")
        ));
    }
}
