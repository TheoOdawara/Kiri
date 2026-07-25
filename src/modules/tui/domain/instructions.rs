use std::path::Path;

/// The `/instructions` panel text: the discovered paths, then both layers' content. Display-only, so it
/// merges the layers freely — unlike the system prompt, which keeps them in separate trust-labelled
/// blocks (S3-1). `None` when no instructions file was found at boot, which is what suppresses the panel.
///
/// Lives here, not on `Settings`: the string is pt-BR UI copy, and `shared/infra` is the leaf every module
/// depends on — user-facing wording in it would make every consumer inherit this front-end's language.
pub fn instructions_display(
    paths: &[impl AsRef<Path>],
    global: Option<&str>,
    project: Option<&str>,
) -> Option<String> {
    if global.is_none() && project.is_none() {
        return None;
    }
    let header = paths
        .iter()
        .map(|path| format!("- {}", path.as_ref().display()))
        .collect::<Vec<_>>()
        .join("\n");
    let text = [global, project]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n\n");
    Some(format!("Arquivos carregados:\n{header}\n\n{text}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn no_instructions_yields_no_panel() {
        assert!(instructions_display(&Vec::<PathBuf>::new(), None, None).is_none());
    }

    #[test]
    fn both_layers_render_under_one_paths_header() {
        let paths = [PathBuf::from("/g/KIRI.md"), PathBuf::from("/p/CLAUDE.md")];
        let display =
            instructions_display(&paths, Some("global text"), Some("project text")).unwrap();
        assert!(display.starts_with("Arquivos carregados:\n- "));
        assert!(display.contains("KIRI.md"));
        assert!(display.contains("CLAUDE.md"));
        // Global before project, matching discovery order.
        assert!(display.find("global text").unwrap() < display.find("project text").unwrap());
    }

    #[test]
    fn one_layer_alone_still_renders() {
        let paths = [PathBuf::from("/p/AGENTS.md")];
        let display = instructions_display(&paths, None, Some("project only")).unwrap();
        assert!(display.contains("project only"));
        assert!(display.contains("AGENTS.md"));
    }
}
