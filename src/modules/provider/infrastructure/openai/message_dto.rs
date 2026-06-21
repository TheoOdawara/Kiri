use std::borrow::Cow;

use serde::ser::SerializeSeq;
use serde::{Serialize, Serializer};

use super::arguments::escape_control_chars_in_strings;
use crate::modules::agent::domain::message::Message;
use crate::modules::agent::domain::role::Role;
use crate::shared::kernel::tool_call::ToolCall;

/// The OpenAI-compatible wire shape of a chat message, built from a domain `Message`. The provider's
/// serialization rules (omit empty content / tool_calls / tool_call_id) live here, keeping the domain
/// `Message` free of any wire concern — so a future provider with a different message shape only adds
/// its own DTO.
#[derive(Debug, Serialize)]
pub struct MessageDto<'a> {
    #[serde(serialize_with = "serialize_role")]
    pub role: Role,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<&'a str>,
    #[serde(
        skip_serializing_if = "<[_]>::is_empty",
        serialize_with = "serialize_tool_calls"
    )]
    pub tool_calls: &'a [ToolCall],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<&'a str>,
}

impl<'a> From<&'a Message> for MessageDto<'a> {
    fn from(message: &'a Message) -> Self {
        Self {
            role: message.role,
            content: message.content.as_deref(),
            tool_calls: &message.tool_calls,
            tool_call_id: message.tool_call_id.as_deref(),
        }
    }
}

/// The OpenAI wire string for a role — the single place the domain `Role` becomes its lowercase wire
/// form, so the domain enum stays serde-free.
const fn wire_role(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

fn serialize_role<S: Serializer>(role: &Role, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(wire_role(*role))
}

/// Serialize tool calls through the wire DTO, re-applying the control-char escaper to each call's
/// `arguments`. This is the send-side boundary guard: it guarantees the outgoing body never carries a
/// raw control char inside an `arguments` value, whatever produced the `ToolCall`. Already-valid
/// arguments take the escaper's borrowed fast path, so the normal case allocates nothing extra.
fn serialize_tool_calls<S: Serializer>(
    calls: &[ToolCall],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let mut seq = serializer.serialize_seq(Some(calls.len()))?;
    for call in calls {
        seq.serialize_element(&ToolCallDto {
            id: &call.id,
            kind: &call.kind,
            function: FunctionCallDto {
                name: &call.function.name,
                arguments: escape_control_chars_in_strings(&call.function.arguments),
            },
        })?;
    }
    seq.end()
}

/// The OpenAI wire shape of a tool call. Mirrors the kernel `ToolCall` but lets the send boundary
/// normalize `arguments` (kept here, not on the kernel type, since it is a wire concern).
#[derive(Debug, Serialize)]
struct ToolCallDto<'a> {
    id: &'a str,
    #[serde(rename = "type")]
    kind: &'a str,
    function: FunctionCallDto<'a>,
}

#[derive(Debug, Serialize)]
struct FunctionCallDto<'a> {
    name: &'a str,
    arguments: Cow<'a, str>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::kernel::tool_call::FunctionCall;

    #[test]
    fn wire_role_maps_all_variants() {
        assert_eq!(wire_role(Role::System), "system");
        assert_eq!(wire_role(Role::User), "user");
        assert_eq!(wire_role(Role::Assistant), "assistant");
        assert_eq!(wire_role(Role::Tool), "tool");
    }

    #[test]
    fn system_message_serializes_role_and_content() {
        let value = serde_json::to_value(MessageDto::from(&Message::system("be concise"))).unwrap();
        assert_eq!(value["role"], "system");
        assert_eq!(value["content"], "be concise");
        assert!(value.get("tool_calls").is_none());
    }

    #[test]
    fn assistant_text_serializes_content() {
        let value = serde_json::to_value(MessageDto::from(&Message::assistant_text("ok"))).unwrap();
        assert_eq!(value["role"], "assistant");
        assert_eq!(value["content"], "ok");
    }

    #[test]
    fn assistant_tool_calls_omits_content_and_includes_tool_calls() {
        let message = Message::assistant_tool_calls(
            None,
            vec![ToolCall {
                id: "c1".to_string(),
                kind: "function".to_string(),
                function: FunctionCall {
                    name: "read_file".to_string(),
                    arguments: r#"{"path":"a.txt"}"#.to_string(),
                },
            }],
        );
        let value = serde_json::to_value(MessageDto::from(&message)).unwrap();
        assert_eq!(value["role"], "assistant");
        assert!(value.get("content").is_none());
        assert_eq!(value["tool_calls"][0]["id"], "c1");
        assert_eq!(value["tool_calls"][0]["function"]["name"], "read_file");
    }

    #[test]
    fn tool_call_arguments_with_raw_control_char_serialize_as_valid_json() {
        // Send-boundary guard: even if a stored ToolCall carries a raw newline inside its arguments,
        // what goes on the wire must be valid JSON (so the provider's nested re-parse cannot 400).
        let message = Message::assistant_tool_calls(
            None,
            vec![ToolCall {
                id: "c1".to_string(),
                kind: "function".to_string(),
                function: FunctionCall {
                    name: "write_file".to_string(),
                    arguments: "{\"content\":\"a\nb\"}".to_string(), // literal 0x0A inside string
                },
            }],
        );
        let value = serde_json::to_value(MessageDto::from(&message)).unwrap();

        assert_eq!(value["tool_calls"][0]["type"], "function");
        let wire_args = value["tool_calls"][0]["function"]["arguments"]
            .as_str()
            .unwrap();
        let parsed: serde_json::Value =
            serde_json::from_str(wire_args).expect("wire arguments must be valid JSON");
        assert_eq!(parsed["content"], "a\nb");
    }

    #[test]
    fn tool_result_serializes_role_and_tool_call_id() {
        let value =
            serde_json::to_value(MessageDto::from(&Message::tool_result("c1", "ok"))).unwrap();
        assert_eq!(value["role"], "tool");
        assert_eq!(value["tool_call_id"], "c1");
        assert_eq!(value["content"], "ok");
        assert!(value.get("tool_calls").is_none());
    }
}
