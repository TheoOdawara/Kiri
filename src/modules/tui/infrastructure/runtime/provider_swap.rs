//! Live provider swapping: the `ProviderSwap` adapter cache (rebuild on `/models`/`/effort`/`/provider`/
//! a wizard save) plus the four `RunLoop` effect handlers that apply and persist each change.

use std::sync::Arc;

use crate::modules::provider::application::completion_provider::CompletionProvider;
use crate::modules::provider::application::secret_store::SecretStore;
use crate::modules::provider::infrastructure::factory::{
    CredentialResolution, build_provider, resolve_credential as resolve_credential_policy,
};
use crate::modules::tui::domain::model::Model;
use crate::shared::infra::config;
use crate::shared::kernel::error::AgentError;
use crate::shared::kernel::provider::{
    AuthMethod, Credential, Effort, ProviderKind, ProviderProfile,
};

use super::RunLoop;

/// Everything the runtime needs to rebuild the provider adapter for a live `/models`/`/effort`/
/// `/provider` change: the HTTP client, the secret store, the full provider catalog, the active id, the
/// active provider's cached credential (so a rebuild needs no store round-trip), and the thinking/
/// effort dials. Effort is captured at adapter construction, so changing effort — or the active
/// provider — means rebuilding the `Arc`. The cached `credential` has three states: `None` during
/// first-run onboarding (no usable key yet); `Some(Credential::None)` for an active keyless provider
/// (auth = "none"); and `Some(Credential::ApiKey { .. })` for a keyed one. It is set the moment a
/// provider is switched to or saved, so a rebuild needs no store round-trip.
pub struct ProviderSwap {
    client: reqwest::Client,
    secrets: Box<dyn SecretStore>,
    providers: Vec<ProviderProfile>,
    pub(super) active: String,
    credential: Option<Credential>,
    /// When true, the cached `credential` came from `KIRI_NO_KEY_IMPORT` (SEC-07 session-only) and must
    /// not be written to the secret store on a blank-key edit (#60).
    credential_session_only: bool,
    thinking: bool,
    pub(super) effort: Effort,
}

/// The outcome of a live `/provider` switch: the rebuilt adapter, its model, and an optional persist
/// warning — `Some` when a one-time env-import key failed to save — so `apply_set_provider` can surface
/// it as a transcript Notice instead of swallowing it (ERR-02).
pub(super) struct ProviderSwitch {
    pub(super) provider: Arc<dyn CompletionProvider>,
    pub(super) model: String,
    pub(super) persist_warning: Option<AgentError>,
}

impl ProviderSwap {
    pub fn new(
        client: reqwest::Client,
        secrets: Box<dyn SecretStore>,
        providers: Vec<ProviderProfile>,
        active: String,
        credential: Option<Credential>,
        thinking: bool,
        effort: Effort,
    ) -> Self {
        Self {
            client,
            secrets,
            providers,
            active,
            credential,
            // Boot path sets this correctly via `with_session_only_credential` when needed; default
            // false is safe (persist is the common path).
            credential_session_only: false,
            thinking,
            effort,
        }
    }

    /// Mark the cached credential as SEC-07 session-only (must not be written on blank-key edit).
    pub fn with_session_only_credential(mut self, session_only: bool) -> Self {
        self.credential_session_only = session_only;
        self
    }

    pub(super) fn active_profile(&self) -> Option<&ProviderProfile> {
        self.providers.iter().find(|p| p.id == self.active)
    }

    fn active_profile_mut(&mut self) -> Option<&mut ProviderProfile> {
        let active = self.active.clone();
        self.providers.iter_mut().find(|p| p.id == active)
    }

    /// The configured provider ids, in catalog order — the `/provider` picker's options.
    pub fn provider_ids(&self) -> Vec<String> {
        self.providers.iter().map(|p| p.id.clone()).collect()
    }

    /// The full provider catalog — the action sub-menu reads this to display details.
    pub(super) fn profiles(&self) -> &[ProviderProfile] {
        &self.providers
    }

