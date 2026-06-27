//! Domain → Anthropic Messages translation. The Messages API differs from chat-completions in three
//! ways this module bridges: `system` is a top-level field (not a `system` message), roles must strictly
//! alternate user/assistant (so consecutive same-role domain messages — e.g. parallel tool results —
//! are merged into one), and tool calls/results are content blocks (`tool_use`/`tool_result`) whose
//! tool schemas use `{name, description, input_schema}` rather than the OpenAI `{type, function}` shape.

use serde::Serialize;
use serde_json::{Value, json};

use crate::shared::kernel::message::Message;
use crate::shared::kernel::role::Role;

/// One Anthropic message: a `user`/`assistant` role and its content blocks. Built owned (rather than
/// borrowing the domain messages) because translation merges messages and parses tool inputs into JSON
/// — the request is assembled once per turn, so the allocations are negligible against the network call.
#[derive(Debug, Serialize)]
pub(crate) struct AnthropicMessage {
    pub role: &'static str,
    pub content: Vec<ContentBlock>,
}

/// A single content block. The internal `type` tag selects the shape, matching the Messages API.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
    },
    Image {
        source: ImageSource,
    },
}

/// A base64 image source (`data:<media_type>;base64,<data>` parsed out of the domain data URL).
#[derive(Debug, Serialize)]
pub(crate) struct ImageSource {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub media_type: String,
    pub data: String,
}

/// Split the domain messages into the top-level `system` text and the alternating user/assistant
/// messages. System messages are concatenated (blank-line separated); every other message is mapped to
/// its content blocks and merged into the previous message when they share an Anthropic role.
pub(crate) fn build_messages(messages: &[Message]) -> (Option<String>, Vec<AnthropicMessage>) {
    let mut system = String::new();
    let mut out: Vec<AnthropicMessage> = Vec::new();

    for message in messages {
        if message.role == Role::System {
            if let Some(text) = message.content.as_deref().filter(|t| !t.is_empty()) {
                if !system.is_empty() {
                    system.push_str("\n\n");
                }
                system.push_str(text);
            }
            continue;
        }
        let role = anthropic_role(message.role);
        let blocks = blocks_for(message);
        if blocks.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.role == role => last.content.extend(blocks),
            _ => out.push(AnthropicMessage {
                role,
                content: blocks,
            }),
        }
    }

    let system = (!system.is_empty()).then_some(system);
    (system, out)
}

/// The Anthropic role for a domain message. `Tool` results are carried in a `user` message (the
/// Messages API has no tool role); `System` is handled out-of-band by [`build_messages`].
fn anthropic_role(role: Role) -> &'static str {
    match role {
        Role::Assistant => "assistant",
        _ => "user",
    }
}

/// The content blocks for one non-system message.
fn blocks_for(message: &Message) -> Vec<ContentBlock> {
    match message.role {
        Role::Tool => message
            .tool_call_id
            .as_deref()
            .map(|id| {
                vec![ContentBlock::ToolResult {
                    tool_use_id: id.to_string(),
                    content: message.content.clone().unwrap_or_default(),
                }]
            })
            .unwrap_or_default(),
        Role::Assistant if !message.tool_calls.is_empty() => {
            let mut blocks = Vec::with_capacity(message.tool_calls.len() + 1);
            if let Some(text) = message.content.as_deref().filter(|t| !t.is_empty()) {
                blocks.push(ContentBlock::Text {
                    text: text.to_string(),
                });
            }
            for call in &message.tool_calls {
                blocks.push(ContentBlock::ToolUse {
                    id: call.id.clone(),
                    name: call.function.name.clone(),
                    input: parse_input(&call.function.arguments),
                });
            }
            blocks
        }
        _ => text_and_images(message),
    }
}

/// User/assistant text, plus any attached images as base64 blocks. A blank caption emits no text block;
/// an unparseable image data URL is skipped rather than sent malformed.
fn text_and_images(message: &Message) -> Vec<ContentBlock> {
    let mut blocks = Vec::new();
    if let Some(text) = message.content.as_deref().filter(|t| !t.is_empty()) {
        blocks.push(ContentBlock::Text {
            text: text.to_string(),
        });
    }
    for url in &message.images {
        if let Some(source) = parse_data_url(url) {
            blocks.push(ContentBlock::Image { source });
        }
    }
    blocks
}

