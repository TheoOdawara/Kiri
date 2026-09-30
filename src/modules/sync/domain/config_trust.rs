use std::collections::BTreeMap;

use serde::Deserialize;

use crate::shared::kernel::provider::AuthMethod;
use crate::shared::kernel::sandbox::{NetworkStance, SandboxMode};

/// A typed view rather than `toml::Value` poking, so the gate reasons over the same schema the loader
/// uses and cannot be fooled by an unexpected layout.
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
    #[serde(default)]
    paths: TrustPaths,
    #[serde(default)]
    http: TrustHttp,
}

#[derive(Deserialize, Default)]
struct TrustProvider {
    #[serde(default)]
    base_url: Option<String>,
    /// Absent means the historical default, which is *not* `Some(AuthMethod::None)`.
    #[serde(default)]
    auth: Option<AuthMethod>,
}

#[derive(Deserialize, Default)]
struct TrustSandbox {
    /// An unknown value maps to the safe variant, so a forward-version config never reads as a downgrade.
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

/// `docs` redirects `consult_docs`'s search root: pointed at the user's home, it would surface excerpts
/// of any `.md` file there to the model (issue #30/S4-1).
#[derive(Deserialize, Default)]
struct TrustPaths {
    #[serde(default)]
    docs: Option<String>,
}

#[derive(Deserialize, Default, PartialEq)]
struct TrustHttp {
    #[serde(default)]
    connect_timeout_ms: Option<u64>,
    #[serde(default)]
    read_timeout_ms: Option<u64>,
}

/// Risky differences in an incoming config that, applied as the trusted global layer, could redirect a
/// credential or weaken the sandbox. An empty list is safe to apply.
///
/// The re-parse and the invalid-TOML arm are redundant on the `pull` path, which validates first; they
/// are kept so a direct caller that has not pre-validated is still gated.
pub(crate) fn risky_config_changes(current: &str, incoming: &str) -> Vec<String> {
    let incoming: TrustView = match toml::from_str(incoming) {
        Ok(value) => value,
        Err(error) => return vec![format!("incoming config is not valid TOML: {error}")],
    };
    // An unparseable baseline cannot prove the change non-risky, so it demands an explicit `--force`.
    let current: TrustView = match toml::from_str(current) {
        Ok(value) => value,
        Err(_) => {
            return vec![
                "current config is unreadable; cannot verify the change is safe".to_string(),
            ];
        }
    };

    let mut risks = Vec::new();

    // A new provider or a changed base_url redirects where a credential is sent — the core exfiltration
    // vector. Disabling auth silently strands the credential and can brick the next boot.
    for (id, inc) in &incoming.providers {
        match current.providers.get(id) {
            None => risks.push(format!("new provider '{id}' added")),
            Some(cur) => {
                if cur.base_url != inc.base_url {
                    risks.push(format!("provider '{id}' base_url changes"));
                }
                // Anything other than an explicit `None` counts as keyed, absent included.
                if cur.auth != Some(AuthMethod::None) && inc.auth == Some(AuthMethod::None) {
                    risks.push(format!("provider '{id}' auth disabled"));
                }
            }
        }
    }

    // Switching to a provider with a different base_url redirects the active credential.
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

    // A redirected embeddings provider receives the embedded text and that provider's key.
    if incoming.embeddings.provider != current.embeddings.provider {
        risks.push("embeddings provider changes".to_string());
    }

    if incoming.paths.docs != current.paths.docs {
        risks.push("paths.docs changes".to_string());
    }

    // Availability-only, but flagged on any change rather than only a widening — the conservative rule.
    if incoming.http != current.http {
        risks.push("http timeout settings change".to_string());
    }

    // Ranking (`Require > Os > Off`) catches every weakening, not only `→ off`. An absent mode is the
    // resolver's `Os` baseline, so `absent → os` is a no-op.
    let current_mode = current.sandbox.mode.unwrap_or(SandboxMode::Os);
    let incoming_mode = incoming.sandbox.mode.unwrap_or(SandboxMode::Os);
    if incoming_mode.rank() < current_mode.rank() {
        risks.push(format!(
            "sandbox confinement weakened ({current_mode:?} -> {incoming_mode:?})"
        ));
    }

    // An absent stance is the `Deny` baseline.
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
        // On a platform with no OS sandbox this is a genuine weakening; the old gate only flagged `→ off`.
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
        let risks = risky_config_changes("", "[sandbox]\nmode = \"os\"\n");
        assert!(
            !risks.iter().any(|r| r.contains("sandbox")),
            "absent baseline is os: {risks:?}"
        );
    }

    #[test]
    fn auth_gate_uses_typed_authmethod() {
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
    fn detects_docs_path_redirect() {
        let risks = risky_config_changes(
            "[paths]\ndocs = \"docs\"\n",
            "[paths]\ndocs = \"/home/victim\"\n",
        );
        assert!(risks.iter().any(|r| r.contains("paths.docs")), "{risks:?}");
    }

    #[test]
    fn identical_docs_path_is_safe() {
        let config = "[paths]\ndocs = \"docs\"\n";
        assert!(
            !risky_config_changes(config, config)
                .iter()
                .any(|r| r.contains("paths.docs"))
        );
    }

    #[test]
    fn absent_docs_path_on_both_sides_is_safe() {
        assert!(risky_config_changes("", "").is_empty());
    }

    #[test]
    fn detects_http_timeout_change() {
        let risks = risky_config_changes(
            "[http]\nconnect_timeout_ms = 5000\n",
            "[http]\nconnect_timeout_ms = 600000\n",
        );
        assert!(risks.iter().any(|r| r.contains("http")), "{risks:?}");
    }

    #[test]
    fn identical_http_settings_are_safe() {
        let config = "[http]\nconnect_timeout_ms = 5000\nread_timeout_ms = 30000\n";
        assert!(
            !risky_config_changes(config, config)
                .iter()
                .any(|r| r.contains("http"))
        );
    }

    #[test]
    fn unreadable_current_config_is_treated_as_risky() {
        let risks = risky_config_changes("this is = = not toml", "[sandbox]\nmode = \"os\"\n");
        assert!(!risks.is_empty());
    }
}
