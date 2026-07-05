//! The slash-command catalog and the live preview menu state. Pure data and pure logic — no I/O, no
//! knowledge of the `Command` enum (mapping a name to a behaviour belongs to `application::command`).
//! The preview filters by the canonical name, by any alias, or by a substring of the one-line blurb.

use crate::modules::tui::domain::nav::wrapping_step;

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
        name: "/resume",
        aliases: &["/retomar"],
        blurb: "retoma a sessão mais recente deste workspace",
    },
    CommandSpec {
        name: "/sessions",
        aliases: &["/sessoes"],
        blurb: "escolhe uma sessão anterior para retomar",
    },
    CommandSpec {
        name: "/sync",
        aliases: &[],
        blurb: "envia config + memória ao seu repo privado (push)",
    },
    CommandSpec {
        name: "/instructions",
        aliases: &["/instrucoes"],
        blurb: "exibe as instruções ativas e os arquivos carregados",
    },
    CommandSpec {
        name: "/rules",
        aliases: &["/regras"],
        blurb: "exibe as regras carregadas (rules globais/projeto)",
    },
    CommandSpec {
        name: "/commands",
        aliases: &["/comandos"],
        blurb: "exibe os comandos custom carregados (globais/projeto)",
    },
    CommandSpec {
        name: "/agents",
        aliases: &["/agentes"],
        blurb: "exibe os perfis de agente carregados",
    },
    CommandSpec {
        name: "/skills",
        aliases: &[],
        blurb: "exibe as skills carregadas (use_skill busca o conteúdo)",
    },
    CommandSpec {
        name: "/hooks",
        aliases: &[],
        blurb: "exibe os hooks carregados e seu estado de aprovação",
    },
    CommandSpec {
        name: "/approve-hook",
        aliases: &[],
        blurb: "aprova um hook de projeto pendente: /approve-hook <id>",
    },
    CommandSpec {
        name: "/mcp",
        aliases: &[],
        blurb: "exibe os servidores MCP carregados e seu estado de aprovação",
    },
    CommandSpec {
        name: "/approve-mcp",
        aliases: &[],
        blurb: "aprova um servidor MCP de projeto pendente: /approve-mcp <id>",
    },
    CommandSpec {
        name: "/plan",
        aliases: &[],
        blurb: "modo plan (só leitura; planeja e executa após aprovação)",
    },
    CommandSpec {
        name: "/auto",
        aliases: &[],
        blurb: "modo auto (executa tudo sem pedir aprovação)",
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
        name: "/provider",
        aliases: &["/providers"],
        blurb: "troca o provider ativo (ou adiciona um novo)",
    },
    CommandSpec {
        name: "/models",
        aliases: &["/modelos"],
        blurb: "troca o modelo ativo",
    },
    CommandSpec {
        name: "/effort",
        aliases: &["/esforco"],
        blurb: "troca o nível de esforço (reasoning)",
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

/// One extension-provided custom slash command (ADR 0021) surfaced in the live preview alongside the
/// built-ins. Aliases still resolve at submit time via the extensions catalog; the preview matches on the
/// canonical name only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomCommandEntry {
    pub name: String,
    pub blurb: String,
}

fn filter_custom(prefix: &str, custom: &[CustomCommandEntry]) -> Vec<usize> {
    let query = prefix.trim_start_matches('/').to_ascii_lowercase();
    custom
        .iter()
        .enumerate()
        .filter(|(_, entry)| {
            entry
                .name
                .trim_start_matches('/')
                .to_ascii_lowercase()
                .starts_with(&query)
        })
        .map(|(i, _)| i)
        .collect()
}

/// A resolved preview row: a built-in command or an extension-provided one. Both display the same way;
/// only the source array differs.
#[derive(Debug, Clone, Copy)]
pub enum MenuEntry<'a> {
    Static(&'static CommandSpec),
    Custom(&'a CustomCommandEntry),
}

impl MenuEntry<'_> {
    pub fn name(&self) -> &str {
        match self {
            Self::Static(spec) => spec.name,
            Self::Custom(entry) => &entry.name,
        }
    }

    pub fn blurb(&self) -> &str {
        match self {
            Self::Static(spec) => spec.blurb,
            Self::Custom(entry) => &entry.blurb,
        }
    }
}

/// Which catalog a filtered row's index resolves against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Static(usize),
    Custom(usize),
}

fn recompute(prefix: &str, custom: &[CustomCommandEntry]) -> Vec<Source> {
    let mut sources: Vec<Source> = filter(prefix).into_iter().map(Source::Static).collect();
    sources.extend(
        filter_custom(prefix, custom)
            .into_iter()
            .map(Source::Custom),
    );
    sources
}

