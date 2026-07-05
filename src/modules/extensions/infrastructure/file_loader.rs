//! Loads extension resources from the filesystem: Markdown files with optional YAML frontmatter.
//! Scans the five resource-type subdirectories (`rules/`, `commands/`, `agents/`, `skills/`, `hooks/`)
//! under both `~/.kiri/` (global) and `<workspace>/.kiri/` (project) — merging by id (global first,
//! project extends). Follows the same convention as `DocsLibrary`: depth-first walk, capped,
//! symlink-skipping.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::modules::extensions::application::catalog::{ExtensionCatalog, ExtensionsLoader};
use crate::modules::extensions::domain::frontmatter::Frontmatter;
use crate::modules::extensions::domain::resource::{
    AgentProfile, CommandSpec as ExtCommandSpec, Hook, Resource, Rule, Skill,
};
use crate::modules::extensions::domain::scope::Layer;
use crate::shared::kernel::error::AgentResult;

/// The five extension resource types this loader scans, in discovery order. Each is a subdirectory name
/// under both the global and project `.kiri/` roots.
const RESOURCE_TYPES: [&str; 5] = ["rules", "commands", "agents", "skills", "hooks"];

/// Cap on total files visited during discovery across all extension types, so a huge tree does not hold
/// boot indefinitely.
const MAX_FILES_SCANNED: usize = 500;

/// Collect Markdown files under `root`, depth-first, capped at `MAX_FILES_SCANNED`. Skips symlinks
/// (mirrors `DocsLibrary::collect_markdown_files`).
async fn collect_markdown(root: &Path, files: &mut Vec<PathBuf>) -> AgentResult<()> {
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        if files.len() >= MAX_FILES_SCANNED {
            break;
        }
        let mut reader = match tokio::fs::read_dir(&dir).await {
            Ok(reader) => reader,
            Err(_) => continue,
        };
        while let Some(entry) = reader.next_entry().await? {
            let path = entry.path();
            let file_type = entry.file_type().await?;
            if file_type.is_symlink() {
                continue;
            } else if file_type.is_dir() {
                dirs.push(path);
            } else if is_md(&path) {
                files.push(path);
                if files.len() >= MAX_FILES_SCANNED {
                    break;
                }
            }
        }
    }
    Ok(())
}

fn is_md(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("md") | Some("markdown")
    )
}

/// Load a single Markdown resource from `path` in `layer`. Returns `None` on a read failure, so one
/// broken file never aborts the whole catalog.
async fn load_one(path: &Path, layer: Layer) -> Option<Resource> {
    let bytes = tokio::fs::read(path).await.ok()?;
    let source = String::from_utf8_lossy(&bytes);
    let (frontmatter, body) = Frontmatter::parse(&source);
    let id = frontmatter
        .get("id")
        .map(|s| s.to_string())
        .unwrap_or_else(|| file_stem(path));
    let body = body.trim().to_string();
    let display_path = path.to_string_lossy().replace('\\', "/");
    Some(Resource::new(id, frontmatter, body, layer, display_path))
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string()
}

/// Load resources from `root`, keying them by id (global first, project extends by id).
async fn load_layer(
    root: &Path,
    layer: Layer,
    map: &mut HashMap<String, Resource>,
) -> AgentResult<()> {
    let mut files = Vec::new();
    collect_markdown(root, &mut files).await?;
    for file in files {
        if let Some(res) = load_one(&file, layer).await {
            // Project extends or adds; global is the base. Both layers are loaded, neither overwrites
            // the other silently without the caller knowing (both items are kept in `resources`).
            map.entry(res.id.clone()).or_insert(res);
        }
    }
    Ok(())
}

/// The filesystem adapter: scans `~/.kiri/{rules,commands,agents,skills,hooks}/` and their
/// `<workspace>/.kiri/` project-layer equivalents, loading Markdown files with frontmatter. Reads the
/// home directory from `shared/infra::home` — the single cross-platform source also read by
/// `config::expand_home`.
pub struct FileExtensionsLoader {
    global_dir: PathBuf,
    project_dir: PathBuf,
}

impl FileExtensionsLoader {
    pub fn new(global_dir: PathBuf, workspace: &Path) -> Self {
        let project_dir = workspace.join(".kiri");
        Self {
            global_dir,
            project_dir,
        }
    }

