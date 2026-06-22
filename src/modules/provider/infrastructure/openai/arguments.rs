use std::borrow::Cow;

use serde_json::Value;

/// Escape raw control characters that appear INSIDE a JSON string value, leaving control characters
/// BETWEEN tokens (structural whitespace) untouched.
///
/// The model sometimes emits a literal newline/tab inside a string value (e.g. a file's `content`)
/// instead of the JSON escape `\n`/`\t` — that is invalid JSON. The provider re-parses
/// `function.arguments` as nested JSON in strict mode and rejects such a body with "Invalid control
/// character", which then poisons every later request (the malformed turn is re-sent verbatim).
/// Escaping only the control chars inside string values keeps the JSON structure intact and the
/// content faithful (a real newline becomes `\n`).
///
/// Fast path: returns the input borrowed and untouched when it contains no control byte (`< 0x20`).
/// A raw-byte scan is correct because control bytes never appear as a UTF-8 continuation byte
/// (those are always `>= 0x80`).
pub(crate) fn escape_control_chars_in_strings(args: &str) -> Cow<'_, str> {
    if !args.bytes().any(|byte| byte < 0x20) {
        return Cow::Borrowed(args);
    }

    let mut out = Vec::with_capacity(args.len() + 16);
    let mut in_string = false;
    let mut escaped = false;
    for &byte in args.as_bytes() {
        if in_string && !escaped && byte == b'\\' {
            out.push(byte);
            escaped = true;
            continue;
        }
        if escaped {
            // Previous byte was a backslash inside a string. A raw control char here is still illegal
            // JSON and must be escaped; anything else is a legitimate escape body (`\"`, `\n`, ...).
            escaped = false;
            if byte >= 0x20 {
                out.push(byte);
            } else {
                push_escaped_control(&mut out, byte);
            }
            continue;
        }
        if byte == b'"' {
            in_string = !in_string;
            out.push(byte);
            continue;
        }
        if in_string && byte < 0x20 {
            push_escaped_control(&mut out, byte);
            continue;
        }
        out.push(byte);
    }

    // WHY the expect cannot fire: the input was a valid `&str` and we only ever inject ASCII escape
    // sequences, so the assembled bytes are always valid UTF-8.
    Cow::Owned(String::from_utf8(out).expect("valid UTF-8: str input, ASCII-only insertions"))
}

fn push_escaped_control(out: &mut Vec<u8>, byte: u8) {
    match byte {
        b'\n' => out.extend_from_slice(b"\\n"),
        b'\t' => out.extend_from_slice(b"\\t"),
        b'\r' => out.extend_from_slice(b"\\r"),
        other => out.extend_from_slice(format!("\\u{other:04x}").as_bytes()),
    }
}

/// Normalize a tool call's `arguments` so the stored value is always valid JSON. Escapes raw control
/// chars inside string values; if the result is still not valid JSON (e.g. a truncated/garbled
/// stream), falls back to `"{}"` so the turn can never poison a later request — the tool then reports
/// invalid arguments and the model recovers on its own.
pub(crate) fn normalize_arguments(args: String) -> String {
    let escaped = escape_to_owned(args);
    if serde_json::from_str::<Value>(&escaped).is_ok() {
        escaped
    } else {
        "{}".to_string()
    }
}

/// Apply the escaper without cloning on the clean (borrowed) path.
fn escape_to_owned(args: String) -> String {
    if let Cow::Owned(owned) = escape_control_chars_in_strings(&args) {
        return owned;
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Value {
        serde_json::from_str(text).expect("valid JSON")
    }

    #[test]
    fn raw_newline_inside_string_is_escaped_and_content_preserved() {
        let bad = "{\"path\":\"a.rs\",\"content\":\"line1\nline2\"}"; // literal 0x0A inside string
        let value = parse(&escape_control_chars_in_strings(bad));
        assert_eq!(value["path"], "a.rs");
        assert_eq!(value["content"], "line1\nline2");
    }

    #[test]
    fn already_valid_arguments_take_the_fast_path() {
        let good = r#"{"path":"a.rs","content":"line1\nline2"}"#;
        assert!(matches!(
            escape_control_chars_in_strings(good),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn structural_whitespace_between_tokens_is_preserved() {
        let pretty = "{\n  \"path\": \"a.rs\"\n}"; // raw newlines OUTSIDE any string
        let result = escape_control_chars_in_strings(pretty);
        assert_eq!(result.as_ref(), pretty);
        let _ = parse(&result);
    }

    #[test]
    fn escaped_quote_inside_string_does_not_close_it() {
        let raw = "{\"content\":\"say \\\"hi\\\"\nbye\"}"; // \" ... raw newline ... still in string
        let value = parse(&escape_control_chars_in_strings(raw));
        assert_eq!(value["content"], "say \"hi\"\nbye");
    }

    #[test]
    fn tab_cr_and_other_control_chars_are_escaped() {
        let raw = "{\"k\":\"a\tb\rc\u{0b}d\"}"; // tab, CR, vertical tab inside the string value
        let value = parse(&escape_control_chars_in_strings(raw));
        assert_eq!(value["k"], "a\tb\rc\u{0b}d");
    }

    #[test]
    fn normalize_keeps_recoverable_arguments() {
        let good = r#"{"path":"a.rs"}"#.to_string();
        assert_eq!(normalize_arguments(good.clone()), good);
    }

    #[test]
    fn normalize_falls_back_to_empty_object_when_unrecoverable() {
        // Control char forces the slow path, but the JSON is truncated (unterminated string).
        let truncated = "{\"path\":\"a.rs\",\"content\":\"oops\nno close".to_string();
        assert_eq!(normalize_arguments(truncated), "{}");
    }
}
