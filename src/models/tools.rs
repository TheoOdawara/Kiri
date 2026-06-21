use serde::Serialize;

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
}
