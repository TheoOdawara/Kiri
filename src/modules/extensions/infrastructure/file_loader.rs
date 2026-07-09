//! Loads extension resources off disk, merging by id across the global, project, and bundled layers.
//! Walks like `DocsLibrary`: depth-first, capped, symlink-skipping.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::modules::extensions::application::catalog::{ExtensionCatalog, ExtensionsLoader};
use crate::modules::extensions::domain::frontmatter::Frontmatter;
use crate::modules::extensions::domain::resource::{
    AgentProfile, CommandSpec as ExtCommandSpec, Hook, McpServer, Resource, Rule, Skill,
};
use crate::modules::extensions::domain::scope::Layer;
use crate::modules::extensions::infrastructure::bundled::bundled_for;
use crate::shared::kernel::error::AgentResult;

/// Scanned in discovery order; each is a subdirectory under both `.kiri/` roots.
const RESOURCE_TYPES: [&str; 6] = ["rules", "commands", "agents", "skills", "hooks", "mcp"];

/// Across all extension types, so a huge tree cannot hold boot indefinitely.
const MAX_FILES_SCANNED: usize = 500;

/// Skips symlinks, so a resource tree cannot reach outside its root.
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

/// `None` on a read failure, so one broken file never aborts the whole catalog.
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
            // Both layers stay in `resources`: neither silently overwrites the other.
            map.entry(res.id.clone()).or_insert(res);
        }
    }
    Ok(())
}

