use crate::shared::kernel::approval_mode::ApprovalMode;

/// A slash command parsed from a submitted input line. A line that does not start with `/` is a model
/// prompt (the parser returns `None`); a `/`-prefixed line that matches nothing is `Unknown`, so the UI
/// can warn instead of silently sending it to the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// End the session.
    Quit,
    /// Discard the conversation and start fresh.
    NewSession,
    /// Show the available commands.
    Help,
    /// Switch the approval mode.
    SetMode(ApprovalMode),
    /// `/cd`: show the active workspace (`None`) or move it to a path (`Some`).
    ChangeWorkspace(Option<String>),
    /// `/models`: open the picker to switch the active model (from the active provider's catalog).
    Models,
    /// `/effort`: open the picker to switch the reasoning effort.
    Effort,
    /// `/provider`: open the picker to switch the active provider.
    Provider,
    /// `/resume`: reopen the most recent session for this workspace.
    Resume,
    /// `/sessions`: open the picker to choose a past session to reopen.
    Sessions,
    /// `/sync`: push the portable profile (config + memory) to the configured private repo.
    Sync,
    /// A `/`-prefixed token that is not a known command.
    Unknown,
}

/// Parse a submitted line. `None` means it is a model prompt (or blank); the caller decides. A line that
/// starts with `/` is always a command — a matched one, or `Unknown` when nothing matches. The first
/// whitespace splits the command from its argument (only `/cd` uses one).
pub fn parse(line: &str) -> Option<Command> {
    let line = line.trim();
    if line.is_empty() || !line.starts_with('/') {
        return None;
    }
    let (head, arg) = match line.split_once(char::is_whitespace) {
        Some((head, arg)) => (head, arg.trim()),
        None => (line, ""),
    };
    let command = match head {
        "/exit" | "/sair" | "/quit" => Command::Quit,
        "/new" | "/novo" => Command::NewSession,
        "/help" | "/ajuda" => Command::Help,
        "/plan" => Command::SetMode(ApprovalMode::Plan),
        "/auto" => Command::SetMode(ApprovalMode::Auto),
        "/default" | "/normal" => Command::SetMode(ApprovalMode::Default),
        "/cd" => Command::ChangeWorkspace((!arg.is_empty()).then(|| arg.to_string())),
        "/models" | "/modelos" => Command::Models,
        "/effort" | "/esforco" => Command::Effort,
        "/provider" | "/providers" => Command::Provider,
        "/resume" | "/retomar" => Command::Resume,
        "/sessions" | "/sessoes" => Command::Sessions,
        "/sync" => Command::Sync,
        _ => Command::Unknown,
    };
    Some(command)
}

/// The one-screen help shown by `/help`, listing every command and the mode shortcut.
pub fn help_text() -> String {
    [
        "Comandos:",
        "  /new           descarta a conversa e começa uma nova sessão",
        "  /resume        retoma a sessão mais recente deste workspace",
        "  /sessions      escolhe uma sessão anterior para retomar",
        "  /sync          envia config + memória ao seu repo privado (push)",
        "  /plan          modo plan (só leitura; planeja e executa após aprovação)",
        "  /auto          modo auto (executa tudo sem pedir aprovação)",
        "  /default       modo default (pede aprovação para cada ação)",
        "  /cd [caminho]  mostra ou muda o workspace ativo",
        "  /provider      troca o provider ativo (ou adiciona um novo)",
        "  /models        troca o modelo ativo",
        "  /effort        troca o nível de esforço (reasoning)",
        "  /help          mostra esta ajuda",
        "  /exit          encerra a sessão",
        "Shift+Tab alterna entre os modos (default → auto → plan).",
        "Edição: setas/Home/End, Ctrl+setas (palavra), Shift+seleção, Ctrl+A/C/X/V, Ctrl+Z/Y.",
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quit_aliases_parse() {
        for q in ["/exit", "/sair", "/quit", "  /exit  "] {
            assert_eq!(parse(q), Some(Command::Quit));
        }
    }

    #[test]
    fn plain_text_and_blank_are_prompts() {
        assert_eq!(parse("hello"), None);
        assert_eq!(parse("  hello world  "), None);
        assert_eq!(parse(""), None);
        assert_eq!(parse("   "), None);
    }

    #[test]
    fn unknown_slash_tokens_are_commands_not_prompts() {
        assert_eq!(parse("/exitnow"), Some(Command::Unknown));
        assert_eq!(parse("/foo"), Some(Command::Unknown));
    }

    #[test]
    fn session_and_help_aliases_parse() {
        assert_eq!(parse("/new"), Some(Command::NewSession));
        assert_eq!(parse("/novo"), Some(Command::NewSession));
        assert_eq!(parse("/help"), Some(Command::Help));
    }

    #[test]
    fn resume_and_sessions_aliases_parse() {
        assert_eq!(parse("/resume"), Some(Command::Resume));
        assert_eq!(parse("/retomar"), Some(Command::Resume));
        assert_eq!(parse("/sessions"), Some(Command::Sessions));
        assert_eq!(parse("/sessoes"), Some(Command::Sessions));
    }

    #[test]
    fn sync_parses() {
        assert_eq!(parse("/sync"), Some(Command::Sync));
    }

    #[test]
    fn paste_is_now_unknown() {
        assert_eq!(parse("/paste"), Some(Command::Unknown));
        assert_eq!(parse("/colar"), Some(Command::Unknown));
    }

    #[test]
    fn mode_commands_parse() {
        assert_eq!(parse("/plan"), Some(Command::SetMode(ApprovalMode::Plan)));
        assert_eq!(parse("/auto"), Some(Command::SetMode(ApprovalMode::Auto)));
        assert_eq!(
            parse("/default"),
            Some(Command::SetMode(ApprovalMode::Default))
        );
    }

    #[test]
    fn models_effort_and_provider_parse() {
        assert_eq!(parse("/models"), Some(Command::Models));
        assert_eq!(parse("/modelos"), Some(Command::Models));
        assert_eq!(parse("/effort"), Some(Command::Effort));
        assert_eq!(parse("/esforco"), Some(Command::Effort));
        assert_eq!(parse("/provider"), Some(Command::Provider));
        assert_eq!(parse("/providers"), Some(Command::Provider));
    }

    #[test]
    fn cd_shows_or_changes_the_workspace() {
        assert_eq!(parse("/cd"), Some(Command::ChangeWorkspace(None)));
        assert_eq!(parse("/cd   "), Some(Command::ChangeWorkspace(None)));
        assert_eq!(
            parse("/cd src"),
            Some(Command::ChangeWorkspace(Some("src".to_string())))
        );
        assert_eq!(
            parse("/cd ~/dev"),
            Some(Command::ChangeWorkspace(Some("~/dev".to_string())))
        );
    }
}
