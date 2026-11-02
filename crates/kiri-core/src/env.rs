use std::path::PathBuf;

use reqwest::Url;
use serde_json::{Map, Value};
use thiserror::Error;

use crate::provider::catalog;

const BASE_URL: &str = "KIRI_BASE_URL";
const API_KEY: &str = "KIRI_API_KEY";
const MODEL: &str = "KIRI_MODEL";
const REASONING_EFFORT: &str = "KIRI_REASONING_EFFORT";
const THINKING: &str = "KIRI_THINKING";
const REQUEST_PARAMS: &str = "KIRI_REQUEST_PARAMS";
const BASE_URL_LIMIT: usize = 2048;
const API_KEY_LIMIT: usize = 4096;
const MODEL_LIMIT: usize = 256;
const REQUEST_PARAMS_LIMIT: usize = 65536;
const RESERVED_BODY_KEYS: [&str; 3] = ["model", "messages", "stream"];

pub(crate) fn home_directory() -> Option<PathBuf> {
    std::env::home_dir()
}

pub(crate) struct Config {
    pub(crate) base_url: String,
    pub(crate) api_key: String,
    pub(crate) model: String,
    pub(crate) request_params: Map<String, Value>,
    pub(crate) replays_reasoning: bool,
}

#[derive(Debug, Error, PartialEq)]
pub(crate) enum ConfigError {
    #[error("Set KIRI_BASE_URL, KIRI_API_KEY, and KIRI_MODEL before opening Kiri. Missing: {0}.")]
    Missing(String),
    #[error("{name} is longer than {limit} bytes.")]
    TooLong { name: &'static str, limit: usize },
    #[error("KIRI_BASE_URL must be an http or https URL.")]
    InvalidBaseUrl,
    #[error("KIRI_REQUEST_PARAMS must be a JSON object.")]
    InvalidRequestParams,
    #[error("KIRI_REQUEST_PARAMS cannot set {0}, because Kiri sets it.")]
    ReservedRequestParam(&'static str),
    #[error("KIRI_THINKING must be on or off.")]
    InvalidThinking,
    #[error(
        "{0} is not in Kiri's catalog, so KIRI_REASONING_EFFORT and KIRI_THINKING do not apply. Use KIRI_REQUEST_PARAMS."
    )]
    NotInCatalog(String),
    #[error("{model} accepts these KIRI_REASONING_EFFORT values: {accepted}. Got: {got}.")]
    UnacceptedEffort {
        model: String,
        accepted: String,
        got: String,
    },
    #[error(
        "{0} has no reasoning effort setting in Kiri. Unset KIRI_REASONING_EFFORT, or use KIRI_REQUEST_PARAMS."
    )]
    NoEffortSetting(String),
    #[error("{0} has no thinking switch in Kiri. Unset KIRI_THINKING, or use KIRI_REQUEST_PARAMS.")]
    NoThinkingSwitch(String),
    #[error("Kiri could not set up its HTTP client: {0}")]
    HttpClient(String),
}

pub(crate) fn read() -> Result<Config, ConfigError> {
    validate(|name| std::env::var(name).ok())
}