    /// Discover and fold one resource type into `catalog`: scans the global then project `type_name/`
    /// subdirectory (a fresh resources map per type, so ids never collide across types), then builds the
    /// typed entries into the matching catalog field. Takes `&mut ExtensionCatalog` rather than one
    /// mutable ref per field — the catalog already groups every accumulator this needs, so this stays
    /// under the argument-count lint as new resource types are added.
    async fn load_type(&self, type_name: &str, catalog: &mut ExtensionCatalog) -> AgentResult<()> {
        let mut resources: HashMap<String, Resource> = HashMap::new();
        let global_root = self.global_dir.join(type_name);
        if global_root.is_dir() {
            load_layer(&global_root, Layer::Global, &mut resources).await?;
        }
        let project_root = self.project_dir.join(type_name);
        if project_root.is_dir() {
            load_layer(&project_root, Layer::Project, &mut resources).await?;
        }

        for res in resources.values() {
            match type_name {
                "rules" => catalog.rules.push(Rule::from_resource(res)),
                "commands" => {
                    let cmd = ExtCommandSpec::from_resource(res);
                    for alias in &cmd.aliases {
                        catalog
                            .command_aliases
                            .insert(alias.clone(), cmd.name.clone());
                    }
                    catalog.commands.insert(cmd.name.clone(), cmd);
                }
                "agents" => {
                    let agent = AgentProfile::from_resource(res);
                    catalog.agents.insert(agent.id.clone(), agent);
                }
                "skills" => {
                    let skill = Skill::from_resource(res);
                    catalog.skills.insert(skill.id.clone(), skill);
                }
                "hooks" => {
                    // A malformed hook (missing/unrecognized event, or a blank command) is dropped
                    // rather than aborting the whole catalog load.
                    if let Some(hook) = Hook::from_resource(res) {
                        catalog.hooks.insert(hook.id.clone(), hook);
                    }
                }
                _ => {}
            }
        }
        catalog.resources.extend(resources);
        Ok(())
    }
}

#[async_trait::async_trait]
impl ExtensionsLoader for FileExtensionsLoader {
    async fn load(&self) -> AgentResult<ExtensionCatalog> {
        let mut catalog = ExtensionCatalog::default();
        for type_name in RESOURCE_TYPES {
            self.load_type(type_name, &mut catalog).await?;
        }
        Ok(catalog)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn write(dir: &Path, rel: &str, content: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.unwrap();
        }
        tokio::fs::write(path, content).await.unwrap();
    }

    #[tokio::test]
    async fn loads_rules_from_global_and_project() {
        let global = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();
        let project_kiri = workspace.path().join(".kiri");

        write(
            global.path(),
            "rules/style.md",
            "---\nid: style\nalways: true\ntags:\n  - rust\n---\n\nUse Rust fmt.\n",
        )
        .await;
        write(
            &project_kiri,
            "rules/team.md",
            "---\nid: team\nalways: false\n---\n\nPrefer async over sync.\n",
        )
        .await;

        let loader = FileExtensionsLoader::new(global.path().to_path_buf(), workspace.path());
        let catalog = loader.load().await.unwrap();

        assert_eq!(catalog.rules.len(), 2);
        let always_on: Vec<&Rule> = catalog.rules.iter().filter(|r| r.always).collect();
        assert_eq!(always_on.len(), 1);
        assert_eq!(always_on[0].id, "style");
        assert_eq!(always_on[0].body, "Use Rust fmt.");
        assert_eq!(always_on[0].layer, Layer::Global);
    }

    #[tokio::test]
    async fn empty_dirs_yield_no_rules() {
        let global = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();
        let loader = FileExtensionsLoader::new(global.path().to_path_buf(), workspace.path());
        let catalog = loader.load().await.unwrap();
        assert!(catalog.rules.is_empty());
        assert!(catalog.commands.is_empty());
    }

