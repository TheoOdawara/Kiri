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
        Some(Command::Instructions) => {
            let text = model
                .instructions_display
                .clone()
                .unwrap_or_else(|| "Nenhuma instrução ativa.".to_string());
            model.notify_info(text);
            vec![]
        }
        Some(Command::Rules) => {
            let text = model
                .rules_display
                .clone()
                .unwrap_or_else(|| "Nenhuma regra carregada.".to_string());
            model.notify_info(text);
            vec![]
        }
        Some(Command::Commands) => {
            let text = model
                .commands_display
                .clone()
                .unwrap_or_else(|| "Nenhum comando custom carregado.".to_string());
            model.notify_info(text);
            vec![]
        }
        Some(Command::Agents) => {
            let text = model
                .agents_display
                .clone()
                .unwrap_or_else(|| "Nenhum agente carregado.".to_string());
            model.notify_info(text);
            vec![]
        }
        Some(Command::Skills) => {
            let text = model
                .skills_display
                .clone()
                .unwrap_or_else(|| "Nenhuma skill carregada.".to_string());
            model.notify_info(text);
            vec![]
        }
        Some(Command::Hooks) => {
            let text = model
                .hooks_display
                .clone()
                .unwrap_or_else(|| "Nenhum hook carregado.".to_string());
            model.notify_info(text);
            vec![]
        }
        Some(Command::ApproveHook(id)) => {
            if id.is_empty() {
                model.notify_error("uso: /approve-hook <id> (veja /hooks)");
                vec![]
            } else {
                vec![Effect::ApproveHook(id)]
            }
        }
        Some(Command::Mcp) => {
            let text = model
                .mcp_display
                .clone()
                .unwrap_or_else(|| "Nenhum servidor MCP carregado.".to_string());
            model.notify_info(text);
            vec![]
        }
        Some(Command::ApproveMcp(id)) => {
            if id.is_empty() {
                model.notify_error("uso: /approve-mcp <id> (veja /mcp)");
                vec![]
            } else {
                vec![Effect::ApproveMcp(id)]
            }
        }
        Some(Command::Help) => {
            model.notify_info(command::help_text());
            vec![]
        }
        Some(Command::SetMode(mode)) => {
            model.approval_mode = mode;
            vec![]
        }
        Some(Command::ChangeWorkspace(None)) => {
            model.notify_info(format!("workspace: {}", model.status.workspace));
            vec![]
        }
        Some(Command::ChangeWorkspace(Some(path))) => vec![Effect::ChangeWorkspace(path)],
        Some(Command::Models) => open_models_picker(model),
        Some(Command::Effort) => open_effort_picker(model),
        Some(Command::Provider) => open_provider_picker(model),
        Some(Command::Unknown(token)) => match model.custom_command_bodies.get(&token).cloned() {
            Some(body) => submit_custom_command(model, &line, body),
            None => {
                model.notify_error(format!("comando desconhecido: {} (use /help)", line.trim()));
                vec![]
            }
        },
        None if line.trim().is_empty() && model.attachments.is_empty() => vec![],
        None if model.unconfigured => {
            // No usable provider yet: never send to the null provider silently. Drop the staged images,
            // surface a clear notice, and re-open onboarding. `busy` is intentionally left false so no
            // turn is armed and the UI is not stranded.
            model.attachments.clear();
            model.notify_info("configure um provider primeiro — escolha um e informe a API key");
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

/// Expand and submit an extension-provided custom command (ADR 0021): the transcript shows what the user
/// typed, but the prompt sent to the model is the command's body with any trailing argument text appended.
fn submit_custom_command(model: &mut Model, line: &str, body: String) -> Vec<Effect> {
    let arg = line
        .split_once(char::is_whitespace)
        .map(|(_, rest)| rest.trim())
        .unwrap_or("");
    let text = if arg.is_empty() {
        body
    } else {
        format!("{body}\n\n{arg}")
    };
    model
        .transcript
        .push(TranscriptItem::User(line.to_string()));
    model.busy = true;
    vec![Effect::SubmitPrompt {
        text,
        images: vec![],
    }]
}

/// Open the `/models` picker for the active provider's catalog, preselecting the current model. An empty
/// catalog surfaces a notice instead — there is nothing to pick.
fn open_models_picker(model: &mut Model) -> Vec<Effect> {
    if model.models.is_empty() {
        model.notify_info(
            "este provider não tem catálogo de modelos; adicione em ~/.kiri/config.toml",
        );
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

/// One `/provider` list row: id, kind, model, and auth status (issue #10's list-view acceptance
/// criterion) — compact enough for the picker's fixed width, unlike `modals::provider_detail_line`'s
/// fuller `base_url`/`thinking` line (that one stays scoped to the single-selected action sub-menu).
fn provider_row_label(id: &str, profile: Option<&ProviderProfile>) -> String {
    match profile {
        Some(p) => format!(
            "{id} · [{}] {} · {}",
            format!("{:?}", p.kind).to_ascii_lowercase(),
            p.model,
            p.auth.as_wire(),
        ),
        None => id.to_string(),
    }
}

/// Open the `/provider` picker over the configured providers (plus the "+ adicionar" row that opens the
/// add wizard), preselecting the active one. With no providers configured it surfaces a notice instead.
fn open_provider_picker(model: &mut Model) -> Vec<Effect> {
    if model.providers.is_empty() {
        model.notify_info("nenhum provider configurado");
    } else {
        let current = model.status.provider.clone();
        let selected = model
            .providers
            .iter()
            .position(|p| *p == current)
            .unwrap_or(0);
        // One row per configured provider (id, kind, model, auth), plus the "+ adicionar" row that
        // opens the add wizard. `model.providers` (the raw ids) stays the source of truth for which
        // provider an option maps to — see `on_picker_key`'s `PickerKind::Provider` arm.
        let mut options: Vec<String> = model
            .providers
            .iter()
            .map(|id| {
                let profile = model.provider_profiles.iter().find(|p| p.id == *id);
                provider_row_label(id, profile)
            })
            .collect();
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