    /// Remove a provider from the in-memory catalog and delete its secret. Best-effort on the secret
    /// (a missing key is a harmless no-op). If the deleted provider was active, falls back to the first
    /// remaining one (or clears `active` when the catalog becomes empty).
    pub(super) fn remove_provider(&mut self, id: &str) {
        let _ = self.secrets.delete(id);
        self.providers.retain(|p| p.id != id);
        if self.active == id {
            self.active = self
                .providers
                .first()
                .map(|p| p.id.clone())
                .unwrap_or_default();
            self.credential = None;
            self.credential_session_only = false;
        }
    }

    /// Build an adapter for a specific profile/credential/effort without committing any state, so a
    /// failed rebuild leaves the current provider untouched.
    fn build(
        &self,
        profile: &ProviderProfile,
        credential: &Credential,
        effort: Effort,
    ) -> Result<Arc<dyn CompletionProvider>, AgentError> {
        build_provider(
            self.client.clone(),
            profile,
            credential.clone(),
            self.thinking,
            effort,
        )
    }

    /// Resolve a provider's credential via the single shared resolver (the same keyless → stored →
    /// env-import policy as boot), returning the credential, whether it is SEC-07 session-only (must not
    /// be written to the store), and any env-import persist warning — `Some` when a one-time env key
    /// failed to save — so the caller can surface it instead of swallowing it.
    /// A keyless provider yields `Credential::None`; a provider with nothing configured is a clear error.
    pub(super) fn resolve_credential(
        &self,
        profile: &ProviderProfile,
    ) -> Result<(Credential, bool, Option<AgentError>), AgentError> {
        match resolve_credential_policy(profile, self.secrets.as_ref())? {
            CredentialResolution::Keyless => Ok((Credential::None, false, None)),
            CredentialResolution::Stored(credential) => Ok((credential, false, None)),
            CredentialResolution::Imported {
                credential,
                persisted,
            } => Ok((credential, false, persisted.err())),
            // SEC-07 opt-out: env key used for this session only, never persisted — no persist warning.
            CredentialResolution::ImportedSessionOnly { credential } => {
                Ok((credential, true, None))
            }
            CredentialResolution::Absent => Err(AgentError::Provider(format!(
                "no credential for provider '{}'. Configure it via /provider or set its API-key env var.",
                profile.id
            ))),
        }
    }

    /// Resolve the credential to reuse when editing `profile` with a blank key ("keep existing key").
    /// Returns `(credential, persistable)` — `persistable` is false for SEC-07 session-only keys (#60).
    /// Reuses the cached credential ONLY when `profile.id` is the currently ACTIVE provider — `credential`
    /// is cached per-active-provider, not per-`profile.id` (see the struct doc comment). Issue #27a: using
    /// it unconditionally let editing a DIFFERENT, non-active provider with a blank key silently adopt the
    /// active provider's credential and re-store it under the edited profile's id, corrupting that
    /// provider's stored secret with the wrong key. For any other provider (or no cache yet), resolves
    /// fresh from the secret store, keyed by `profile.id` itself.
    pub(super) fn resolve_credential_for_edit(
        &self,
        profile: &ProviderProfile,
    ) -> Result<(Credential, bool), AgentError> {
        if self.active == profile.id
            && let Some(credential) = self.credential.clone()
        {
            return Ok((credential, !self.credential_session_only));
        }
        let (credential, session_only, _) = self.resolve_credential(profile)?;
        Ok((credential, !session_only))
    }

    /// Rebuild the active provider with a new `effort`, committing the effort only on success. Without a
    /// live credential (first-run onboarding) there is nothing to rebuild, so it surfaces a clear error
    /// and leaves the effort dial untouched rather than panicking or silently diverging.
    pub(super) fn rebuild_with_effort(
        &mut self,
        effort: Effort,
    ) -> Result<Arc<dyn CompletionProvider>, AgentError> {
        let Some(credential) = self.credential.clone() else {
            return Err(AgentError::Provider(
                "configure um provider com /provider antes de mudar o esforço".to_string(),
            ));
        };
        let profile = self
            .active_profile()
            .ok_or_else(|| AgentError::Provider("no active provider configured".to_string()))?;
        let provider = self.build(profile, &credential, effort)?;
        self.effort = effort;
        Ok(provider)
    }