/// The live preview shown while the buffer is a slash command in progress. Open it from `sync_menu`
/// whenever the input starts with `/` and has no whitespace yet (a space ends the command name and
/// starts the arguments, so the menu closes). `filtered` resolves against the built-in catalog or the
/// snapshot of extension-provided commands taken at `open`; `selected` is the highlighted row, wrapped on
/// `move_cursor` to stay in range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandMenu {
    custom: Vec<CustomCommandEntry>,
    filtered: Vec<Source>,
    selected: usize,
}

impl CommandMenu {
    /// Open with the filtered set for `prefix` and the first row highlighted. `custom` is snapshotted
    /// once here (extension commands are loaded once at boot, never change mid-session).
    pub fn open(prefix: &str, custom: &[CustomCommandEntry]) -> Self {
        let custom = custom.to_vec();
        let filtered = recompute(prefix, &custom);
        Self {
            custom,
            filtered,
            selected: 0,
        }
    }

    /// Recompute the filter for a new `prefix`, keeping the selection in range.
    pub fn refresh(&mut self, prefix: &str) {
        self.filtered = recompute(prefix, &self.custom);
        if !self.filtered.is_empty() && self.selected >= self.filtered.len() {
            self.selected = self.filtered.len() - 1;
        } else if self.filtered.is_empty() {
            self.selected = 0;
        }
    }

    /// Move the highlight by `delta` rows, wrapping within the filtered set.
    pub fn move_cursor(&mut self, delta: i32) {
        self.selected = wrapping_step(self.selected, delta, self.filtered.len());
    }

    fn resolve(&self, source: Source) -> MenuEntry<'_> {
        match source {
            Source::Static(i) => MenuEntry::Static(&COMMANDS[i]),
            Source::Custom(i) => MenuEntry::Custom(&self.custom[i]),
        }
    }

    /// The entry of the highlighted row, if any.
    pub fn entry(&self) -> Option<MenuEntry<'_>> {
        self.filtered.get(self.selected).map(|&s| self.resolve(s))
    }

    /// The entry at a given display row, for rendering the whole filtered list.
    pub fn row(&self, row: usize) -> Option<MenuEntry<'_>> {
        self.filtered.get(row).map(|&s| self.resolve(s))
    }

    pub fn len(&self) -> usize {
        self.filtered.len()
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
        let mut m = CommandMenu::open("/", &[]);
        assert_eq!(m.selected(), 0);
        m.move_cursor(3);
        assert_eq!(m.selected(), 3);
        m.refresh("/new");
        assert_eq!(m.len(), 1);
        assert_eq!(m.selected(), 0);
    }

    #[test]
    fn move_cursor_wraps_in_both_directions() {
        let mut m = CommandMenu::open("/", &[]);
        let len = m.len();
        m.move_cursor(-(1));
        assert_eq!(m.selected(), len - 1);
        m.move_cursor(1);
        assert_eq!(m.selected(), 0);
    }

    #[test]
    fn entry_returns_the_highlighted_command() {
        let mut m = CommandMenu::open("/", &[]);
        m.move_cursor(1);
        assert_eq!(m.entry().unwrap().name(), COMMANDS[1].name);
    }

    #[test]
    fn custom_commands_are_filtered_and_shown_alongside_built_ins() {
        let custom = [CustomCommandEntry {
            name: "/test".to_string(),
            blurb: "run the suite".to_string(),
        }];
        let m = CommandMenu::open("/", &custom);
        assert_eq!(m.len(), COMMANDS.len() + 1);

        let m = CommandMenu::open("/te", &custom);
        assert_eq!(m.len(), 1);
        assert_eq!(m.entry().unwrap().name(), "/test");
        assert_eq!(m.entry().unwrap().blurb(), "run the suite");
    }

    /// Locks the invariant called out at the top of the file: every entry in `COMMANDS` must resolve
    /// to a real command via `application::command::parse`. If a future change drops a parser arm but
    /// leaves its catalog row — or vice versa — the test fails and the menu and the parser stop
    /// disagreeing silently.
    #[test]
    fn catalog_matches_parse() {
        use crate::modules::tui::application::command::{Command, parse};
        for spec in COMMANDS {
            let parsed = parse(spec.name).expect("catalog name must parse");
            assert!(
                !matches!(parsed, Command::Unknown(_)),
                "{spec:?} is in the catalog but parse() returned Unknown"
            );
        }
    }
}
