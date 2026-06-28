use std::borrow::Cow;

use crate::modules::provider::infrastructure::tool_args;

/// ASCII space — the first printable character. Every byte below it (newline, tab, NUL, ...) is a
/// control character that strict JSON forbids unescaped inside a string value. Naming the boundary
/// keeps the intent of every comparison explicit instead of a bare `0x20`.
const FIRST_PRINTABLE_ASCII: u8 = 0x20;

fn is_json_control(byte: u8) -> bool {
    byte < FIRST_PRINTABLE_ASCII
}

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
/// Fast path: returns the input borrowed and untouched when it holds no control character.
/// A raw-byte scan is correct because control bytes are pure ASCII and never appear as a UTF-8
/// continuation byte (those are always >= 0x80). The result is built by copying verbatim runs of the
/// original `&str` (always valid UTF-8) and splicing in ASCII escape sequences, so the output is
/// valid UTF-8 by construction — no fallible reassembly is needed.
pub(crate) fn escape_control_chars_in_strings(args: &str) -> Cow<'_, str> {
    if !args.bytes().any(is_json_control) {
        return Cow::Borrowed(args);
    }

    let mut out = String::with_capacity(args.len() + 16);
    let mut run_start = 0;
    let mut in_string = false;
    let mut escaped = false;
    for (index, &byte) in args.as_bytes().iter().enumerate() {
        if escaped {
            // Previous byte was a backslash inside a string. A raw control char here is still illegal
            // JSON and must be escaped; anything else is a legitimate escape body (`\"`, `\n`, ...)
            // that stays in the verbatim run.
            escaped = false;
            if is_json_control(byte) {
                out.push_str(&args[run_start..index]);
                push_escaped_control(&mut out, byte);
                run_start = index + 1;
            }
            continue;
        }
        if in_string && byte == b'\\' {
            escaped = true;
            continue;
        }
        if byte == b'"' {
            in_string = !in_string;
            continue;
        }
        if in_string && is_json_control(byte) {
            out.push_str(&args[run_start..index]);
            push_escaped_control(&mut out, byte);
            run_start = index + 1;
        }
    }
    out.push_str(&args[run_start..]);
    Cow::Owned(out)
}

fn push_escaped_control(out: &mut String, byte: u8) {
    match byte {
        b'\n' => out.push_str("\\n"),
        b'\t' => out.push_str("\\t"),
        b'\r' => out.push_str("\\r"),
        other => out.push_str(&format!("\\u{other:04x}")),
    }
}

/// Normalize a tool call's `arguments` so the stored value is always valid JSON. Applies OpenAI's
/// distinct control-char escaping first, then delegates the validate-or-`{}` decision to the shared
/// tool-args rule — so the escaping stays local while the fallback policy is single-sourced.
pub(crate) fn normalize_arguments(args: String) -> String {
    let escaped = escape_to_owned(args);
    tool_args::sanitized_string(&escaped)
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
    use serde_json::Value;

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