    /// Switch the active provider to `id`: look up its profile + credential, build the adapter, and
    /// commit (active id + cached credential) only on success. Returns the new adapter, the target model
    /// id, and any env-import persist warning so the caller can surface it. An unknown id or a missing
    /// credential is a clear error.
    pub(super) fn switch_to(&mut self, id: &str) -> Result<ProviderSwitch, AgentError> {
        let profile = self
            .providers
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| AgentError::Provider(format!("provider '{id}' is not configured")))?
            .clone();
        let (credential, session_only, persist_warning) = self.resolve_credential(&profile)?;
        let provider = self.build(&profile, &credential, self.effort)?;
        self.active = id.to_string();
        self.credential = Some(credential);
        self.credential_session_only = session_only;
        Ok(ProviderSwitch {
            provider,
            model: profile.model,
            persist_warning,
        })
    }

    /// Store a new provider's credential, build its adapter, add-or-replace it in the catalog, and make
    /// it active — all committed only if the credential stores and the adapter builds. Returns the new
    /// adapter and its model.
    ///
    /// `persist_secret`: when false (SEC-07 session-only keep-existing-key edit, #60), the credential
    /// stays in memory only and is never written to the secret store.
    pub(super) fn add_and_activate(
        &mut self,
        profile: ProviderProfile,
        credential: Credential,
        persist_secret: bool,
    ) -> Result<(Arc<dyn CompletionProvider>, String), AgentError> {
        // Build first (validates the profile/credential), then store the secret — so a build failure
        // never leaves an orphaned credential in the store for a provider that was not added.
        let provider = self.build(&profile, &credential, self.effort)?;
        match &credential {
            // A keyless provider stores nothing; clear any stale key from a prior keyed config of this
            // id best-effort (a missing-key delete is a harmless no-op) so no orphaned secret lingers.
            Credential::None => {
                let _ = self.secrets.delete(&profile.id);
                self.credential_session_only = false;
            }
            _ if persist_secret => {
                self.secrets.set(&profile.id, &credential)?;
                self.credential_session_only = false;
            }
            _ => {
                // Session-only: leave the store untouched (#60).
                self.credential_session_only = true;
            }
        }
        let id = profile.id.clone();
        let model = profile.model.clone();
        self.providers.retain(|p| p.id != id);
        self.providers.push(profile);
        self.active = id;
        self.credential = Some(credential);
        Ok((provider, model))
    }
}

/// Surface a persist failure as an error notice, or stay silent on success — the single "persist failed →
/// report it" path shared by the four `apply_*` handlers, so a write failure is never swallowed. Generic
/// over `E: Display` so it survives the config writers moving from `anyhow::Result` to `AgentError`.
fn persist_or_notice<E: std::fmt::Display>(
    result: Result<(), E>,
    model: &mut Model,
    context: &str,
) {
    if let Err(error) = result {
        model.notify_error(format!("{context}: {error:#}"));
    }
}

/// Surface a one-time env-import persist failure as an error notice, or stay silent when the key
/// persisted (or there was nothing to import) — the testable render for the `ProviderSwitch` warning
/// (ERR-02), so the once-swallowed `let _ =` stays closed and locked.
fn notice_env_import_failure(warning: Option<AgentError>, model: &mut Model) {
    if let Some(error) = warning {
        model.notify_error(format!(
            "a chave importada do ambiente não foi salva: {error:#}"
        ));
    }
}

impl RunLoop {
    /// Apply a `/models` selection: a model change is just the per-turn `model` field — no provider
    /// rebuild. Apply it live, reflect it in the status line, and persist (best-effort) to the global
    /// config; a write failure is surfaced but the live change stands.
    pub(super) fn apply_set_model(&mut self, model_id: String) {
        if let Some(profile) = self.provider_swap.active_profile_mut() {
            profile.model = model_id.clone();
        }
        self.agent_loop.set_model(model_id.clone());
        self.model.status.model = model_id.clone();
        self.model.notify_info(format!("modelo: {model_id}"));
        let persisted =
            config::persist_active_model(&self.config_path, &self.provider_swap.active, &model_id);
        persist_or_notice(persisted, &mut self.model, "não persistiu o modelo");
    }

