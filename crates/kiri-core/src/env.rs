use std::path::PathBuf;

use reqwest::Url;
use thiserror::Error;

const BASE_URL: &str = "KIRI_BASE_URL";
const API_KEY: &str = "KIRI_API_KEY";
const MODEL: &str = "KIRI_MODEL";
const THINKING: &str = "KIRI_THINKING";
const BASE_URL_LIMIT: usize = 2048;
const API_KEY_LIMIT: usize = 4096;
const MODEL_LIMIT: usize = 256;

pub(crate) fn home_directory() -> Option<PathBuf> {
    std::env::home_dir()
}

pub(crate) struct Config {
    pub(crate) base_url: String,
    pub(crate) api_key: String,
    pub(crate) model: String,
}

#[derive(Debug, Error, PartialEq)]
pub(crate) enum ConfigError {
    #[error("Set KIRI_BASE_URL, KIRI_API_KEY, and KIRI_MODEL before opening Kiri. Missing: {0}.")]
    Missing(String),
    #[error("{name} is longer than {limit} bytes.")]
    TooLong { name: &'static str, limit: usize },
    #[error("KIRI_BASE_URL must be an http or https URL.")]
    InvalidBaseUrl,
    #[error("KIRI_THINKING must be on or off.")]
    InvalidThinking,
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
    for (name, value, limit) in [
        (BASE_URL, base_url, BASE_URL_LIMIT),
        (API_KEY, api_key, API_KEY_LIMIT),
        (MODEL, model, MODEL_LIMIT),
    ] {
        if value.len() > limit {
            return Err(ConfigError::TooLong { name, limit });
        }
    }
    let is_http = Url::parse(base_url).is_ok_and(|url| matches!(url.scheme(), "http" | "https"));
    if !is_http {
        return Err(ConfigError::InvalidBaseUrl);
    }
    // shortcut: KIRI_THINKING is only validated, #18 sends it with the catalog.
    if lookup(THINKING).is_some_and(|value| !matches!(value.as_str(), "on" | "off")) {
        return Err(ConfigError::InvalidThinking);
    }
    Ok(Config {
        base_url: base_url.clone(),
        api_key: api_key.clone(),
        model: model.clone(),
    })
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

    #[test]
    fn a_complete_configuration_keeps_its_values() {
        let config = validated(&valid_with(THINKING, "off")).unwrap_or_else(|_| panic!("refused"));
        assert_eq!(config.base_url, "https://example.com/v1");
        assert_eq!(config.api_key, "key");
        assert_eq!(config.model, "model");
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
        ];
        for (name, limit, message) in cases {
            let value_of = |length: usize| {
                let value = if name == BASE_URL {
                    format!("http://{}", "k".repeat(length - "http://".len()))
                } else {
                    "k".repeat(length)
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
        assert!(validated(&valid_with(THINKING, "on")).is_ok());
        assert_eq!(
            error_of(&valid_with(THINKING, "auto")),
            "KIRI_THINKING must be on or off."
        );
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
    }
}
