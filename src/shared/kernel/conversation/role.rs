/// The author of a message. Serde-free (ADR 0003): [`Role::as_wire_str`]/[`Role::from_wire_str`] are the
/// single source of each wire spelling, shared by the provider DTO and the SQLite session store so the two
/// can never drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl Role {
    /// The one place a `Role` becomes its protocol spelling.
    pub const fn as_wire_str(self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }

    /// An unknown string maps to `None`, so a stored row corrupted by an external tool is skipped rather
    /// than panicking.
    pub fn from_wire_str(s: &str) -> Option<Role> {
        match s {
            "system" => Some(Role::System),
            "user" => Some(Role::User),
            "assistant" => Some(Role::Assistant),
            "tool" => Some(Role::Tool),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Role; 4] = [Role::System, Role::User, Role::Assistant, Role::Tool];

    #[test]
    fn as_wire_str_maps_all_four_variants() {
        assert_eq!(Role::System.as_wire_str(), "system");
        assert_eq!(Role::User.as_wire_str(), "user");
        assert_eq!(Role::Assistant.as_wire_str(), "assistant");
        assert_eq!(Role::Tool.as_wire_str(), "tool");
    }

    #[test]
    fn from_wire_str_round_trips_all_four() {
        assert_eq!(Role::from_wire_str("system"), Some(Role::System));
        assert_eq!(Role::from_wire_str("user"), Some(Role::User));
        assert_eq!(Role::from_wire_str("assistant"), Some(Role::Assistant));
        assert_eq!(Role::from_wire_str("tool"), Some(Role::Tool));
    }

    #[test]
    fn from_wire_str_rejects_unknown_role() {
        assert_eq!(Role::from_wire_str("bogus"), None);
    }

    #[test]
    fn wire_round_trip_is_identity() {
        for role in ALL {
            assert_eq!(Role::from_wire_str(role.as_wire_str()), Some(role));
        }
    }
}
