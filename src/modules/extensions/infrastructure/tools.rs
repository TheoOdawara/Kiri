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
