use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// Compares parsed instants, never the strings: `time` emits variable-width RFC3339, so `"…00.5Z"`
/// sorts *below* `"…00Z"` lexicographically though it is strictly later. An unparseable timestamp
/// fails closed and keeps the existing entry, so a hand-crafted `updated_at` cannot overwrite it.
/// A tie also keeps the existing entry, which makes a re-pull idempotent.
pub fn incoming_wins(incoming_updated_at: &str, existing_updated_at: &str) -> bool {
    match (
        OffsetDateTime::parse(incoming_updated_at, &Rfc3339),
        OffsetDateTime::parse(existing_updated_at, &Rfc3339),
    ) {
        (Ok(incoming), Ok(existing)) => incoming > existing,
        _ => false,
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
        // Each of these fails under a string compare: `.` (0x2E) and digits sort below `Z` (0x5A).
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

    #[test]
    fn non_rfc3339_incoming_does_not_overwrite() {
        // SYNC-08 regression: `"zzzz" > "2026-…"` under the old lexicographic fallback.
        assert!(!incoming_wins("zzzz", "2026-06-26T12:00:00Z"));
        assert!(!incoming_wins("nope", "also-bad"));
    }
}