/// Parse a tool call's stored JSON-string arguments into the JSON value Anthropic's `tool_use.input`
/// expects. Falls back to an empty object if the string is empty or not valid JSON, so a garbled turn
/// can never produce a malformed request body.
fn parse_input(arguments: &str) -> Value {
    let trimmed = arguments.trim();
    if trimmed.is_empty() {
        return json!({});
    }
    serde_json::from_str(trimmed).unwrap_or_else(|_| json!({}))
}

/// Parse a `data:<media_type>;base64,<data>` URL into an Anthropic base64 image source. Returns `None`
/// for any other shape (e.g. a remote URL or a non-base64 data URL), so the caller skips it.
fn parse_data_url(url: &str) -> Option<ImageSource> {
    let rest = url.strip_prefix("data:")?;
    let (meta, data) = rest.split_once(',')?;
    let media_type = meta.strip_suffix(";base64")?;
    if media_type.is_empty() || data.is_empty() {
        return None;
    }
    Some(ImageSource {
        kind: "base64",
        media_type: media_type.to_string(),
        data: data.to_string(),
    })
}

/// Translate the registry's OpenAI-shaped tool schemas (`{type:"function", function:{name, description,
/// parameters}}`) into Anthropic's (`{name, description, input_schema}`). A schema already in Anthropic
/// shape (no `function` wrapper) is passed through unchanged; a missing `parameters` defaults to an
/// empty object schema (the Messages API requires a valid `input_schema`).
pub(crate) fn translate_tools(tools: &[Value]) -> Vec<Value> {
    tools.iter().map(translate_tool).collect()
}

