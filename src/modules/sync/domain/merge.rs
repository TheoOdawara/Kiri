use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// Whether an incoming entry should replace the existing one, given their `updated_at` timestamps.
/// Last-write-wins by parsed instant: the RFC3339 timestamps `time` produces are variable-width (it
/// omits or trims the sub-second fraction), so a plain string compare is WRONG — e.g.
/// `"…00.5Z" > "…00Z"` is false lexicographically though `.5s` is strictly later. We therefore parse
/// both and compare the instants. If either fails to parse (timestamps are always machine-generated,
/// so this is defensive), fall back to the string compare. Ties keep the existing entry (the incoming
/// is not strictly newer), making a re-pull idempotent.
pub fn incoming_wins(incoming_updated_at: &str, existing_updated_at: &str) -> bool {
    match (
        OffsetDateTime::parse(incoming_updated_at, &Rfc3339),
        OffsetDateTime::parse(existing_updated_at, &Rfc3339),
    ) {
        (Ok(incoming), Ok(existing)) => incoming > existing,
        _ => incoming_updated_at > existing_updated_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_incoming_wins() {
        assert!(incoming_wins(
            "2026-06-26T12:00:01Z",
            "2026-06-26T12:00:00Z"
        ));
    }

    #[test]
    fn older_or_equal_incoming_loses() {
        assert!(!incoming_wins(
            "2026-06-26T12:00:00Z",
            "2026-06-26T12:00:01Z"
        ));
        assert!(!incoming_wins(
            "2026-06-26T12:00:00Z",
            "2026-06-26T12:00:00Z"
        ));
    }

    #[test]
    fn variable_width_fractions_compare_chronologically() {
        // These all FAIL under a lexicographic string compare but are correct by instant: `.` (0x2E)
        // and a longer fraction's next digit sort below `Z` (0x5A).
        assert!(incoming_wins(
            "2026-06-26T12:00:00.5Z",
            "2026-06-26T12:00:00Z"
        ));
        assert!(incoming_wins(
            "2026-06-26T12:00:00.500001Z",
            "2026-06-26T12:00:00.5Z"
        ));
        assert!(!incoming_wins(
            "2026-06-26T12:00:00Z",
            "2026-06-26T12:00:00.5Z"
        ));
    }
}
