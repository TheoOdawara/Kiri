/// A slash-command parsed from a submitted input line. The grammar grows in later phases; for now it
/// only covers ending the session, so everything else is a model prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Quit,
}

/// Parse a submitted line. `None` means it is a model prompt (or blank); the caller decides.
pub fn parse(line: &str) -> Option<Command> {
    match line.trim() {
        "/exit" | "/sair" | "/quit" => Some(Command::Quit),
        _ => None,
    }
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
    fn plain_text_and_unknown_slash_are_prompts() {
        assert_eq!(parse("hello"), None);
        assert_eq!(parse("/exitnow"), None);
        assert_eq!(parse(""), None);
    }
}
