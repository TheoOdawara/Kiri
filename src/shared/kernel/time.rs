use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// The single source for the harness's "now as a string". It lives in `kernel`, not `infra`, so its
/// memory-**domain** caller gains no domain→infra edge; an ambient clock is an accepted kernel primitive.
///
/// Deliberately ignored: formatting a valid UTC instant cannot fail, and the empty fallback keeps this
/// path total without an `unwrap` (forbidden outside tests).
pub fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_rfc3339_parses_back_as_rfc3339() {
        let now = now_rfc3339();
        assert!(
            OffsetDateTime::parse(&now, &Rfc3339).is_ok(),
            "output must round-trip through the RFC3339 parser: {now}"
        );
    }

    #[test]
    fn now_rfc3339_is_utc() {
        let now = now_rfc3339();
        assert!(
            now.ends_with('Z') || now.ends_with("+00:00"),
            "the timestamp must carry a UTC marker: {now}"
        );
    }
}
