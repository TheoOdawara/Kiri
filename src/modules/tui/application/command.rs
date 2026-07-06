use crate::modules::tui::domain::command_menu::COMMANDS;
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
    /// `/instructions`: show the active instructions files and their merged content.
    Instructions,
    /// `/rules`: show the loaded extension rules (id, layer, always-on).
    Rules,
    /// `/commands`: show the loaded extension custom commands (name, aliases, layer, source path).
    Commands,
    /// `/agents`: show the loaded agent profiles (id, layer, source path).
    Agents,
    /// `/skills`: show the loaded skills (id, tags, layer, source path).
    Skills,
    /// `/hooks`: show the loaded hooks (id, event, layer, source path).
    Hooks,
    /// `/approve-hook <id>`: approve a pending project-layer hook (ADR 0021 TOFU gate).
    ApproveHook(String),
    /// `/mcp`: show the loaded MCP servers (id, command, layer, source path).
    Mcp,
    /// `/approve-mcp <id>`: approve a pending project-layer MCP server (ADR 0021 TOFU gate).
    ApproveMcp(String),
    /// A `/`-prefixed token that matches no built-in — carries the raw head token so the runtime can
    /// still resolve it against the extension-provided custom commands before reporting it unknown.
    Unknown(String),
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
        "/instructions" | "/instrucoes" => Command::Instructions,
        "/rules" | "/regras" => Command::Rules,
        "/commands" | "/comandos" => Command::Commands,
        "/agents" | "/agentes" => Command::Agents,
        "/skills" => Command::Skills,
        "/hooks" => Command::Hooks,
        "/approve-hook" => Command::ApproveHook(arg.to_string()),
        "/mcp" => Command::Mcp,
        "/approve-mcp" => Command::ApproveMcp(arg.to_string()),
        _ => Command::Unknown(head.to_string()),
    };
    Some(command)
}

/// The one-screen help shown by `/help`. The per-command rows are derived from the single `COMMANDS`
/// catalog so the help and the live preview menu can never list different commands or blurbs; only the
/// header, the mode shortcut, and the editing hint are help-specific framing. Command names are ASCII, so
/// `len()` is their display width for the alignment padding (no infra `display_width` dependency from this
/// application-layer module).
pub fn help_text() -> String {
    let name_width = COMMANDS
        .iter()
        .map(|spec| spec.name.len())
        .max()
        .unwrap_or(0);
    let mut lines = vec!["Comandos:".to_string()];
    for spec in COMMANDS {
        let pad = " ".repeat(name_width - spec.name.len());
        lines.push(format!("  {}{pad}  {}", spec.name, spec.blurb));
    }
    lines.push("Shift+Tab alterna entre os modos (default → auto → plan).".to_string());
    lines.push(
        "Edição: setas/Home/End, Ctrl+setas (palavra), Shift+seleção, Ctrl+A/C/X/V, Ctrl+Z/Y."
            .to_string(),
    );
    lines.join("\n")
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
        assert_eq!(
            parse("/exitnow"),
            Some(Command::Unknown("/exitnow".to_string()))
        );
        assert_eq!(parse("/foo"), Some(Command::Unknown("/foo".to_string())));
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
    fn instructions_parses() {
        assert_eq!(parse("/instructions"), Some(Command::Instructions));
        assert_eq!(parse("/instrucoes"), Some(Command::Instructions));
    }

    #[test]
    fn paste_is_now_unknown() {
        assert_eq!(
            parse("/paste"),
            Some(Command::Unknown("/paste".to_string()))
        );
        assert_eq!(
            parse("/colar"),
            Some(Command::Unknown("/colar".to_string()))
        );
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
    fn help_text_is_derived_from_catalog() {
        let help = help_text();
        for spec in COMMANDS {
            assert!(help.contains(spec.name), "help omits {}", spec.name);
            assert!(
                help.contains(spec.blurb),
                "help omits the blurb for {}",
                spec.name
            );
        }
    }

    #[test]
    fn catalog_aliases_match_parser() {
        use std::collections::BTreeSet;

        // Every catalog token (canonical name + aliases) must be a real parser synonym mapping to the
        // same command as its name, so the menu never advertises a token the parser would reject.
        for spec in COMMANDS {
            let canonical = parse(spec.name).expect("catalog name parses");
            assert!(!matches!(canonical, Command::Unknown(_)));
            for alias in spec.aliases {
                assert_eq!(
                    parse(alias),
                    Some(canonical.clone()),
                    "alias {alias} desynced from {}",
                    spec.name
                );
            }
        }

        // The catalog token set must equal the parser's arm set. This list mirrors the `parse` match
        // arms; comparing it to the catalog locks the two against drift (an alias added to one but not the
        // other fails here — the guard TUIC-06 asks for while the parser keeps its explicit arms).
        let catalog: BTreeSet<&str> = COMMANDS
            .iter()
            .flat_map(|spec| std::iter::once(spec.name).chain(spec.aliases.iter().copied()))
            .collect();
        let parser_arms: BTreeSet<&str> = [
            "/exit",
            "/sair",
            "/quit",
            "/new",
            "/novo",
            "/help",
            "/ajuda",
            "/plan",
            "/auto",
            "/default",
            "/normal",
            "/cd",
            "/models",
            "/modelos",
            "/effort",
            "/esforco",
            "/provider",
            "/providers",
            "/resume",
            "/retomar",
            "/sessions",
            "/sessoes",
            "/sync",
            "/instructions",
            "/instrucoes",
            "/rules",
            "/regras",
            "/commands",
            "/comandos",
            "/agents",
            "/agentes",
            "/skills",
            "/hooks",
            "/approve-hook",
            "/mcp",
            "/approve-mcp",
        ]
        .into_iter()
        .collect();
        assert_eq!(
            catalog, parser_arms,
            "catalog tokens and parser arms drifted"
        );
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
