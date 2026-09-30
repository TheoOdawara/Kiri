/// Where an extension resource was discovered (ADRs 0019/0012/0028). `Global` (`~/.kiri/`) and `Bundled`
/// (compiled in) are trusted; `Project` (`<workspace>/.kiri/`) is untrusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Global,
    Project,
    Bundled,
}

impl Layer {
    pub fn label(self) -> &'static str {
        match self {
            Layer::Global => "global",
            Layer::Project => "project",
            Layer::Bundled => "bundled",
        }
    }

    /// Lowest value = highest precedence. The single source for every layer-order sort, so a
    /// HashMap-sourced resource list renders in a stable order.
    pub fn precedence(self) -> u8 {
        match self {
            Layer::Global => 0,
            Layer::Project => 1,
            Layer::Bundled => 2,
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
        assert_eq!(Layer::Bundled.label(), "bundled");
    }
}