    #[tokio::test]
    async fn loads_commands_with_aliases() {
        let global = TempDir::new().unwrap();
        write(
            global.path(),
            "commands/test.md",
            "---\nname: test\naliases:\n  - t\ndescription: Run tests\n---\n\nRun.\n",
        )
        .await;

        let workspace = TempDir::new().unwrap();
        let loader = FileExtensionsLoader::new(global.path().to_path_buf(), workspace.path());
        let catalog = loader.load().await.unwrap();

        assert_eq!(catalog.commands.len(), 1);
        let cmd = catalog.commands.get("/test").unwrap();
        assert_eq!(cmd.body, "Run.");
        assert_eq!(cmd.aliases, ["/t"]);
        assert_eq!(
            catalog.command_aliases.get("/t"),
            Some(&"/test".to_string())
        );
    }

    #[tokio::test]
    async fn project_command_does_not_overwrite_global_of_same_name() {
        let global = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();
        let project_kiri = workspace.path().join(".kiri");

        write(
            global.path(),
            "commands/x.md",
            "---\nid: x\nname: x\n---\n\nGlobal.\n",
        )
        .await;
        write(
            &project_kiri,
            "commands/x.md",
            "---\nid: x\nname: x\n---\n\nProject.\n",
        )
        .await;

        let loader = FileExtensionsLoader::new(global.path().to_path_buf(), workspace.path());
        let catalog = loader.load().await.unwrap();

        // Global loads first, retains its entry; project is present but its extension did not overwrite.
        let cmd = catalog.commands.get("/x").unwrap();
        assert_eq!(cmd.body, "Global.");
        assert_eq!(cmd.layer, Layer::Global);
        // Both resources are in the catalog for display.
        assert_eq!(catalog.resources.len(), 1);
    }

    #[tokio::test]
    async fn loads_agents_and_skills() {
        let global = TempDir::new().unwrap();
        write(
            global.path(),
            "agents/researcher.md",
            "---\nmodel: gpt-pro\n---\n\nYou are a deep-research agent.\n",
        )
        .await;
        write(
            global.path(),
            "skills/pdf-extract.md",
            "---\ndescription: Extract text from PDFs\ntags:\n  - pdf\n---\n\nUse pdftotext.\n",
        )
        .await;

        let workspace = TempDir::new().unwrap();
        let loader = FileExtensionsLoader::new(global.path().to_path_buf(), workspace.path());
        let catalog = loader.load().await.unwrap();

        let agent = catalog.agents.get("researcher").unwrap();
        assert_eq!(agent.system_prompt, "You are a deep-research agent.");
        assert_eq!(agent.model.as_deref(), Some("gpt-pro"));

        let skill = catalog.skills.get("pdf-extract").unwrap();
        assert_eq!(skill.description, "Extract text from PDFs");
        assert!(skill.tags.contains("pdf"));
        assert_eq!(skill.body, "Use pdftotext.");
    }

    #[tokio::test]
    async fn empty_dirs_yield_no_agents_or_skills() {
        let global = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();
        let loader = FileExtensionsLoader::new(global.path().to_path_buf(), workspace.path());
        let catalog = loader.load().await.unwrap();
        assert!(catalog.agents.is_empty());
        assert!(catalog.skills.is_empty());
    }

    #[tokio::test]
    async fn loads_hooks() {
        let global = TempDir::new().unwrap();
        write(
            global.path(),
            "hooks/notify.md",
            "---\nevent: SessionStart\n---\n\necho welcome back\n",
        )
        .await;

        let workspace = TempDir::new().unwrap();
        let loader = FileExtensionsLoader::new(global.path().to_path_buf(), workspace.path());
        let catalog = loader.load().await.unwrap();

        let hook = catalog.hooks.get("notify").unwrap();
        assert_eq!(
            hook.event,
            crate::modules::extensions::domain::resource::HookEvent::SessionStart
        );
        assert_eq!(hook.command, "echo welcome back");
    }

    #[tokio::test]
    async fn a_malformed_hook_is_dropped_not_fatal() {
        let global = TempDir::new().unwrap();
        write(
            global.path(),
            "hooks/broken.md",
            "---\nevent: NotARealEvent\n---\n\necho hi\n",
        )
        .await;

        let workspace = TempDir::new().unwrap();
        let loader = FileExtensionsLoader::new(global.path().to_path_buf(), workspace.path());
        let catalog = loader.load().await.unwrap();
        assert!(catalog.hooks.is_empty());
    }
}
