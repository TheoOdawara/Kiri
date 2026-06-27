use serde::{Deserialize, Serialize};

/// A tool call the model emitted, assembled from its streamed fragments and re-sent in history.
/// Cross-cutting protocol primitive: the agent stores it in history, the provider assembles it,
/// and the tools layer executes it — so it lives in the kernel, depended on by all three.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", default = "default_function_type")]
    pub kind: String,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    /// JSON-encoded argument string, exactly as the model produced it.
    pub arguments: String,
}

/// The canonical OpenAI tool-call `type`. The only value the chat-completions API assigns, and the kind
/// the agent re-sends in history — named once so the SSE accumulators and the serde default agree.
pub const TOOL_CALL_FUNCTION_KIND: &str = "function";

fn default_function_type() -> String {
    TOOL_CALL_FUNCTION_KIND.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_call_round_trips() {
        let call = ToolCall {
            id: "call_1".to_string(),
            kind: "function".to_string(),
            function: FunctionCall {
                name: "write_file".to_string(),
                arguments: r#"{"path":"a.txt"}"#.to_string(),
            },
        };
        let json = serde_json::to_string(&call).unwrap();
        let back: ToolCall = serde_json::from_str(&json).unwrap();
        assert_eq!(back, call);
    }

    #[test]
    fn tool_call_defaults_type_when_provider_omits_it() {
        let back: ToolCall =
            serde_json::from_str(r#"{"id":"c1","function":{"name":"x","arguments":"{}"}}"#)
                .unwrap();
        assert_eq!(back.kind, "function");
    }

    #[test]
    fn default_function_type_equals_the_const() {
        assert_eq!(default_function_type(), TOOL_CALL_FUNCTION_KIND);
    }
}