fn translate_tool(tool: &Value) -> Value {
    let Some(function) = tool.get("function") else {
        return tool.clone();
    };
    let mut out = serde_json::Map::new();
    if let Some(name) = function.get("name") {
        out.insert("name".to_string(), name.clone());
    }
    if let Some(description) = function.get("description") {
        out.insert("description".to_string(), description.clone());
    }
    let input_schema = function
        .get("parameters")
        .cloned()
        .unwrap_or_else(|| json!({"type": "object"}));
    out.insert("input_schema".to_string(), input_schema);
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::kernel::tool_call::{FunctionCall, ToolCall};

    fn to_value(messages: &[AnthropicMessage]) -> Value {
        serde_json::to_value(messages).unwrap()
    }

    #[test]
    fn system_messages_are_lifted_and_concatenated() {
        let messages = vec![
            Message::system("be concise"),
            Message::system("be kind"),
            Message::user("hi"),
        ];
        let (system, out) = build_messages(&messages);
        assert_eq!(system.as_deref(), Some("be concise\n\nbe kind"));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].role, "user");
    }

    #[test]
    fn user_text_becomes_a_single_text_block() {
        let (_system, out) = build_messages(&[Message::user("hello")]);
        let value = to_value(&out);
        assert_eq!(value[0]["role"], "user");
        assert_eq!(value[0]["content"][0]["type"], "text");
        assert_eq!(value[0]["content"][0]["text"], "hello");
    }

    #[test]
    fn assistant_tool_calls_become_tool_use_blocks_with_parsed_input() {
        let message = Message::assistant_tool_calls(
            Some("let me read it".to_string()),
            vec![ToolCall {
                id: "toolu_1".to_string(),
                kind: "function".to_string(),
                function: FunctionCall {
                    name: "read_file".to_string(),
                    arguments: r#"{"path":"a.txt"}"#.to_string(),
                },
            }],
        );
        let (_system, out) = build_messages(&[message]);
        let value = to_value(&out);
        assert_eq!(value[0]["role"], "assistant");
        assert_eq!(value[0]["content"][0]["type"], "text");
        assert_eq!(value[0]["content"][0]["text"], "let me read it");
        assert_eq!(value[0]["content"][1]["type"], "tool_use");
        assert_eq!(value[0]["content"][1]["id"], "toolu_1");
        assert_eq!(value[0]["content"][1]["name"], "read_file");
        // input must be a JSON object, not the raw string.
        assert_eq!(value[0]["content"][1]["input"]["path"], "a.txt");
    }

    #[test]
    fn tool_result_becomes_a_user_tool_result_block() {
        let (_system, out) = build_messages(&[Message::tool_result("toolu_1", "file contents")]);
        let value = to_value(&out);
        assert_eq!(value[0]["role"], "user");
        assert_eq!(value[0]["content"][0]["type"], "tool_result");
        assert_eq!(value[0]["content"][0]["tool_use_id"], "toolu_1");
        assert_eq!(value[0]["content"][0]["content"], "file contents");
    }

    #[test]
    fn parallel_tool_results_merge_into_one_user_message() {
        // Two consecutive tool results (parallel calls) must become ONE user message with two blocks —
        // the Messages API rejects consecutive user messages / a turn missing a tool_use's result.
        let messages = vec![
            Message::tool_result("toolu_1", "out 1"),
            Message::tool_result("toolu_2", "out 2"),
        ];
        let (_system, out) = build_messages(&messages);
        assert_eq!(out.len(), 1, "parallel results must merge into one message");
        let value = to_value(&out);
        assert_eq!(value[0]["content"].as_array().unwrap().len(), 2);
        assert_eq!(value[0]["content"][0]["tool_use_id"], "toolu_1");
        assert_eq!(value[0]["content"][1]["tool_use_id"], "toolu_2");
    }

    #[test]
    fn consecutive_assistant_messages_merge() {
        let messages = vec![
            Message::assistant_text("part one"),
            Message::assistant_text("part two"),
        ];
        let (_system, out) = build_messages(&messages);
        assert_eq!(out.len(), 1);
        let value = to_value(&out);
        assert_eq!(value[0]["content"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn roles_strictly_alternate_across_a_tool_round_trip() {
        let messages = vec![
            Message::user("read it"),
            Message::assistant_tool_calls(
                None,
                vec![ToolCall {
                    id: "toolu_1".to_string(),
                    kind: "function".to_string(),
                    function: FunctionCall {
                        name: "read_file".to_string(),
                        arguments: "{}".to_string(),
                    },
                }],
            ),
            Message::tool_result("toolu_1", "data"),
            Message::assistant_text("done"),
        ];
        let (_system, out) = build_messages(&messages);
        let roles: Vec<&str> = out.iter().map(|m| m.role).collect();
        assert_eq!(roles, vec!["user", "assistant", "user", "assistant"]);
    }

    #[test]
    fn user_images_become_base64_image_blocks() {
        let message =
            Message::user_multimodal("look", vec!["data:image/png;base64,AAAA".to_string()]);
        let (_system, out) = build_messages(&[message]);
        let value = to_value(&out);
        assert_eq!(value[0]["content"][0]["type"], "text");
        assert_eq!(value[0]["content"][1]["type"], "image");
        assert_eq!(value[0]["content"][1]["source"]["type"], "base64");
        assert_eq!(value[0]["content"][1]["source"]["media_type"], "image/png");
        assert_eq!(value[0]["content"][1]["source"]["data"], "AAAA");
    }

    #[test]
    fn malformed_image_url_is_skipped() {
        let message =
            Message::user_multimodal("look", vec!["https://example.test/x.png".to_string()]);
        let (_system, out) = build_messages(&[message]);
        let value = to_value(&out);
        let blocks = value[0]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 1, "only the text block survives");
        assert_eq!(blocks[0]["type"], "text");
    }

    #[test]
    fn invalid_tool_arguments_fall_back_to_empty_object() {
        let message = Message::assistant_tool_calls(
            None,
            vec![ToolCall {
                id: "toolu_1".to_string(),
                kind: "function".to_string(),
                function: FunctionCall {
                    name: "x".to_string(),
                    arguments: "{not json".to_string(),
                },
            }],
        );
        let (_system, out) = build_messages(&[message]);
        let value = to_value(&out);
        assert_eq!(value[0]["content"][0]["input"], json!({}));
    }

    #[test]
    fn translate_tools_maps_openai_shape_to_anthropic() {
        let tools = vec![json!({
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read a file",
                "parameters": {"type": "object", "properties": {"path": {"type": "string"}}}
            }
        })];
        let translated = translate_tools(&tools);
        assert_eq!(translated.len(), 1);
        let tool = &translated[0];
        assert_eq!(tool["name"], "read_file");
        assert_eq!(tool["description"], "Read a file");
        assert_eq!(tool["input_schema"]["type"], "object");
        assert_eq!(tool["input_schema"]["properties"]["path"]["type"], "string");
        assert!(
            tool.get("type").is_none(),
            "the OpenAI wrapper must be gone"
        );
        assert!(tool.get("function").is_none());
    }

    #[test]
    fn translate_tools_defaults_missing_parameters_to_object_schema() {
        let tools = vec![json!({"type": "function", "function": {"name": "noop"}})];
        let translated = translate_tools(&tools);
        assert_eq!(translated[0]["input_schema"], json!({"type": "object"}));
    }
}
