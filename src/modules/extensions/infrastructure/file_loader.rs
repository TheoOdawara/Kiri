//! Loads extension resources from the filesystem: Markdown files with optional YAML frontmatter.
//! Scans two well-known directory trees — `~/.kiri/rules/`/`commands/` (global) and
//! `<workspace>/.kiri/rules/`/`commands/` (project) — merging by id (global first, project extends).
//! Follows the same convention as `DocsLibrary`: depth-first walk, capped, symlink-skipping.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::modules::extensions::application::catalog::{ExtensionCatalog, ExtensionsLoader};
use crate::modules::extensions::domain::frontmatter::Frontmatter;
use crate::modules::extensions::domain::resource::{CommandSpec as ExtCommandSpec, Resource, Rule};
use crate::modules::extensions::domain::scope::Layer;
use crate::shared::kernel::error::AgentResult;

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

/// The filesystem adapter: scans `~/.kiri/rules/` and `<workspace>/.kiri/rules/` (plus `commands/`
/// equivalents for Phase 2), loading Markdown files with frontmatter. Reads the home directory from
/// `shared/infra::home` — the single cross-platform source also read by `config::expand_home`.
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

    async fn load_type(
        &self,
        type_name: &str,
        resources: &mut HashMap<String, Resource>,
        rules: &mut Vec<Rule>,
        commands: &mut HashMap<String, ExtCommandSpec>,
        command_aliases: &mut HashMap<String, String>,
    ) -> AgentResult<()> {
        let global_root = self.global_dir.join(type_name);
        if global_root.is_dir() {
            load_layer(&global_root, Layer::Global, resources).await?;
        }
        let project_root = self.project_dir.join(type_name);
        if project_root.is_dir() {
            load_layer(&project_root, Layer::Project, resources).await?;
        }

        for (_, res) in resources.iter() {
            match type_name {
                "rules" => {
                    rules.push(Rule::from_resource(res));
                }
                "commands" => {
                    let cmd = ExtCommandSpec::from_resource(res);
                    for alias in &cmd.aliases {
                        command_aliases.insert(alias.clone(), cmd.name.clone());
                    }
                    commands.insert(cmd.name.clone(), cmd);
                }
                _ => {}
            }
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl ExtensionsLoader for FileExtensionsLoader {
    async fn load(&self) -> AgentResult<ExtensionCatalog> {
        let mut resources: HashMap<String, Resource> = HashMap::new();
        let mut rules: Vec<Rule> = Vec::new();
        let mut commands: HashMap<String, ExtCommandSpec> = HashMap::new();
        let mut command_aliases: HashMap<String, String> = HashMap::new();

        // Rules first (global, then project), then commands — same order each type.
        self.load_type(
            "rules",
            &mut resources,
            &mut rules,
            &mut commands,
            &mut command_aliases,
        )
        .await?;
        // Commands use a fresh resources map so they don't share the rules' id namespace.
        let mut cmd_resources: HashMap<String, Resource> = HashMap::new();
        self.load_type(
            "commands",
            &mut cmd_resources,
            &mut rules,
            &mut commands,
            &mut command_aliases,
        )
        .await?;
        // Merge the two resource maps (rules + commands).
        resources.extend(cmd_resources);

        Ok(ExtensionCatalog {
            resources,
            rules,
            commands,
            command_aliases,
        })
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
}
