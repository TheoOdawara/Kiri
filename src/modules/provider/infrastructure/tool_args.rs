//! The single owner of the tool-arguments validation rule shared by the provider adapters: an empty or
//! non-JSON value falls back to an empty object so a garbled turn can never poison a later request. Kept
//! here once so the rule and its rationale are not re-typed per adapter.

use serde_json::{Value, json};

/// Sanitize a tool call's raw JSON-string arguments into the `Value` Anthropic's `tool_use.input` expects.
/// An empty or non-JSON input becomes `{}`; otherwise the parsed value.
pub(crate) fn sanitized_object(raw: &str) -> Value {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return json!({});
    }
    serde_json::from_str(trimmed).unwrap_or_else(|_| json!({}))
}

/// Sanitize a tool call's raw JSON-string arguments into the domain `arguments` string: return the
/// ORIGINAL `raw` (the exact bytes the model produced) when it is valid JSON, else `"{}"`. The validity
/// check runs on `raw` itself — NOT a trimmed copy — because Rust's `trim` strips Unicode whitespace JSON
/// does not (e.g. U+00A0), so validating a trimmed value while returning the raw could hand the provider
/// an invalid-JSON arguments string and re-poison a later request. `serde_json` already tolerates JSON's
/// own leading/trailing whitespace, so a valid payload with surrounding spaces still round-trips untouched.
pub(crate) fn sanitized_string(raw: &str) -> String {
    if serde_json::from_str::<Value>(raw).is_ok() {
        raw.to_string()
    } else {
        "{}".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitized_object_empty_input_is_empty_object() {
        assert_eq!(sanitized_object("   "), json!({}));
    }

    #[test]
    fn sanitized_object_non_json_is_empty_object() {
        assert_eq!(sanitized_object("{not json"), json!({}));
    }

    #[test]
    fn sanitized_object_parses_valid_json() {
        assert_eq!(
            sanitized_object(r#"{"path":"a.txt"}"#),
            json!({"path": "a.txt"})
        );
    }

    #[test]
    fn sanitized_string_keeps_valid_json_untrimmed() {
        // Surrounding whitespace around valid JSON must be preserved in the return (the no-behavior-change
        // rule: the bytes re-sent to the provider are the original ones).
        let raw = "  {\"path\":\"a.txt\"}  ";
        assert_eq!(sanitized_string(raw), raw);
    }

    #[test]
    fn sanitized_string_falls_back_for_garbled() {
        assert_eq!(sanitized_string("{\"p\":"), "{}");
    }

    #[test]
    fn sanitized_string_empty_is_brace_brace() {
        assert_eq!(sanitized_string("   "), "{}");
    }

    #[test]
    fn sanitized_string_rejects_a_non_json_whitespace_prefix() {
        // U+00A0 is whitespace to Rust's `trim` but NOT to JSON, so the returned bytes must be validated
        // as-is: a payload that is only "valid once trimmed" falls back to `{}` rather than being re-sent.
        assert_eq!(sanitized_string("\u{a0}{\"p\":\"a\"}"), "{}");
    }
}
