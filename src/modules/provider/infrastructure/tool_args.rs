use serde_json::{Value, json};

/// An empty or non-JSON input becomes `{}`, so a garbled turn cannot poison a later request.
pub(crate) fn sanitized_object(raw: &str) -> Value {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return json!({});
    }
    serde_json::from_str(trimmed).unwrap_or_else(|_| json!({}))
}

/// Returns the exact bytes the model produced when they are valid JSON, else `"{}"`. The check runs on
/// `raw`, never a trimmed copy: Rust's `trim` strips Unicode whitespace that JSON does not (U+00A0), so
/// validating the trimmed value while returning the raw one would re-poison a later request.
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
        // U+00A0 is whitespace to Rust's `trim` but not to JSON.
        assert_eq!(sanitized_string("\u{a0}{\"p\":\"a\"}"), "{}");
    }
}
