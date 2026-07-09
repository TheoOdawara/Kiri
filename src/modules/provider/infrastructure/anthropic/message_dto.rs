//! The Messages API departs from chat-completions in three ways this module bridges: `system` is a
//! top-level field, roles must strictly alternate user/assistant (so consecutive same-role messages are
//! merged), and tool calls/results are content blocks rather than their own message shape.

use serde::Serialize;
use serde_json::{Value, json};

use crate::modules::provider::infrastructure::tool_args;
use crate::shared::kernel::message::{Message, ThinkingBlock};
use crate::shared::kernel::role::Role;

/// Owned rather than borrowed: translation merges messages and parses tool inputs into JSON, and the
/// allocations are negligible against the network call this is assembled for.
#[derive(Debug, Serialize)]
pub(crate) struct AnthropicMessage {
    pub role: &'static str,
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ContentBlock {
    Thinking {
        thinking: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    RedactedThinking {
        data: String,
    },
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

#[derive(Debug, Serialize)]
pub(crate) struct ImageSource {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub media_type: String,
    pub data: String,
}

/// System messages are concatenated into the top-level text; every other message is merged into the
/// previous one when they share an Anthropic role.
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

/// The Messages API has no tool role, so `Tool` results ride in a `user` message.
fn anthropic_role(role: Role) -> &'static str {
    match role {
        Role::Assistant => "assistant",
        _ => "user",
    }
}

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
            let mut blocks = Vec::with_capacity(message.tool_calls.len() + 2);
            push_thinking(&mut blocks, message);
            if let Some(text) = message.content.as_deref().filter(|t| !t.is_empty()) {
                blocks.push(ContentBlock::Text {
                    text: text.to_string(),
                });
            }
            for call in &message.tool_calls {
                blocks.push(ContentBlock::ToolUse {
                    id: call.id.clone(),
                    name: call.function.name.clone(),
                    input: tool_args::sanitized_object(&call.function.arguments),
                });
            }
            blocks
        }
        Role::Assistant => {
            let mut blocks = Vec::new();
            push_thinking(&mut blocks, message);
            blocks.extend(text_and_images(message));
            blocks
        }
        _ => text_and_images(message),
    }
}

/// The Messages API requires a thinking block to LEAD an assistant turn's content array. This stays
/// correct only because `agent_loop` pushes exactly one assistant message per turn: a thinking block on
/// the second of two merged assistant messages would land after the first's content.
fn push_thinking(blocks: &mut Vec<ContentBlock>, message: &Message) {
    match &message.thinking {
        Some(ThinkingBlock::Visible { text, signature }) => blocks.push(ContentBlock::Thinking {
            thinking: text.clone(),
            signature: signature.clone(),
        }),
        Some(ThinkingBlock::Redacted { data }) => {
            blocks.push(ContentBlock::RedactedThinking { data: data.clone() })
        }
        None => {}
    }
}

/// A blank caption emits no text block; an unparseable image URL is skipped rather than sent malformed.
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

/// `None` for any shape other than `data:<media_type>;base64,<data>`, so the caller skips it.
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

/// A schema already in Anthropic shape (no `function` wrapper) passes through unchanged. A missing
/// `parameters` becomes an empty object schema, since the API requires a valid `input_schema`.
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
        // The Messages API rejects consecutive user messages, so parallel tool results must merge.
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

    #[test]
    fn thinking_block_leads_a_tool_use_turn() {
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
        )
        .with_thinking(ThinkingBlock::Visible {
            text: "I should read the file first".to_string(),
            signature: Some("sig-abc".to_string()),
        });
        let (_system, out) = build_messages(&[message]);
        let value = to_value(&out);
        let content = value[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(content[0]["thinking"], "I should read the file first");
        assert_eq!(content[0]["signature"], "sig-abc");
        assert_eq!(content[1]["type"], "text");
        assert_eq!(content[2]["type"], "tool_use");
    }

    #[test]
    fn redacted_thinking_block_leads_a_tool_use_turn() {
        let message = Message::assistant_tool_calls(
            Some("let me read it".to_string()),
            vec![ToolCall {
                id: "toolu_1".to_string(),
                kind: "function".to_string(),
                function: FunctionCall {
                    name: "read_file".to_string(),
                    arguments: "{}".to_string(),
                },
            }],
        )
        .with_thinking(ThinkingBlock::Redacted {
            data: "encrypted-blob".to_string(),
        });
        let (_system, out) = build_messages(&[message]);
        let value = to_value(&out);
        let content = value[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "redacted_thinking");
        assert_eq!(content[0]["data"], "encrypted-blob");
        assert!(content[0].get("thinking").is_none());
        assert_eq!(content[1]["type"], "text");
        assert_eq!(content[2]["type"], "tool_use");
    }

    #[test]
    fn thinking_block_leads_a_plain_text_turn_with_no_tool_calls() {
        let message =
            Message::assistant_text("the answer is 4").with_thinking(ThinkingBlock::Visible {
                text: "2 + 2".to_string(),
                signature: Some("sig-1".to_string()),
            });
        let (_system, out) = build_messages(&[message]);
        let value = to_value(&out);
        let content = value[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(content[1]["type"], "text");
        assert_eq!(content[1]["text"], "the answer is 4");
    }

    #[test]
    fn thinking_block_stays_first_even_when_a_following_assistant_message_merges_in() {
        // A misplaced thinking block is a hard 400, not a soft failure.
        let first = Message::assistant_tool_calls(
            None,
            vec![ToolCall {
                id: "toolu_1".to_string(),
                kind: "function".to_string(),
                function: FunctionCall {
                    name: "read_file".to_string(),
                    arguments: "{}".to_string(),
                },
            }],
        )
        .with_thinking(ThinkingBlock::Visible {
            text: "reasoning".to_string(),
            signature: Some("sig".to_string()),
        });
        let second = Message::assistant_text("more");
        let (_system, out) = build_messages(&[first, second]);
        assert_eq!(out.len(), 1, "consecutive assistant messages must merge");
        let value = to_value(&out);
        let content = value[0]["content"].as_array().unwrap();
        assert_eq!(
            content[0]["type"], "thinking",
            "the thinking block must stay first after the merge, got {content:?}"
        );
    }

    #[test]
    fn no_thinking_means_no_thinking_block() {
        let message = Message::assistant_text("hi");
        let (_system, out) = build_messages(&[message]);
        let value = to_value(&out);
        let content = value[0]["content"].as_array().unwrap();
        assert!(content.iter().all(|block| block["type"] != "thinking"));
    }
}
