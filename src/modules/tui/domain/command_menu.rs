//! The slash-command catalog and the live preview menu state. Pure data and pure logic — no I/O, no
//! knowledge of the `Command` enum (mapping a name to a behaviour belongs to `application::command`).
//! The preview filters by the canonical name, by any alias, or by a substring of the one-line blurb.

/// One entry in the slash-command catalog. `name` is the canonical token shown in the preview and the
/// one Tab completes to; `aliases` are accepted synonyms at submit time; `blurb` is the short pt-BR
/// description shown next to the name.
#[derive(Debug, Clone, Copy)]
pub struct CommandSpec {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub blurb: &'static str,
}

/// The single source of truth for the preview, kept in sync with `application::command::parse` by the
/// shared test suite. Order is the display order of the menu.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "/new",
        aliases: &["/novo"],
        blurb: "descarta a conversa e começa uma nova sessão",
    },
    CommandSpec {
        name: "/plan",
        aliases: &[],
        blurb: "modo plan (planeja e executa após aprovação)",
    },
    CommandSpec {
        name: "/auto",
        aliases: &[],
        blurb: "modo auto (executa sem pedir aprovação)",
    },
    CommandSpec {
        name: "/default",
        aliases: &["/normal"],
        blurb: "modo default (pede aprovação para cada ação)",
    },
    CommandSpec {
        name: "/cd",
        aliases: &[],
        blurb: "mostra ou muda o workspace ativo",
    },
    CommandSpec {
        name: "/paste",
        aliases: &["/colar"],
        blurb: "cola imagem ou texto do clipboard no input",
    },
    CommandSpec {
        name: "/help",
        aliases: &["/ajuda"],
        blurb: "mostra esta ajuda",
    },
    CommandSpec {
        name: "/exit",
        aliases: &["/sair", "/quit"],
        blurb: "encerra a sessão",
    },
];

/// Return the indices of the commands whose canonical name or any alias starts with `prefix` (after
/// the leading slash is stripped). The one-line blurb is display-only — it never drives the filter, so
/// typing `/n` matches `/new` (and its alias `/novo`) rather than every command whose description
/// happens to contain the letter `n`. An empty query (just `/`) matches the whole catalog. Matching is
/// case-insensitive.
pub fn filter(prefix: &str) -> Vec<usize> {
    let query = prefix.trim_start_matches('/').to_ascii_lowercase();
    COMMANDS
        .iter()
        .enumerate()
        .filter(|(_, spec)| {
            let name_hit = spec.name[1..].to_ascii_lowercase().starts_with(&query);
            let alias_hit = spec
                .aliases
                .iter()
                .any(|a| a[1..].to_ascii_lowercase().starts_with(&query));
            name_hit || alias_hit
        })
        .map(|(i, _)| i)
        .collect()
}

/// The live preview shown while the buffer is a slash command in progress. Open it from `sync_menu`
/// whenever the input starts with `/` and has no whitespace yet (a space ends the command name and
/// starts the arguments, so the menu closes). `filtered` indexes into `COMMANDS`; `selected` is the
/// highlighted row, wrapped on `move_cursor` to stay in range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandMenu {
    filtered: Vec<usize>,
    selected: usize,
}

impl CommandMenu {
    /// Open with the filtered set for `prefix` and the first row highlighted.
    pub fn open(prefix: &str) -> Self {
        let filtered = filter(prefix);
        Self {
            filtered,
            selected: 0,
        }
    }

    /// Recompute the filter for a new `prefix`, keeping the selection in range.
    pub fn refresh(&mut self, prefix: &str) {
        self.filtered = filter(prefix);
        if !self.filtered.is_empty() && self.selected >= self.filtered.len() {
            self.selected = self.filtered.len() - 1;
        } else if self.filtered.is_empty() {
            self.selected = 0;
        }
    }

    /// Move the highlight by `delta` rows, wrapping within the filtered set.
    pub fn move_cursor(&mut self, delta: i32) {
        if self.filtered.is_empty() {
            self.selected = 0;
            return;
        }
        let len = self.filtered.len() as i32;
        let mut next = self.selected as i32 + delta;
        next = next.rem_euclid(len);
        self.selected = next as usize;
    }

    /// The spec of the highlighted row, if any.
    pub fn spec(&self) -> Option<&'static CommandSpec> {
        self.filtered.get(self.selected).map(|&i| &COMMANDS[i])
    }

    pub fn filtered(&self) -> &[usize] {
        &self.filtered
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn is_empty(&self) -> bool {
        self.filtered.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_slash_matches_everything() {
        assert_eq!(filter("/"), (0..COMMANDS.len()).collect::<Vec<_>>());
        assert_eq!(filter(""), (0..COMMANDS.len()).collect::<Vec<_>>());
    }

    #[test]
    fn name_prefix_filters_case_insensitively() {
        let idx = |name: &str| COMMANDS.iter().position(|s| s.name == name).unwrap();
        // `/N` matches `/new` by name and `/default` by its alias `/normal`.
        assert_eq!(filter("/N"), [idx("/new"), idx("/default")]);
        assert_eq!(filter("/EX"), [idx("/exit")]);
    }

    #[test]
    fn alias_prefix_filters_to_the_owning_command() {
        let idx = |name: &str| COMMANDS.iter().position(|s| s.name == name).unwrap();
        assert_eq!(filter("/nov"), [idx("/new")]);
        assert_eq!(filter("/nor"), [idx("/default")]);
    }

    #[test]
    fn blurb_is_display_only_and_never_drives_filtering() {
        let got = filter("/workspace");
        assert!(got.is_empty(), "blurb substring must not match: {got:?}");
    }

    #[test]
    fn no_match_returns_empty() {
        assert!(filter("/zzz").is_empty());
    }

    #[test]
    fn menu_refresh_keeps_selection_in_range() {
        let mut m = CommandMenu::open("/");
        assert_eq!(m.selected(), 0);
        m.move_cursor(3);
        assert_eq!(m.selected(), 3);
        m.refresh("/new");
        assert_eq!(m.filtered().len(), 1);
        assert_eq!(m.selected(), 0);
    }

    #[test]
    fn move_cursor_wraps_in_both_directions() {
        let mut m = CommandMenu::open("/");
        let len = m.filtered().len();
        m.move_cursor(-(1));
        assert_eq!(m.selected(), len - 1);
        m.move_cursor(1);
        assert_eq!(m.selected(), 0);
    }

    #[test]
    fn spec_returns_the_highlighted_command() {
        let mut m = CommandMenu::open("/");
        m.move_cursor(1);
        assert_eq!(m.spec().unwrap().name, COMMANDS[1].name);
    }
}