fn validate(lookup: impl Fn(&str) -> Option<String>) -> Result<Config, ConfigError> {
    let required = |name: &'static str| lookup(name).filter(|value| !value.is_empty());
    let (base_url, api_key, model) = (required(BASE_URL), required(API_KEY), required(MODEL));
    let (Some(base_url), Some(api_key), Some(model)) = (&base_url, &api_key, &model) else {
        let missing: Vec<&str> = [
            (BASE_URL, base_url.is_none()),
            (API_KEY, api_key.is_none()),
            (MODEL, model.is_none()),
        ]
        .into_iter()
        .filter_map(|(name, is_missing)| is_missing.then_some(name))
        .collect();
        return Err(ConfigError::Missing(missing.join(", ")));
    };
    let request_params = lookup(REQUEST_PARAMS);
    for (name, value, limit) in [
        (BASE_URL, Some(base_url), BASE_URL_LIMIT),
        (API_KEY, Some(api_key), API_KEY_LIMIT),
        (MODEL, Some(model), MODEL_LIMIT),
        (
            REQUEST_PARAMS,
            request_params.as_ref(),
            REQUEST_PARAMS_LIMIT,
        ),
    ] {
        if value.is_some_and(|value| value.len() > limit) {
            return Err(ConfigError::TooLong { name, limit });
        }
    }
    let is_http = Url::parse(base_url).is_ok_and(|url| matches!(url.scheme(), "http" | "https"));
    if !is_http {
        return Err(ConfigError::InvalidBaseUrl);
    }
    let pass_through = match request_params {
        Some(value) => serde_json::from_str::<Map<String, Value>>(&value)
            .map_err(|_| ConfigError::InvalidRequestParams)?,
        None => Map::new(),
    };
    if let Some(key) = RESERVED_BODY_KEYS
        .into_iter()
        .find(|key| pass_through.contains_key(*key))
    {
        return Err(ConfigError::ReservedRequestParam(key));
    }
    let thinking = match lookup(THINKING).as_deref() {
        Some("on") => Some(true),
        Some("off") => Some(false),
        Some(_) => return Err(ConfigError::InvalidThinking),
        None => None,
    };
    let (mut request_params, replays_reasoning) =
        catalog_settings(model, lookup(REASONING_EFFORT), thinking)?;
    request_params.extend(pass_through);
    Ok(Config {
        base_url: base_url.clone(),
        api_key: api_key.clone(),
        model: model.clone(),
        request_params,
        replays_reasoning,
    })
}

