use std::io::{self, IsTerminal, Write};
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, BufReader, Lines, Stdin};

use crate::modules::agent::application::approval_policy::{Approval, ApprovalPolicy};
use crate::modules::agent::application::presenter::Presenter;
use crate::modules::agent::domain::stream_event::StreamEvent;
use crate::modules::provider::application::completion_provider::EventSink;
use crate::modules::tools::application::tool::Confirmation;
use crate::shared::kernel::error::AgentError;

const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";
const CLEAR_LINE: &str = "\r\x1b[K";
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const SPINNER_INTERVAL: Duration = Duration::from_millis(80);

/// All terminal I/O for the REPL: the single stdin line reader, stdout, and the per-turn spinner
/// state. Implements the engine's UI ports — `EventSink` (render streamed deltas), `Presenter` (finish
/// a turn, emit notices), and `ApprovalPolicy` (prompt for a decision) — plus the inherent helpers the
/// REPL uses to read user input. Owning every handle keeps the engine free of direct console access.
pub struct Terminal {
    reader: Lines<BufReader<Stdin>>,
    stdout: io::Stdout,
    is_tty: bool,
    started: Instant,
    last_tick: Instant,
    frame: usize,
    answering: bool,
}

impl Terminal {
    pub fn new() -> Self {
        let reader = BufReader::new(tokio::io::stdin()).lines();
        let stdout = io::stdout();
        let is_tty = stdout.is_terminal();
        let now = Instant::now();
        Self {
            reader,
            stdout,
            is_tty,
            started: now,
            last_tick: now,
            frame: 0,
            answering: false,
        }
    }

    /// Write a prompt label (no newline) and flush — e.g. the `você ›` input prompt.
    pub fn prompt(&mut self, label: &str) -> io::Result<()> {
        write!(self.stdout, "{label}")?;
        self.stdout.flush()
    }

    /// Read one line of user input. `None` when stdin is closed.
    pub async fn read_line(&mut self) -> io::Result<Option<String>> {
        self.reader.next_line().await
    }

    /// Read a yes/no answer and map it through `answer_approves`. `Aborted` when the input stream ends
    /// or a read fails.
    async fn read_decision(&mut self, default_accept: bool) -> Approval {
        match self.read_line().await {
            Ok(Some(answer)) if answer_approves(&answer, default_accept) => Approval::Approved,
            Ok(Some(_)) => Approval::Declined,
            Ok(None) | Err(_) => Approval::Aborted,
        }
    }
}

impl Default for Terminal {
    fn default() -> Self {
        Self::new()
    }
}

impl EventSink for Terminal {
    fn on_event(&mut self, event: StreamEvent) -> Result<(), AgentError> {
        match event {
            StreamEvent::Reasoning(_) => {
                if !self.answering && self.is_tty && self.last_tick.elapsed() >= SPINNER_INTERVAL {
                    self.frame = (self.frame + 1) % SPINNER.len();
                    self.last_tick = Instant::now();
                    let secs = self.started.elapsed().as_secs();
                    write!(
                        self.stdout,
                        "{CLEAR_LINE}{DIM}{} pensando… ({secs}s){RESET}",
                        SPINNER[self.frame]
                    )?;
                    self.stdout.flush()?;
                }
            }
            StreamEvent::Content(text) => {
                if !self.answering {
                    self.answering = true;
                    if self.is_tty {
                        write!(self.stdout, "{CLEAR_LINE}")?;
                    }
                }
                write!(self.stdout, "{text}")?;
                self.stdout.flush()?;
            }
        }
        Ok(())
    }
}

impl Presenter for Terminal {
    fn begin_turn(&mut self) {
        let now = Instant::now();
        self.started = now;
        self.last_tick = now;
        self.frame = 0;
        self.answering = false;
    }

    fn finish_turn(&mut self) -> Result<(), AgentError> {
        // In a terminal, erase a leftover spinner if the stream ended mid-reasoning, and never leave
        // the terminal dimmed. When piped, keep the output free of escape codes.
        if self.is_tty {
            if !self.answering {
                let _ = write!(self.stdout, "{CLEAR_LINE}");
            }
            let _ = write!(self.stdout, "{RESET}");
        }
        let _ = writeln!(self.stdout);
        let _ = self.stdout.flush();
        Ok(())
    }

    fn notice(&mut self, line: &str) {
        let _ = writeln!(self.stdout, "{line}");
        let _ = self.stdout.flush();
    }
}

#[async_trait::async_trait(?Send)]
impl ApprovalPolicy for Terminal {
    async fn decide(&mut self, confirmation: &Confirmation) -> Approval {
        if self.prompt(&confirmation.prompt).is_err() {
            return Approval::Aborted;
        }
        self.read_decision(confirmation.default_accept).await
    }

    async fn confirm_continue(&mut self, minutes: u64) -> Approval {
        let line = format!("Execução já dura ~{minutes}min. Continuar? [S/n] ");
        if self.prompt(&line).is_err() {
            return Approval::Aborted;
        }
        self.read_decision(true).await
    }
}

/// Interpret a confirmation answer. An explicit yes/no always wins; an empty or unrecognized answer
/// follows `default_accept` — `[S/n]` (accept) inside the workspace, `[s/N]` (decline) for out-of-root
/// operations.
fn answer_approves(answer: &str, default_accept: bool) -> bool {
    match answer.trim().to_lowercase().as_str() {
        "s" | "sim" | "y" | "yes" => true,
        "n" | "nao" | "não" | "no" => false,
        _ => default_accept,
    }
}

#[cfg(test)]
mod tests {
    use super::answer_approves;

    #[test]
    fn answer_approves_follows_default_for_empty_and_unknown() {
        // Explicit yes/no override the default either way.
        for yes in ["s", "sim", "S", "y", "yes"] {
            assert!(answer_approves(yes, false), "{yes:?} should approve");
        }
        for no in ["n", "N", "nao", "não", "no", " NÃO "] {
            assert!(!answer_approves(no, true), "{no:?} should decline");
        }
        // Empty/unrecognized follow the default.
        assert!(answer_approves("", true));
        assert!(answer_approves("ok", true));
        assert!(!answer_approves("", false));
        assert!(!answer_approves("ok", false));
    }
}
