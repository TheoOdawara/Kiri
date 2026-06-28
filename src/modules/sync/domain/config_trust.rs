use std::collections::BTreeMap;

use serde::Deserialize;

use crate::shared::kernel::provider::AuthMethod;
use crate::shared::kernel::sandbox::{NetworkStance, SandboxMode};

/// The security-relevant fields of a config, parsed against the real shape (extra fields ignored).
/// Deliberately a small typed view rather than `toml::Value` poking, so the trust gate reasons over the
/// same schema the loader uses and cannot be fooled by an unexpected layout.
#[derive(Deserialize, Default)]
struct TrustView {
    #[serde(default)]
    active_provider: Option<String>,
    #[serde(default)]
    providers: BTreeMap<String, TrustProvider>,
    #[serde(default)]
    sandbox: TrustSandbox,
    #[serde(default)]
    embeddings: TrustEmbeddings,
}

#[derive(Deserialize, Default)]
struct TrustProvider {
    #[serde(default)]
    base_url: Option<String>,
    /// Typed against the kernel [`AuthMethod`] (forward-compat `Deserialize`) so the gate reasons over
    /// the same enum the loader uses, not a hand-typed `"none"` literal. Absent = the historical default.
    #[serde(default)]
    auth: Option<AuthMethod>,
}

#[derive(Deserialize, Default)]
struct TrustSandbox {
    /// Typed against the kernel sandbox primitives; an unknown value maps to the safe variant, so a
    /// forward-version config is never read as a downgrade. Absent = the resolver's baseline.
    #[serde(default)]
    mode: Option<SandboxMode>,
    #[serde(default)]
    network: Option<NetworkStance>,
}

#[derive(Deserialize, Default)]
struct TrustEmbeddings {
    #[serde(default)]
    provider: Option<String>,
}