    /// Apply an `/effort` selection. Effort is baked into the provider at construction, so rebuild and
    /// swap it in. Build with the new effort first; commit (status + cached effort + persist) only if the
    /// rebuild succeeds, so a failure leaves the current provider untouched.
    pub(super) fn apply_set_effort(&mut self, effort: Effort) {
        let is_anthropic =
            self.provider_swap.active_profile().map(|p| p.kind) == Some(ProviderKind::Anthropic);
        match self.provider_swap.rebuild_with_effort(effort) {
            Ok(provider) => {
                self.agent_loop.set_provider(provider);
                self.model.status.effort = effort;
                // The Anthropic adapter ignores effort today — surface that rather than
                // silently appearing to change nothing.
                let note = if is_anthropic {
                    format!(
                        "esforço: {} — nota: ainda não afeta modelos Claude",
                        effort.label()
                    )
                } else {
                    format!("esforço: {}", effort.label())
                };
                self.model.notify_info(note);
                persist_or_notice(
                    config::persist_effort(&self.config_path, effort),
                    &mut self.model,
                    "não persistiu o esforço",
                );
            }
            Err(error) => self
                .model
                .notify_error(format!("não foi possível aplicar o esforço: {error:#}")),
        }
    }

    /// Install a successful `ProviderSwitch`: swap the live adapter, adopt its model, refresh the status
    /// line AND the `/models` picker's catalog (`self.model.models`), and surface any one-time env-import
    /// persist warning. The one place both `apply_set_provider` and `apply_delete_provider`'s
    /// fallback-to-next-provider path install a switch, so the model-catalog refresh can never silently
    /// drift between the two again — it did (issue #27/G2-2): the delete path's own copy of this block had
    /// dropped the `self.model.models` line, leaving the `/models` picker offering the deleted provider's
    /// models until the next `/provider` switch.
    fn install_switch(&mut self, id: &str, switch: ProviderSwitch) {
        let ProviderSwitch {
            provider,
            model: target_model,
            persist_warning,
        } = switch;
        self.agent_loop.set_provider(provider);
        self.agent_loop.set_model(target_model.clone());
        self.model.status.model = target_model;
        self.model.status.provider = id.to_string();
        self.model.models = self
            .provider_swap
            .active_profile()
            .map(|p| p.models.clone())
            .unwrap_or_default();
        // Surface a one-time env-import persist failure (closes the former swallowed `let _ =`):
        // the switch still succeeded for this session, but the key was not saved for the next one.
        notice_env_import_failure(persist_warning, &mut self.model);
    }

    /// Apply a `/provider` switch: rebuild the chosen provider's adapter with its stored credential and
    /// swap it in, also adopting its model. Commit + persist only on success; a missing credential or
    /// unknown id is surfaced, never a silent no-op.
    pub(super) fn apply_set_provider(&mut self, id: String) {
        match self.provider_swap.switch_to(&id) {
            Ok(switch) => {
                let target_model = switch.model.clone();
                self.install_switch(&id, switch);
                self.model
                    .notify_info(format!("provider: {id} ({target_model})"));
                persist_or_notice(
                    config::persist_active_provider(&self.config_path, &id),
                    &mut self.model,
                    "não persistiu o provider ativo",
                );
            }
            Err(error) => self
                .model
                .notify_error(format!("não foi possível trocar de provider: {error:#}")),
        }
    }

    /// Apply a wizard `SaveProvider` or edit: derive the credential, build and store the provider, make
    /// it active, drop the onboarding gate, and persist. `keep_existing_key` skips the store write
    /// (edit mode, user left key field blank — the stored secret is reused). Commit only on success.
    pub(super) fn apply_save_provider(
        &mut self,
        profile: ProviderProfile,
        keep_existing_key: bool,
    ) {
        let (credential, persist_secret) = match &profile.auth {
            AuthMethod::ApiKey if keep_existing_key => {
                // Editing with a blank key: see `ProviderSwap::resolve_credential_for_edit` (issue #27a)
                // for why this can't just reuse the cached credential unconditionally.
                self.model.pending_credential = None;
                match self.provider_swap.resolve_credential_for_edit(&profile) {
                    Ok((c, persistable)) => (c, persistable),
                    Err(error) => {
                        self.model.notify_error(format!(
                            "não foi possível recuperar a chave existente: {error:#}"
                        ));
                        return;
                    }
                }
            }
            AuthMethod::ApiKey => {
                let Some(key) = self.model.pending_credential.take() else {
                    self.model
                        .notify_error("chave ausente; provider não foi salvo");
                    return;
                };
                (Credential::ApiKey { key }, true)
            }
            AuthMethod::None => {
                self.model.pending_credential = None;
                (Credential::None, true)
            }
            other => {
                self.model.pending_credential = None;
                self.model.notify_error(format!(
                    "método de auth não suportado pelo wizard ({other:?}); provider não foi salvo"
                ));
                return;
            }
        };
        let id = profile.id.clone();
        match self
            .provider_swap
            .add_and_activate(profile.clone(), credential, persist_secret)
        {
            Ok((provider, target_model)) => {
                self.agent_loop.set_provider(provider);
                self.agent_loop.set_model(target_model.clone());
                self.model.unconfigured = false;
                self.model.status.model = target_model;
                self.model.status.provider = id.clone();
                self.model.models = profile.models.clone();
                self.model.providers = self.provider_swap.provider_ids();
                self.model.provider_profiles = self.provider_swap.profiles().to_vec();
                self.model
                    .notify_info(format!("provider '{id}' salvo e ativo"));
                let persisted = config::upsert_provider(&self.config_path, &profile)
                    .and_then(|()| config::persist_active_provider(&self.config_path, &id));
                persist_or_notice(
                    persisted,
                    &mut self.model,
                    "provider ativo, mas não persistiu no config",
                );
            }
            Err(error) => self
                .model
                .notify_error(format!("não foi possível salvar o provider: {error:#}")),
        }
    }

