/// Whether an incoming entry should replace the existing one, given their `updated_at` timestamps.
/// Last-write-wins by RFC3339 UTC timestamp: same-format `…Z` timestamps sort lexicographically in
/// chronological order, so a plain string comparison is correct and avoids parsing. Ties keep the
/// existing entry (the incoming is not strictly newer), making a re-pull idempotent.
pub fn incoming_wins(incoming_updated_at: &str, existing_updated_at: &str) -> bool {
    incoming_updated_at > existing_updated_at
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
}