/// Reads the home directory from `shared/infra::home`, the single cross-platform source `config` uses too.
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

    /// A fresh resources map per type, so ids never collide across types. Precedence is
    /// global > project > bundled, enforced by `or_insert` (first wins): a user file always overrides a
    /// bundled default of the same id.
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
        for res in bundled_for(type_name) {
            resources.entry(res.id.clone()).or_insert(res);
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
                    // Dropped rather than aborting the whole catalog load.
                    if let Some(hook) = Hook::from_resource(res) {
                        catalog.hooks.insert(hook.id.clone(), hook);
                    }
                }
                "mcp" => {
                    // Dropped rather than aborting the whole catalog load.
                    if let Some(server) = McpServer::from_resource(res) {
                        catalog.mcp_servers.insert(server.id.clone(), server);
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

        // 3, not 2: the bundled `ponytail` rule folds in alongside the two user-authored ones.
        assert_eq!(catalog.rules.len(), 3);
        let style = catalog.rules.iter().find(|r| r.id == "style").unwrap();
        assert!(style.always);
        assert_eq!(style.body, "Use Rust fmt.");
        assert_eq!(style.layer, Layer::Global);
        let team = catalog.rules.iter().find(|r| r.id == "team").unwrap();
        assert!(!team.always);
    }

    #[tokio::test]
    async fn empty_dirs_yield_only_the_bundled_ponytail_rule() {
        // Rules are never empty by default — the bundled `ponytail` rule is always on. Commands have no
        // bundled default, so they stay empty.
        let global = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();
        let loader = FileExtensionsLoader::new(global.path().to_path_buf(), workspace.path());
        let catalog = loader.load().await.unwrap();
        assert_eq!(catalog.rules.len(), 1);
        assert_eq!(catalog.rules[0].id, "ponytail");
        assert!(catalog.rules[0].always);
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

        // Global loads first and retains its entry; the project extension does not overwrite it.
        let cmd = catalog.commands.get("/x").unwrap();
        assert_eq!(cmd.body, "Global.");
        assert_eq!(cmd.layer, Layer::Global);
        // `resources` also holds the bundled agents/skills, so its total length is not asserted here.
        assert_eq!(
            catalog.resources.get("x").map(|r| r.layer),
            Some(Layer::Global)
        );
    }

    #[tokio::test]
    async fn loads_agents_and_skills() {
        let global = TempDir::new().unwrap();
        write(
            global.path(),
            "agents/researcher.md",
            "---\nname: Researcher\ndescription: Deep-research specialist.\nmodel: gpt-pro\n---\n\nYou are a deep-research agent.\n",
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
        assert_eq!(agent.name, "Researcher");
        assert_eq!(agent.description, "Deep-research specialist.");
        assert_eq!(agent.system_prompt, "You are a deep-research agent.");
        assert_eq!(agent.model.as_deref(), Some("gpt-pro"));

        let skill = catalog.skills.get("pdf-extract").unwrap();
        assert_eq!(skill.description, "Extract text from PDFs");
        assert!(skill.tags.contains("pdf"));
        assert_eq!(skill.body, "Use pdftotext.");
    }

    #[tokio::test]
    async fn empty_dirs_yield_only_the_bundled_defaults() {
        // With no user files, agents/skills are exactly the binary-shipped defaults — never empty, which
        // is the point of ADR 0028.
        let global = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();
        let loader = FileExtensionsLoader::new(global.path().to_path_buf(), workspace.path());
        let catalog = loader.load().await.unwrap();
        assert_eq!(catalog.agents.len(), 2);
        assert!(catalog.agents.contains_key("search"));
        assert!(catalog.agents.contains_key("planning"));
        assert_eq!(catalog.skills.len(), 8);
        for id in [
            "plano",
            "gh",
            "commit",
            "ponytail",
            "ponytail-review",
            "ponytail-audit",
            "ponytail-debt",
            "ponytail-gain",
        ] {
            assert!(
                catalog.skills.contains_key(id),
                "missing bundled skill {id}"
            );
        }
    }

    #[tokio::test]
    async fn user_global_skill_overrides_bundled_default_of_same_id() {
        let global = TempDir::new().unwrap();
        write(
            global.path(),
            "skills/plano.md",
            "---\ndescription: User override\n---\n\nUser body.\n",
        )
        .await;
        let workspace = TempDir::new().unwrap();
        let loader = FileExtensionsLoader::new(global.path().to_path_buf(), workspace.path());
        let catalog = loader.load().await.unwrap();

        let skill = catalog.skills.get("plano").unwrap();
        assert_eq!(skill.body, "User body.");
        assert_eq!(skill.layer, Layer::Global);
    }

    #[tokio::test]
    async fn project_skill_overrides_bundled_default_of_same_id() {
        let global = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();
        write(
            &workspace.path().join(".kiri"),
            "skills/plano.md",
            "---\ndescription: Project override\n---\n\nProject body.\n",
        )
        .await;
        let loader = FileExtensionsLoader::new(global.path().to_path_buf(), workspace.path());
        let catalog = loader.load().await.unwrap();

        let skill = catalog.skills.get("plano").unwrap();
        assert_eq!(skill.body, "Project body.");
        assert_eq!(skill.layer, Layer::Project);
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

    #[tokio::test]
    async fn loads_mcp_servers() {
        let global = TempDir::new().unwrap();
        write(
            global.path(),
            "mcp/filesystem.md",
            "---\ncommand: npx\nargs:\n  - -y\n  - server-fs\n---\n",
        )
        .await;

        let workspace = TempDir::new().unwrap();
        let loader = FileExtensionsLoader::new(global.path().to_path_buf(), workspace.path());
        let catalog = loader.load().await.unwrap();

        let server = catalog.mcp_servers.get("filesystem").unwrap();
        assert_eq!(server.command, "npx");
        assert_eq!(server.args, ["-y", "server-fs"]);
    }

    #[tokio::test]
    async fn a_malformed_mcp_server_is_dropped_not_fatal() {
        let global = TempDir::new().unwrap();
        write(global.path(), "mcp/broken.md", "---\n---\n").await;

        let workspace = TempDir::new().unwrap();
        let loader = FileExtensionsLoader::new(global.path().to_path_buf(), workspace.path());
        let catalog = loader.load().await.unwrap();
        assert!(catalog.mcp_servers.is_empty());
    }
}
