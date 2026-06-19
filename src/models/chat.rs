use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_template_kwargs: Option<ChatTemplateKwargs>,
}

/// Provider-specific knob that asks the model to emit reasoning. Reasoning models stream it by
/// default; sending this makes the intent explicit.
#[derive(Debug, Serialize)]
pub struct ChatTemplateKwargs {
    pub thinking: bool,
}

#[derive(Debug, Deserialize)]
pub struct ChatStreamChunk {
    pub choices: Vec<StreamChoice>,
}

#[derive(Debug, Deserialize)]
pub struct StreamChoice {
    pub delta: Delta,
}

#[derive(Debug, Deserialize)]
pub struct Delta {
    pub content: Option<String>,
    /// Reasoning text under the standard `reasoning_content` name (vLLM/NVIDIA convention).
    #[serde(default, deserialize_with = "string_or_none")]
    pub reasoning_content: Option<String>,
    /// Some providers (and NVIDIA Nemotron) also/instead send `reasoning`. Kept as its own field:
    /// a serde `alias` would make a delta carrying BOTH keys fail as a duplicate field.
    #[serde(default, deserialize_with = "string_or_none")]
    pub reasoning: Option<String>,
}

/// Serde adapter: accept a string into `Some`; coerce any other JSON shape (object, list, number,
/// null) to `None`. Keeps an unexpected reasoning shape from failing the whole delta and dropping
/// its `content`.
fn string_or_none<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    Ok(
        match Option::<serde_json::Value>::deserialize(deserializer)? {
            Some(serde_json::Value::String(text)) => Some(text),
            _ => None,
        },
    )
}

#[derive(Debug, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&Role::System).unwrap(), "\"system\"");
        assert_eq!(serde_json::to_string(&Role::User).unwrap(), "\"user\"");
        assert_eq!(
            serde_json::to_string(&Role::Assistant).unwrap(),
            "\"assistant\""
        );
    }

    #[test]
    fn chat_request_serializes_expected_shape() {
        dotenvy::dotenv().ok();
        let model = std::env::var("NVIDIA_MODEL").expect("NVIDIA_MODEL must be set in .env");

        let request = ChatRequest {
            model: model.clone(),
            messages: vec![Message {
                role: Role::User,
                content: "hi".to_string(),
            }],
            stream: true,
            chat_template_kwargs: None,
        };

        let value: serde_json::Value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["model"], model);
        assert_eq!(value["stream"], true);
        assert_eq!(value["messages"][0]["role"], "user");
        assert_eq!(value["messages"][0]["content"], "hi");
    }

    #[test]
    fn chat_template_kwargs_omitted_when_none() {
        let request = ChatRequest {
            model: "m".to_string(),
            messages: vec![],
            stream: true,
            chat_template_kwargs: None,
        };
        let value: serde_json::Value = serde_json::to_value(&request).unwrap();
        assert!(value.get("chat_template_kwargs").is_none());
    }

    #[test]
    fn chat_template_kwargs_serializes_nested() {
        let request = ChatRequest {
            model: "m".to_string(),
            messages: vec![],
            stream: true,
            chat_template_kwargs: Some(ChatTemplateKwargs { thinking: true }),
        };
        let value: serde_json::Value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["chat_template_kwargs"]["thinking"], true);
    }

    #[test]
    fn message_round_trips() {
        let message = Message {
            role: Role::Assistant,
            content: "ok".to_string(),
        };
        let json = serde_json::to_string(&message).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back.role, Role::Assistant);
        assert_eq!(back.content, "ok");
    }

    #[test]
    fn delta_keeps_content_when_reasoning_is_not_a_string() {
        let delta: Delta =
            serde_json::from_str(r#"{"reasoning":{"step":1},"content":"Hi"}"#).unwrap();
        assert_eq!(delta.content.as_deref(), Some("Hi"));
        assert_eq!(delta.reasoning, None);
        assert_eq!(delta.reasoning_content, None);
    }

    #[test]
    fn delta_reads_reasoning_string() {
        let delta: Delta = serde_json::from_str(r#"{"reasoning_content":"why"}"#).unwrap();
        assert_eq!(delta.reasoning_content.as_deref(), Some("why"));
    }

    #[test]
    fn delta_accepts_both_reasoning_keys_at_once() {
        // NVIDIA Nemotron streams `reasoning` and `reasoning_content` together; this must not fail.
        let delta: Delta =
            serde_json::from_str(r#"{"reasoning":"Okay","reasoning_content":"Okay"}"#).unwrap();
        assert_eq!(delta.reasoning.as_deref(), Some("Okay"));
        assert_eq!(delta.reasoning_content.as_deref(), Some("Okay"));
    }
}
