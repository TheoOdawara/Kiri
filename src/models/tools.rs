use serde::{Deserialize, Serialize};

/// A tool advertised to the model in a chat request.
#[derive(Debug, Clone, Serialize)]
pub struct Tool {
    #[serde(rename = "type")]
    pub kind: ToolKind,
    pub function: FunctionDef,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolKind {
    Function,
}

/// A function tool's schema: name, description, and JSON Schema parameters. The parameters are kept as
/// a `serde_json::Value` because the schemas are static literals; modeling JSON Schema as Rust types
/// would add weight without value.
#[derive(Debug, Clone, Serialize)]
pub struct FunctionDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// A tool call the model emitted, assembled from its streamed fragments and re-sent in history.
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

fn default_function_type() -> String {
    "function".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_serializes_type_function_and_parameters() {
        let tool = Tool {
            kind: ToolKind::Function,
            function: FunctionDef {
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                parameters: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            },
        };
        let value = serde_json::to_value(&tool).unwrap();
        assert_eq!(value["type"], "function");
        assert_eq!(value["function"]["name"], "read_file");
        assert_eq!(value["function"]["parameters"]["type"], "object");
    }

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
}
