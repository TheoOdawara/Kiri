/// The layer an extension resource was discovered in, mirroring the two-layer model used for instructions
/// (ADR 0019) and config (ADR 0012). `Global` is the trusted `~/.kiri/` layer; `Project` is the untrusted
/// `<workspace>/.kiri/` layer. Pure data only — no path knowledge, no I/O.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Global,
    Project,
}

impl Layer {
    /// The short label used in the `/rules` display and boot notices.
    pub fn label(self) -> &'static str {
        match self {
            Layer::Global => "global",
            Layer::Project => "project",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_labels() {
        assert_eq!(Layer::Global.label(), "global");
        assert_eq!(Layer::Project.label(), "project");
    }
}
