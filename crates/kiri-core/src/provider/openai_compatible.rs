use eventsource_stream::{Event, EventStreamError, Eventsource};
use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use reqwest::{Client, redirect};
use serde_json::{Value, json};
use thiserror::Error;

use crate::env::{Config, ConfigError};
use crate::session::Message;

const DONE: &str = "[DONE]";

// shortcut: one message for every failure and none of the endpoint's text, #19 maps each failure and redacts the key.
#[derive(Debug, Error)]
#[error("The request to the endpoint failed.")]
pub(crate) struct ProviderError;

#[derive(Debug, PartialEq)]
pub(crate) enum ProviderEvent {
    Text(String),
}

pub(crate) struct OpenAiCompatible {
    client: Client,
    url: String,
    api_key: String,
    model: String,
}

impl OpenAiCompatible {
    pub(crate) fn new(config: Config) -> Result<Self, ConfigError> {
        let client = Client::builder()
            .redirect(redirect::Policy::none())
            .user_agent(concat!("kiri/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|cause| ConfigError::HttpClient(cause.without_url().to_string()))?;
        let base_url = config.base_url;
        Ok(Self {
            client,
            url: format!(
                "{}/chat/completions",
                base_url.strip_suffix('/').unwrap_or(&base_url)
            ),
            api_key: config.api_key,
            model: config.model,
        })
    }

    pub(crate) async fn send(&self, messages: &[Message]) -> Result<Reply, ProviderError> {
        let messages: Vec<Value> = messages
            .iter()
            .map(|message| json!({"role": message.role.as_str(), "content": message.text}))
            .collect();
        let body = json!({"model": self.model, "messages": messages, "stream": true});
        let response = self
            .client
            .post(&self.url)
            .bearer_auth(&self.api_key)
            .header(ACCEPT, "text/event-stream")
            .header(CONTENT_TYPE, "application/json")
            .body(body.to_string())
            .send()
            .await
            .map_err(|cause| {
                tracing::warn!(cause = %cause.without_url(), "could not reach the endpoint");
                ProviderError
            })?;
        let status = response.status();
        if !status.is_success() {
            tracing::warn!(status = status.as_u16(), "the endpoint refused a request");
            return Err(ProviderError);
        }
        Ok(Reply {
            events: response.bytes_stream().eventsource().boxed(),
            finished: false,
        })
    }
}

pub(crate) struct Reply {
    events: BoxStream<'static, Result<Event, EventStreamError<reqwest::Error>>>,
    finished: bool,
}

impl Reply {
    pub(crate) async fn next_event(&mut self) -> Result<Option<ProviderEvent>, ProviderError> {
        while !self.finished {
            let event = match self.events.next().await {
                Some(Ok(event)) => event,
                Some(Err(_)) | None => {
                    tracing::warn!("the endpoint's stream broke before the turn finished");
                    return Err(ProviderError);
                }
            };
            let (text, finished) = read_chunk(&event.data).map_err(|_| {
                tracing::warn!("the endpoint sent an unreadable chunk");
                ProviderError
            })?;
            self.finished = finished;
            if let Some(text) = text {
                return Ok(Some(ProviderEvent::Text(text)));
            }
        }
        Ok(None)
    }
}

// shortcut: reads only delta.content, #17 adds reasoning and tool calls, and #19 maps the finish reasons.
fn read_chunk(data: &str) -> Result<(Option<String>, bool), serde_json::Error> {
    if data == DONE {
        return Ok((None, true));
    }
    let chunk: Value = serde_json::from_str(data)?;
    let Some(choice) = chunk.pointer("/choices/0") else {
        return Ok((None, false));
    };
    let text = choice
        .pointer("/delta/content")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned);
    let finished = choice.get("finish_reason").is_some_and(Value::is_string);
    Ok((text, finished))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chunk_gives_its_text_and_whether_the_stream_finished() {
        let cases = [
            (
                r#"{"choices":[{"delta":{"content":"hi"},"finish_reason":null}]}"#,
                Some("hi"),
                false,
            ),
            (
                r#"{"choices":[{"delta":{"content":"end"},"finish_reason":"stop"}]}"#,
                Some("end"),
                true,
            ),
            (
                r#"{"choices":[{"delta":{},"finish_reason":"length"}]}"#,
                None,
                true,
            ),
            (r#"{"choices":[],"usage":{}}"#, None, false),
            ("[DONE]", None, true),
        ];
        for (data, text, finished) in cases {
            let (read_text, read_finished) = read_chunk(data).unwrap();
            assert_eq!(read_text.as_deref(), text, "{data}");
            assert_eq!(read_finished, finished, "{data}");
        }
    }

    #[test]
    fn a_chunk_that_is_not_json_is_an_error() {
        assert!(read_chunk("{oops").is_err());
    }

    #[test]
    fn one_trailing_slash_of_the_base_url_is_removed() {
        let url_of = |base_url: &str| {
            OpenAiCompatible::new(Config {
                base_url: base_url.to_owned(),
                api_key: "key".to_owned(),
                model: "model".to_owned(),
            })
            .unwrap_or_else(|_| panic!("no client"))
            .url
        };
        assert_eq!(url_of("http://a/v1"), "http://a/v1/chat/completions");
        assert_eq!(url_of("http://a/v1/"), "http://a/v1/chat/completions");
        assert_eq!(url_of("http://a/v1//"), "http://a/v1//chat/completions");
    }
}
