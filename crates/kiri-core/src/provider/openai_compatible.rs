use std::collections::VecDeque;

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
    Reasoning(String),
    Text(String),
    ToolCallFragment {
        index: u64,
        id: Option<String>,
        name: Option<String>,
        arguments: Option<String>,
    },
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
            pending: VecDeque::new(),
            finished: false,
        })
    }
}

pub(crate) struct Reply {
    events: BoxStream<'static, Result<Event, EventStreamError<reqwest::Error>>>,
    pending: VecDeque<ProviderEvent>,
    finished: bool,
}

impl Reply {
    pub(crate) async fn next_event(&mut self) -> Result<Option<ProviderEvent>, ProviderError> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Ok(Some(event));
            }
            if self.finished {
                return Ok(None);
            }
            let event = match self.events.next().await {
                Some(Ok(event)) => event,
                Some(Err(_)) | None => {
                    tracing::warn!("the endpoint's stream broke before the turn finished");
                    return Err(ProviderError);
                }
            };
            let (events, finished) = read_chunk(&event.data).map_err(|_| {
                tracing::warn!("the endpoint sent an unreadable chunk");
                ProviderError
            })?;
            self.finished = finished;
            self.pending.extend(events);
        }
    }
}

// shortcut: any finish reason ends the stream the same way, #19 maps them to stop reasons.
fn read_chunk(data: &str) -> Result<(Vec<ProviderEvent>, bool), serde_json::Error> {
    if data == DONE {
        return Ok((Vec::new(), true));
    }
    let chunk: Value = serde_json::from_str(data)?;
    let Some(choice) = chunk.pointer("/choices/0") else {
        return Ok((Vec::new(), false));
    };
    let delta = &choice["delta"];
    let mut events = Vec::new();
    if let Some(reasoning) =
        non_empty_text(&delta["reasoning_content"]).or_else(|| non_empty_text(&delta["reasoning"]))
    {
        events.push(ProviderEvent::Reasoning(reasoning));
    }
    if let Some(text) = non_empty_text(&delta["content"]) {
        events.push(ProviderEvent::Text(text));
    }
    let tool_calls = delta["tool_calls"].as_array().into_iter().flatten();
    for (position, tool_call) in tool_calls.enumerate() {
        events.push(ProviderEvent::ToolCallFragment {
            index: tool_call["index"].as_u64().unwrap_or(position as u64),
            id: non_empty_text(&tool_call["id"]),
            name: non_empty_text(&tool_call["function"]["name"]),
            arguments: non_empty_text(&tool_call["function"]["arguments"]),
        });
    }
    let finished = choice.get("finish_reason").is_some_and(Value::is_string);
    Ok((events, finished))
}

fn non_empty_text(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(text: &str) -> ProviderEvent {
        ProviderEvent::Text(text.to_owned())
    }

    fn reasoning(text: &str) -> ProviderEvent {
        ProviderEvent::Reasoning(text.to_owned())
    }

    #[test]
    fn a_chunk_gives_its_events_and_whether_the_stream_finished() {
        let cases = [
            (
                r#"{"choices":[{"delta":{"content":"hi"},"finish_reason":null}]}"#,
                vec![text("hi")],
                false,
            ),
            (
                r#"{"choices":[{"delta":{"content":"end"},"finish_reason":"stop"}]}"#,
                vec![text("end")],
                true,
            ),
            (
                r#"{"choices":[{"delta":{},"finish_reason":"length"}]}"#,
                vec![],
                true,
            ),
            (r#"{"choices":[],"usage":{}}"#, vec![], false),
            ("[DONE]", vec![], true),
            (
                r#"{"choices":[{"delta":{"reasoning_content":"a"}}]}"#,
                vec![reasoning("a")],
                false,
            ),
            (
                r#"{"choices":[{"delta":{"reasoning":"b"}}]}"#,
                vec![reasoning("b")],
                false,
            ),
            (
                r#"{"choices":[{"delta":{"reasoning_content":null,"reasoning":"c"}}]}"#,
                vec![reasoning("c")],
                false,
            ),
            (
                r#"{"choices":[{"delta":{"reasoning_content":"","reasoning":"d"}}]}"#,
                vec![reasoning("d")],
                false,
            ),
            (
                r#"{"choices":[{"delta":{"reasoning_content":"e","reasoning":"f"}}]}"#,
                vec![reasoning("e")],
                false,
            ),
        ];
        for (data, events, finished) in cases {
            let (read_events, read_finished) = read_chunk(data).unwrap();
            assert_eq!(read_events, events, "{data}");
            assert_eq!(read_finished, finished, "{data}");
        }
    }

    #[test]
    fn a_chunk_with_every_part_gives_reasoning_then_text_then_tool_calls() {
        let data = r#"{"choices":[{"delta":{
            "tool_calls":[
                {"id":"x","function":{"name":"first","arguments":"{"}},
                {"index":7,"function":{"arguments":"}"}}
            ],
            "content":"t",
            "reasoning":"r"
        },"finish_reason":"tool_calls"}]}"#;
        let (events, finished) = read_chunk(data).unwrap();
        assert_eq!(
            events,
            vec![
                reasoning("r"),
                text("t"),
                ProviderEvent::ToolCallFragment {
                    index: 0,
                    id: Some("x".to_owned()),
                    name: Some("first".to_owned()),
                    arguments: Some("{".to_owned()),
                },
                ProviderEvent::ToolCallFragment {
                    index: 7,
                    id: None,
                    name: None,
                    arguments: Some("}".to_owned()),
                },
            ]
        );
        assert!(finished);
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
