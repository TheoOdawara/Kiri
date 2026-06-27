use super::*;

/// Submit the editor contents: a quit command ends the session; anything else non-blank starts a turn.
pub(super) fn submit(model: &mut Model) -> Vec<Effect> {
    if model.busy {
        return vec![];
    }
    let line = model.input.take();
    model.history.record(&line);
    model.scroll.pin();
    model.command_menu = None;
    match command::parse(&line) {
        Some(Command::Quit) => {
            model.should_quit = true;
            vec![Effect::Quit]
        }
        Some(Command::NewSession) => vec![Effect::NewSession],
        Some(Command::Resume) => vec![Effect::ResumeLast],
        Some(Command::Sessions) => vec![Effect::ListSessions],
        Some(Command::Sync) => vec![Effect::SyncPush],
        Some(Command::Help) => {
            model.transcript.push(TranscriptItem::Notice(
                NoticeLevel::Info,
                command::help_text(),
            ));
            vec![]
        }
        Some(Command::SetMode(mode)) => {
            model.approval_mode = mode;
            vec![]
        }
        Some(Command::ChangeWorkspace(None)) => {
            model.transcript.push(TranscriptItem::Notice(
                NoticeLevel::Info,
                format!("workspace: {}", model.status.workspace),
            ));
            vec![]
        }
        Some(Command::ChangeWorkspace(Some(path))) => vec![Effect::ChangeWorkspace(path)],
        Some(Command::Models) => open_models_picker(model),
        Some(Command::Effort) => open_effort_picker(model),
        Some(Command::Provider) => open_provider_picker(model),
        Some(Command::Unknown) => {
            model.transcript.push(TranscriptItem::Notice(
                NoticeLevel::Error,
                format!("comando desconhecido: {} (use /help)", line.trim()),
            ));
            vec![]
        }
        None if line.trim().is_empty() && model.attachments.is_empty() => vec![],
        None if model.unconfigured => {
            // No usable provider yet: never send to the null provider silently. Drop the staged images,
            // surface a clear notice, and re-open onboarding. `busy` is intentionally left false so no
            // turn is armed and the UI is not stranded.
            model.attachments.clear();
            model.transcript.push(TranscriptItem::Notice(
                NoticeLevel::Info,
                "configure um provider primeiro — escolha um e informe a API key".to_string(),
            ));
            model.wizard = Some(ProviderWizard::onboarding());
            vec![]
        }
        None => {
            // Drain the staged images into the prompt; a turn can carry text, images, or both.
            let images: Vec<String> = std::mem::take(&mut model.attachments)
                .into_iter()
                .map(|attachment| attachment.data_url)
                .collect();
            let label = if line.trim().is_empty() {
                format!("🖼 {} imagem(ns)", images.len())
            } else {
                line.clone()
            };
            model.transcript.push(TranscriptItem::User(label));
            model.busy = true;
            vec![Effect::SubmitPrompt { text: line, images }]
        }
    }
}

/// Open the `/models` picker for the active provider's catalog, preselecting the current model. An empty
/// catalog surfaces a notice instead — there is nothing to pick.
fn open_models_picker(model: &mut Model) -> Vec<Effect> {
    if model.models.is_empty() {
        model.transcript.push(TranscriptItem::Notice(
            NoticeLevel::Info,
            "este provider não tem catálogo de modelos; adicione em ~/.kiri/config.toml"
                .to_string(),
        ));
    } else {
        let current = model.status.model.clone();
        let selected = model.models.iter().position(|m| *m == current).unwrap_or(0);
        model.picker = Some(Picker::new(
            PickerKind::Models,
            "modelo",
            "Escolha o modelo ativo:",
            model.models.clone(),
            selected,
        ));
    }
    vec![]
}

/// Open the `/effort` picker over the reasoning-effort levels, preselecting the current effort.
fn open_effort_picker(model: &mut Model) -> Vec<Effect> {
    let options: Vec<String> = Effort::ALL.iter().map(|e| e.label().to_string()).collect();
    let selected = Effort::ALL
        .iter()
        .position(|e| *e == model.status.effort)
        .unwrap_or(0);
    model.picker = Some(Picker::new(
        PickerKind::Effort,
        "esforço",
        "Escolha o nível de esforço (reasoning):",
        options,
        selected,
    ));
    vec![]
}

/// Open the `/provider` picker over the configured providers (plus the "+ adicionar" row that opens the
/// add wizard), preselecting the active one. With no providers configured it surfaces a notice instead.
fn open_provider_picker(model: &mut Model) -> Vec<Effect> {
    if model.providers.is_empty() {
        model.transcript.push(TranscriptItem::Notice(
            NoticeLevel::Info,
            "nenhum provider configurado".to_string(),
        ));
    } else {
        let current = model.status.provider.clone();
        let selected = model
            .providers
            .iter()
            .position(|p| *p == current)
            .unwrap_or(0);
        // The configured providers, plus the "+ adicionar" row that opens the add wizard.
        let mut options = model.providers.clone();
        options.push(ADD_PROVIDER_LABEL.to_string());
        model.picker = Some(Picker::new(
            PickerKind::Provider,
            "provider",
            "Escolha o provider ativo (ou adicione um novo):",
            options,
            selected,
        ));
    }
    vec![]
}