fn catalog_settings(
    model: &str,
    effort: Option<String>,
    thinking: Option<bool>,
) -> Result<(Map<String, Value>, bool), ConfigError> {
    let Some(entry) = catalog::find(model) else {
        if effort.is_some() || thinking.is_some() {
            return Err(ConfigError::NotInCatalog(model.to_owned()));
        }
        return Ok((Map::new(), false));
    };
    let mut settings = Map::new();
    if let Some(effort) = effort {
        if entry.efforts.is_empty() {
            return Err(ConfigError::NoEffortSetting(model.to_owned()));
        }
        if !entry.efforts.contains(&effort.as_str()) {
            return Err(ConfigError::UnacceptedEffort {
                model: model.to_owned(),
                accepted: entry.efforts.join(", "),
                got: effort,
            });
        }
        settings.insert(catalog::EFFORT_KEY.to_owned(), Value::String(effort));
    }
    if let Some(enabled) = thinking {
        let Some(format) = entry.thinking else {
            return Err(ConfigError::NoThinkingSwitch(model.to_owned()));
        };
        let (key, value) = format.body_entry(enabled);
        settings.insert(key.to_owned(), value);
    }
    Ok((settings, entry.replays_reasoning))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn validated(variables: &[(&str, &str)]) -> Result<Config, ConfigError> {
        validate(|name| {
            variables
                .iter()
                .find(|(variable, _)| *variable == name)
                .map(|(_, value)| (*value).to_owned())
        })
    }

    fn error_of(variables: &[(&str, &str)]) -> String {
        match validated(variables) {
            Ok(_) => panic!("the configuration was accepted"),
            Err(cause) => cause.to_string(),
        }
    }

    fn valid_with(name: &'static str, value: &'static str) -> Vec<(&'static str, &'static str)> {
        let mut variables = vec![
            (BASE_URL, "https://example.com/v1"),
            (API_KEY, "key"),
            (MODEL, "model"),
        ];
        variables.retain(|(variable, _)| *variable != name);
        variables.push((name, value));
        variables
    }

    fn on_model(
        model: &'static str,
        settings: &[(&'static str, &'static str)],
    ) -> Vec<(&'static str, &'static str)> {
        let mut variables = valid_with(MODEL, model);
        variables.extend_from_slice(settings);
        variables
    }

    fn body_settings(variables: &[(&str, &str)]) -> Value {
        let config = validated(variables).unwrap_or_else(|_| panic!("refused"));
        Value::Object(config.request_params)
    }

    #[test]
    fn a_complete_configuration_keeps_its_values() {
        let config =
            validated(&valid_with(REQUEST_PARAMS, "{}")).unwrap_or_else(|_| panic!("refused"));
        assert_eq!(config.base_url, "https://example.com/v1");
        assert_eq!(config.api_key, "key");
        assert_eq!(config.model, "model");
        assert!(config.request_params.is_empty());
        assert!(!config.replays_reasoning);
    }

    #[test]
    fn missing_and_empty_variables_are_all_named() {
        assert_eq!(
            error_of(&[(API_KEY, "")]),
            "Set KIRI_BASE_URL, KIRI_API_KEY, and KIRI_MODEL before opening Kiri. \
             Missing: KIRI_BASE_URL, KIRI_API_KEY, KIRI_MODEL."
        );
    }

    #[test]
    fn each_limit_admits_its_length_and_refuses_one_byte_more() {
        let cases = [
            (
                BASE_URL,
                BASE_URL_LIMIT,
                "KIRI_BASE_URL is longer than 2048 bytes.",
            ),
            (
                API_KEY,
                API_KEY_LIMIT,
                "KIRI_API_KEY is longer than 4096 bytes.",
            ),
            (MODEL, MODEL_LIMIT, "KIRI_MODEL is longer than 256 bytes."),
            (
                REQUEST_PARAMS,
                REQUEST_PARAMS_LIMIT,
                "KIRI_REQUEST_PARAMS is longer than 65536 bytes.",
            ),
        ];
        for (name, limit, message) in cases {
            let value_of = |length: usize| {
                let value = match name {
                    BASE_URL => format!("http://{}", "k".repeat(length - "http://".len())),
                    REQUEST_PARAMS => format!("{{{}}}", " ".repeat(length - 2)),
                    _ => "k".repeat(length),
                };
                &*value.leak()
            };
            assert!(validated(&valid_with(name, value_of(limit))).is_ok());
            assert_eq!(error_of(&valid_with(name, value_of(limit + 1))), message);
        }
    }

    #[test]
    fn only_http_and_https_base_urls_pass() {
        for refused in ["ftp://example.com", "example.com", "http://"] {
            assert_eq!(
                error_of(&valid_with(BASE_URL, refused)),
                "KIRI_BASE_URL must be an http or https URL."
            );
        }
        assert!(validated(&valid_with(BASE_URL, "http://127.0.0.1:8080")).is_ok());
    }

    #[test]
    fn thinking_accepts_only_on_and_off() {
        assert!(validated(&on_model("qwen3.8-max", &[(THINKING, "on")])).is_ok());
        for refused in ["auto", ""] {
            assert_eq!(
                error_of(&valid_with(THINKING, refused)),
                "KIRI_THINKING must be on or off."
            );
        }
    }

    #[test]
    fn the_first_failure_in_the_table_order_is_the_one_reported() {
        let long_model: &str = "m".repeat(MODEL_LIMIT + 1).leak();
        assert_eq!(
            error_of(&[
                (BASE_URL, "ftp://example.com"),
                (API_KEY, "key"),
                (MODEL, long_model),
                (THINKING, "auto"),
            ]),
            "KIRI_MODEL is longer than 256 bytes."
        );
        let cases = [
            (
                vec![
                    (BASE_URL, "ftp://example.com"),
                    (REQUEST_PARAMS, "[]"),
                    (API_KEY, "key"),
                    (MODEL, "model"),
                ],
                "KIRI_BASE_URL must be an http or https URL.",
            ),
            (
                on_model("model", &[(REQUEST_PARAMS, "[]"), (THINKING, "auto")]),
                "KIRI_REQUEST_PARAMS must be a JSON object.",
            ),
            (
                on_model(
                    "model",
                    &[(REQUEST_PARAMS, r#"{"stream":false}"#), (THINKING, "auto")],
                ),
                "KIRI_REQUEST_PARAMS cannot set stream, because Kiri sets it.",
            ),
            (
                on_model("model", &[(THINKING, "auto"), (REASONING_EFFORT, "low")]),
                "KIRI_THINKING must be on or off.",
            ),
        ];
        for (variables, message) in cases {
            assert_eq!(error_of(&variables), message);
        }
    }

    #[test]
    fn request_params_must_be_a_json_object() {
        for refused in ["[1,2]", "{oops", "", "1", "null"] {
            assert_eq!(
                error_of(&valid_with(REQUEST_PARAMS, refused)),
                "KIRI_REQUEST_PARAMS must be a JSON object."
            );
        }
    }

    #[test]
    fn request_params_name_the_first_reserved_key() {
        let cases = [
            (r#"{"model":"x"}"#, "model"),
            (r#"{"messages":[]}"#, "messages"),
            (r#"{"stream":false}"#, "stream"),
            (r#"{"stream":false,"messages":[],"model":"x"}"#, "model"),
        ];
        for (params, key) in cases {
            assert_eq!(
                error_of(&valid_with(REQUEST_PARAMS, params)),
                format!("KIRI_REQUEST_PARAMS cannot set {key}, because Kiri sets it.")
            );
        }
    }

    #[test]
    fn a_setting_the_catalog_cannot_send_is_refused_with_its_message() {
        let cases = [
            (
                on_model("glm-5.3", &[(REASONING_EFFORT, "medium")]),
                "glm-5.3 accepts these KIRI_REASONING_EFFORT values: low, high, max. Got: medium.",
            ),
            (
                on_model("glm-5.3", &[(REASONING_EFFORT, "HIGH")]),
                "glm-5.3 accepts these KIRI_REASONING_EFFORT values: low, high, max. Got: HIGH.",
            ),
            (
                on_model("gemini-4-argon", &[(REASONING_EFFORT, "")]),
                "gemini-4-argon accepts these KIRI_REASONING_EFFORT values: low, medium, high. Got: .",
            ),
            (
                on_model("qwen3.8-max", &[(REASONING_EFFORT, "low")]),
                "qwen3.8-max has no reasoning effort setting in Kiri. Unset KIRI_REASONING_EFFORT, or use KIRI_REQUEST_PARAMS.",
            ),
            (
                on_model("glm-5.3", &[(THINKING, "on")]),
                "glm-5.3 has no thinking switch in Kiri. Unset KIRI_THINKING, or use KIRI_REQUEST_PARAMS.",
            ),
            (
                on_model("other", &[(THINKING, "on")]),
                "other is not in Kiri's catalog, so KIRI_REASONING_EFFORT and KIRI_THINKING do not apply. Use KIRI_REQUEST_PARAMS.",
            ),
            (
                on_model("other", &[(REASONING_EFFORT, "")]),
                "other is not in Kiri's catalog, so KIRI_REASONING_EFFORT and KIRI_THINKING do not apply. Use KIRI_REQUEST_PARAMS.",
            ),
        ];
        for (variables, message) in cases {
            assert_eq!(error_of(&variables), message);
        }
    }

    #[test]
    fn pass_through_wins_over_the_catalog_but_does_not_excuse_an_invalid_setting() {
        assert_eq!(
            body_settings(&on_model(
                "deepseek-v4-pro",
                &[
                    (REASONING_EFFORT, "high"),
                    (THINKING, "on"),
                    (REQUEST_PARAMS, r#"{"reasoning_effort":"max","tools":[]}"#),
                ],
            )),
            serde_json::json!({
                "reasoning_effort": "max",
                "thinking": {"type": "enabled"},
                "tools": [],
            })
        );
        assert_eq!(
            error_of(&on_model(
                "deepseek-v4-pro",
                &[
                    (REASONING_EFFORT, "medium"),
                    (REQUEST_PARAMS, r#"{"reasoning_effort":"max"}"#),
                ],
            )),
            "deepseek-v4-pro accepts these KIRI_REASONING_EFFORT values: low, high, max. Got: medium."
        );
    }

    #[test]
    fn a_catalog_model_with_nothing_set_adds_nothing_to_the_body() {
        let config =
            validated(&on_model("kimi-k3.1-pro", &[])).unwrap_or_else(|_| panic!("refused"));
        assert!(config.request_params.is_empty());
        assert!(config.replays_reasoning);
    }
}
