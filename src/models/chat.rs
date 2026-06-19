use serde::{Deserialize, Serialize};

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
        };

        let value: serde_json::Value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["model"], model);
        assert_eq!(value["stream"], true);
        assert_eq!(value["messages"][0]["role"], "user");
        assert_eq!(value["messages"][0]["content"], "hi");
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
}