/// Identify risky differences in an incoming config that, applied as the trusted global layer, could
/// redirect a credential or weaken the sandbox. Flags: a newly added provider; an existing provider's
/// `base_url` added or changed; an existing provider's auth disabled; the active provider switching to a
/// different endpoint; the embeddings provider changing; the sandbox confinement *weakened* by rank
/// (`require → os`, `require → off`, `os → off`); the sandbox network widened to allow. Reasons over the
/// typed kernel [`AuthMethod`]/[`SandboxMode`]/[`NetworkStance`] (no magic strings). Returns a
/// human-readable list (empty = safe to apply).
///
/// Standalone entry point: it re-parses both configs and owns its own incoming-TOML-validity guard (the
/// `not valid TOML` arm below), so it stays correct when called directly — from its unit tests, or any
/// future caller that has not pre-validated. On the `pull` path the incoming config is validated first,
/// which makes that arm redundant *there*; the self-contained parse is deliberate defense-in-depth, not
/// dead code. (Reusing the already-parsed config would mean returning the parsed `RawConfig` from
/// `validate_config_str` and threading it here — a wider config-API change deferred as not worth it.)
pub(crate) fn risky_config_changes(current: &str, incoming: &str) -> Vec<String> {
    let incoming: TrustView = match toml::from_str(incoming) {
        Ok(value) => value,
        Err(error) => return vec![format!("incoming config is not valid TOML: {error}")],
    };
    // A current config we cannot parse is not a baseline we can compare against — we cannot prove the
    // change is non-risky, so treat it as requiring an explicit `--force`.
    let current: TrustView = match toml::from_str(current) {
        Ok(value) => value,
        Err(_) => {
            return vec![
                "current config is unreadable; cannot verify the change is safe".to_string(),
            ];
        }
    };

    let mut risks = Vec::new();

    // A new provider, or a base_url added/changed on an existing one, can redirect where a credential
    // (stored or env-imported) is sent — the core credential-exfiltration vector. Turning off an existing
    // provider's authentication (api-key/oauth -> none) silently disables its credential, and for a vendor
    // endpoint the next boot then fails to build it (a DoS-via-sync); both require an explicit `--force`.
    for (id, inc) in &incoming.providers {
        match current.providers.get(id) {
            None => risks.push(format!("new provider '{id}' added")),
            Some(cur) => {
                if cur.base_url != inc.base_url {
                    risks.push(format!("provider '{id}' base_url changes"));
                }
                // Anything not explicitly `None` (api-key/oauth, or absent = the historical default,
                // since `None != Some(AuthMethod::None)`) is treated as keyed, so dropping authentication
                // is always flagged.
                if cur.auth != Some(AuthMethod::None) && inc.auth == Some(AuthMethod::None) {
                    risks.push(format!("provider '{id}' auth disabled"));
                }
            }
        }
    }

    // The active provider switching to one with a different base_url redirects the active credential.
    let active_url = |view: &TrustView| -> Option<String> {
        view.active_provider
            .as_ref()
            .and_then(|id| view.providers.get(id))
            .and_then(|p| p.base_url.clone())
    };
    if incoming.active_provider != current.active_provider
        && active_url(&incoming) != active_url(&current)
    {
        risks.push("active_provider changes to a different endpoint".to_string());
    }

    // Redirecting the embeddings provider sends the embedded text (and that provider's key) elsewhere.
    if incoming.embeddings.provider != current.embeddings.provider {
        risks.push("embeddings provider changes".to_string());
    }

    // Sandbox confinement must not weaken. Rank the modes (`Require > Os > Off`) so any strictly-lower
    // incoming rank is flagged — not only `→ off`. An absent mode is the resolver's `Os` baseline, so
    // `absent → os` (and `os → require`) does not flag. Debug-format the modes so the message carries no
    // bare `"off"`/`"require"` literal the gate could be mistaken for comparing against.
    let current_mode = current.sandbox.mode.unwrap_or(SandboxMode::Os);
    let incoming_mode = incoming.sandbox.mode.unwrap_or(SandboxMode::Os);
    if incoming_mode.rank() < current_mode.rank() {
        risks.push(format!(
            "sandbox confinement weakened ({current_mode:?} -> {incoming_mode:?})"
        ));
    }

    // Base network stance must not widen from deny to allow (an absent stance is the `Deny` baseline).
    let current_net = current.sandbox.network.unwrap_or(NetworkStance::Deny);
    let incoming_net = incoming.sandbox.network.unwrap_or(NetworkStance::Deny);
    if incoming_net == NetworkStance::Allow && current_net != NetworkStance::Allow {
        risks.push(format!(
            "sandbox network widened ({current_net:?} -> {incoming_net:?})"
        ));
    }

    risks
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A complete, schema-valid provider profile (the loader requires kind/base_url/model/auth;
    /// `ProviderKind::OpenAiCompatible` serializes kebab-case as `open-ai-compatible`).
    fn provider_toml(id: &str, base_url: &str) -> String {
        format!(
            "[providers.{id}]\nkind = \"open-ai-compatible\"\nbase_url = \"{base_url}\"\nmodel = \"m\"\nauth = \"api-key\"\n"
        )
    }

    #[test]
    fn detects_base_url_change() {
        let current = r#"
[providers.nvidia]
base_url = "https://integrate.api.nvidia.com/v1"
"#;
        let incoming = r#"
[providers.nvidia]
base_url = "https://evil.example/v1"
"#;
        let risks = risky_config_changes(current, incoming);
        assert_eq!(risks.len(), 1);
        assert!(risks[0].contains("nvidia"));
    }

    #[test]
    fn detects_sandbox_off() {
        let risks = risky_config_changes("", "[sandbox]\nmode = \"off\"\n");
        assert!(risks.iter().any(|r| r.contains("sandbox")));
    }

    #[test]
    fn detects_sandbox_require_to_os_downgrade() {
        // The audited hole: `require → os` is a genuine weakening on a platform with no OS sandbox, yet
        // the old gate only flagged `→ off`. Ranking catches it.
        let risks = risky_config_changes(
            "[sandbox]\nmode = \"require\"\n",
            "[sandbox]\nmode = \"os\"\n",
        );
        assert!(risks.iter().any(|r| r.contains("sandbox")), "{risks:?}");
    }

    #[test]
    fn detects_sandbox_require_to_off_downgrade() {
        let risks = risky_config_changes(
            "[sandbox]\nmode = \"require\"\n",
            "[sandbox]\nmode = \"off\"\n",
        );
        assert!(risks.iter().any(|r| r.contains("sandbox")), "{risks:?}");
    }

    #[test]
    fn detects_sandbox_os_to_off_downgrade() {
        let risks =
            risky_config_changes("[sandbox]\nmode = \"os\"\n", "[sandbox]\nmode = \"off\"\n");
        assert!(risks.iter().any(|r| r.contains("sandbox")), "{risks:?}");
    }

    #[test]
    fn sandbox_os_to_require_is_safe() {
        // Strengthening confinement (lower → higher rank) is never risky.
        let risks = risky_config_changes(
            "[sandbox]\nmode = \"os\"\n",
            "[sandbox]\nmode = \"require\"\n",
        );
        assert!(
            !risks.iter().any(|r| r.contains("sandbox")),
            "strengthening must not flag: {risks:?}"
        );
    }

    #[test]
    fn sandbox_absent_to_os_is_safe() {
        // An absent mode is the `Os` baseline, so `absent → os` is a no-op, not a downgrade.
        let risks = risky_config_changes("", "[sandbox]\nmode = \"os\"\n");
        assert!(
            !risks.iter().any(|r| r.contains("sandbox")),
            "absent baseline is os: {risks:?}"
        );
    }

    #[test]
    fn auth_gate_uses_typed_authmethod() {
        // The auth-disabled check reasons over the typed `Some(AuthMethod::None)`, not a `"none"` string:
        // an `api-key → none` change on the same endpoint is flagged.
        let current = provider_toml("nvidia", "https://x/v1"); // auth = "api-key"
        let incoming = "[providers.nvidia]\nkind = \"open-ai-compatible\"\n\
                        base_url = \"https://x/v1\"\nmodel = \"m\"\nauth = \"none\"\n";
        let risks = risky_config_changes(&current, incoming);
        assert!(
            risks.iter().any(|r| r.contains("auth disabled")),
            "{risks:?}"
        );
    }

    #[test]
    fn identical_config_is_safe() {
        let config = "[providers.nvidia]\nbase_url = \"https://x/v1\"\n";
        assert!(risky_config_changes(config, config).is_empty());
    }

    #[test]
    fn detects_a_new_provider() {
        let current = "[providers.nvidia]\nbase_url = \"https://x/v1\"\n";
        let incoming = "[providers.nvidia]\nbase_url = \"https://x/v1\"\n\
                        [providers.evil]\nbase_url = \"https://attacker/v1\"\n";
        let risks = risky_config_changes(current, incoming);
        assert!(risks.iter().any(|r| r.contains("evil")), "{risks:?}");
    }

    #[test]
    fn detects_auth_downgrade_to_none() {
        // Turning off authentication on an existing provider (same base_url) must require --force, so a
        // synced config cannot silently disable a credential — and, for a vendor endpoint, brick the boot.
        let current = provider_toml("nvidia", "https://x/v1"); // auth = "api-key"
        let incoming = "[providers.nvidia]\nkind = \"open-ai-compatible\"\n\
                        base_url = \"https://x/v1\"\nmodel = \"m\"\nauth = \"none\"\n";
        let risks = risky_config_changes(&current, incoming);
        assert!(
            risks.iter().any(|r| r.contains("auth disabled")),
            "{risks:?}"
        );
    }

    #[test]
    fn already_keyless_provider_unchanged_is_safe() {
        // A provider that was already keyless (none -> none) is not flagged as a downgrade.
        let config = "[providers.lmstudio]\nkind = \"open-ai-compatible\"\n\
                      base_url = \"http://localhost:1234/v1\"\nmodel = \"m\"\nauth = \"none\"\n";
        assert!(risky_config_changes(config, config).is_empty());
    }

    #[test]
    fn detects_active_provider_redirect() {
        let current = "active_provider = \"a\"\n[providers.a]\nbase_url = \"https://a/v1\"\n\
                       [providers.b]\nbase_url = \"https://b/v1\"\n";
        let incoming = "active_provider = \"b\"\n[providers.a]\nbase_url = \"https://a/v1\"\n\
                        [providers.b]\nbase_url = \"https://b/v1\"\n";
        let risks = risky_config_changes(current, incoming);
        assert!(
            risks.iter().any(|r| r.contains("active_provider")),
            "{risks:?}"
        );
    }

    #[test]
    fn detects_sandbox_network_widened() {
        let risks = risky_config_changes(
            "[sandbox]\nnetwork = \"deny\"\n",
            "[sandbox]\nnetwork = \"allow\"\n",
        );
        assert!(risks.iter().any(|r| r.contains("network")), "{risks:?}");
    }

    #[test]
    fn detects_embeddings_provider_change() {
        let risks = risky_config_changes(
            "[embeddings]\nprovider = \"nvidia\"\n",
            "[embeddings]\nprovider = \"evil\"\n",
        );
        assert!(risks.iter().any(|r| r.contains("embeddings")), "{risks:?}");
    }

    #[test]
    fn unreadable_current_config_is_treated_as_risky() {
        let risks = risky_config_changes("this is = = not toml", "[sandbox]\nmode = \"os\"\n");
        assert!(!risks.is_empty());
    }
}
