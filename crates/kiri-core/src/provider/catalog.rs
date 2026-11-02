use serde_json::{Value, json};

#[derive(Clone, Copy)]
pub(crate) enum ThinkingFormat {
    ThinkingType,
    EnableThinking,
}

impl ThinkingFormat {
    pub(crate) fn body_entry(self, enabled: bool) -> (&'static str, Value) {
        match self {
            Self::ThinkingType => {
                let kind = if enabled { "enabled" } else { "disabled" };
                ("thinking", json!({"type": kind}))
            }
            Self::EnableThinking => ("enable_thinking", json!(enabled)),
        }
    }
}

pub(crate) struct CatalogEntry {
    ids: &'static [&'static str],
    pub(crate) efforts: &'static [&'static str],
    pub(crate) thinking: Option<ThinkingFormat>,
    pub(crate) replays_reasoning: bool,
}

pub(crate) const EFFORT_KEY: &str = "reasoning_effort";

static CATALOG: [CatalogEntry; 5] = [
    CatalogEntry {
        ids: &["deepseek-v4-pro", "deepseek-flash"],
        efforts: &["low", "high", "max"],
        thinking: Some(ThinkingFormat::ThinkingType),
        replays_reasoning: true,
    },
    CatalogEntry {
        ids: &["kimi-k3.1-pro"],
        efforts: &["low", "high", "max"],
        thinking: None,
        replays_reasoning: true,
    },
    CatalogEntry {
        ids: &["glm-5.3", "glm-5.3-flash"],
        efforts: &["low", "high", "max"],
        thinking: None,
        replays_reasoning: true,
    },
    CatalogEntry {
        ids: &["qwen3.8-max", "qwen3.8-flash"],
        efforts: &[],
        thinking: Some(ThinkingFormat::EnableThinking),
        replays_reasoning: false,
    },
    CatalogEntry {
        ids: &["gemini-4-argon", "gemini-3.8-flash"],
        efforts: &["low", "medium", "high"],
        thinking: None,
        replays_reasoning: false,
    },
];

pub(crate) fn find(model: &str) -> Option<&'static CatalogEntry> {
    CATALOG.iter().find(|entry| entry.ids.contains(&model))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_model_matches_only_by_its_exact_id() {
        assert!(find("glm-5.3").is_some());
        assert!(find("GLM-5.3").is_none());
        assert!(find("z-ai/glm-5.3").is_none());
    }
}
