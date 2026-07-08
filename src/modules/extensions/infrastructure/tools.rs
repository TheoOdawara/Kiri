pub mod use_skill;

use std::collections::HashMap;
use std::sync::Arc;

use crate::modules::extensions::domain::resource::Skill;
use crate::modules::tools::application::tool::Tool;

/// The extensions tool set advertised to the model: fetch a loaded skill's body on demand. Wired
/// alongside the memory/docs tools in `app::wire`.
pub fn default_extension_tools(skills: Arc<HashMap<String, Skill>>) -> Vec<Arc<dyn Tool>> {
    vec![Arc::new(use_skill::UseSkill::new(skills))]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_extension_tools_are_exactly_the_known_safe_set() {
        // SEC-01 / ADR 0029 guard (see `tools/infrastructure/fs.rs`): lock the read-only surface a
        // headless subagent can hold. `use_skill` only reads a harness-loaded skill body — no fs path —
        // so it is safe. A new read-only extension tool must be reviewed against SEC-01 before it lands
        // here.
        let tools = default_extension_tools(Arc::new(HashMap::new()));
        let read_only: Vec<&str> = tools
            .iter()
            .filter(|tool| tool.is_read_only())
            .map(|tool| tool.name())
            .collect();
        assert_eq!(read_only, ["use_skill"]);
    }
}
