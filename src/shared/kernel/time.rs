use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// RFC3339 timestamp for "now", in UTC. The single source for the harness's "now as a string": the memory
/// domain and the session store both call this instead of each re-deriving it. Lives in `shared/kernel`
/// (not `shared/infra`) so the memory **domain** caller gains no domain→infra edge — reading an ambient
/// clock here is an accepted kernel primitive. Formatting a valid UTC instant cannot fail in practice; the
/// empty fallback keeps this runtime path total without an `unwrap` (forbidden outside tests).
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