    /// Apply a `DeleteProvider` effect: remove from in-memory catalog + store, persist to TOML,
    /// rebuild the adapter for the new active provider (or revert to the null/unconfigured state), and
    /// update model state.
    pub(super) fn apply_delete_provider(&mut self, id: String) {
        let was_active = self.provider_swap.active == id;
        self.provider_swap.remove_provider(&id);
        persist_or_notice(
            config::delete_provider(&self.config_path, &id),
            &mut self.model,
            "provider removido, mas não persistiu no config",
        );
        self.model.providers = self.provider_swap.provider_ids();
        self.model.provider_profiles = self.provider_swap.profiles().to_vec();
        self.model.notify_info(format!("provider '{id}' removido"));

        if !was_active {
            return;
        }
        let new_active = self.provider_swap.active.clone();
        if new_active.is_empty() {
            // No providers left: revert to the unconfigured/onboarding state.
            self.model.status.provider = String::new();
            self.model.status.model = String::new();
            self.model.enter_onboarding();
            return;
        }
        // Rebuild the adapter for the new active provider.
        match self.provider_swap.switch_to(&new_active) {
            Ok(switch) => self.install_switch(&new_active, switch),
            Err(error) => self.model.notify_error(format!(
                "não foi possível ativar o próximo provider: {error:#}"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::tui::domain::transcript::{NoticeLevel, TranscriptItem};

    #[test]
    fn persist_or_notice_is_silent_on_ok() {
        let mut model = Model::default();
        persist_or_notice(Ok::<(), String>(()), &mut model, "contexto");
        assert!(
            model.transcript.is_empty(),
            "a successful persist must add no notice"
        );
    }

    #[test]
    fn persist_or_notice_pushes_an_error_notice_on_err() {
        let mut model = Model::default();
        persist_or_notice(Err("disco cheio"), &mut model, "não persistiu o modelo");
        assert_eq!(
            model.transcript.items().last(),
            Some(&TranscriptItem::Notice(
                NoticeLevel::Error,
                "não persistiu o modelo: disco cheio".to_string()
            )),
            "a persist failure must surface a contextual error notice"
        );
    }

    #[test]
    fn notice_env_import_failure_some_pushes_error_notice() {
        let mut model = Model::default();
        // The real warning is a SecretStore persist failure (`persisted.err()`), so use that variant.
        notice_env_import_failure(Some(AgentError::Secret("falha".to_string())), &mut model);
        assert_eq!(
            model.transcript.items().last(),
            Some(&TranscriptItem::Notice(
                NoticeLevel::Error,
                "a chave importada do ambiente não foi salva: credential store error: falha"
                    .to_string()
            )),
            "an env-import persist failure must surface the exact one-time warning"
        );
    }

    #[test]
    fn notice_env_import_failure_none_is_silent() {
        let mut model = Model::default();
        notice_env_import_failure(None, &mut model);
        assert!(
            model.transcript.is_empty(),
            "a persisted (or absent) env key must add no notice"
        );
    }
}
