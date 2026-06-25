use crate::modules::memory::domain::entry::{MemoryEntry, MemoryKind};
use crate::modules::memory::domain::project_memory::ProjectMemory;
use crate::shared::kernel::error::AgentError;
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use tokio::sync::RwLock;

type Result<T> = std::result::Result<T, AgentError>;

/// Map a serialization/format failure into the kernel's memory error variant.
fn mem<E: std::fmt::Display>(error: E) -> AgentError {
    AgentError::Memory(error.to_string())
}

/// Armazenamento de memória de projeto baseado em arquivos Markdown com front-matter YAML.
/// Estrutura:
///   .kiri/memory/
///   ├── index.json          # Índice: id -> { path, kind, tags, updated_at }
///   ├── architecture.md
///   ├── patterns.md
///   └── decisions/
///       └── 001-example.md
pub struct FileProjectMemory {
    root: PathBuf,
    index: Arc<RwLock<ProjectIndex>>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct ProjectIndex {
    entries: HashMap<String, IndexEntry>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct IndexEntry {
    path: String,
    kind: MemoryKind,
    tags: Vec<String>,
    updated_at: String,
}

impl FileProjectMemory {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            index: Arc::new(RwLock::new(ProjectIndex::default())),
        }
    }

    fn index_path(&self) -> PathBuf {
        self.root.join("index.json")
    }

    async fn load_index(&self) -> Result<()> {
        let path = self.index_path();
        if path.exists() {
            let content = fs::read_to_string(&path).await?;
            let index: ProjectIndex = serde_json::from_str(&content).map_err(mem)?;
            *self.index.write().await = index;
        }
        Ok(())
    }

    async fn save_index(&self) -> Result<()> {
        let index = self.index.read().await.clone();
        let content = serde_json::to_string_pretty(&index).map_err(mem)?;
        fs::write(self.index_path(), content).await?;
        Ok(())
    }

    fn entry_path(&self, kind: MemoryKind, id: &str) -> PathBuf {
        let filename = format!("{}-{}.md", kind.as_str(), &id[..8.min(id.len())]);
        match kind {
            MemoryKind::Decision => self.root.join("decisions").join(filename),
            _ => self.root.join(filename),
        }
    }

    fn ensure_dirs(&self) -> Result<()> {
        std::fs::create_dir_all(&self.root)?;
        std::fs::create_dir_all(self.root.join("decisions"))?;
        Ok(())
    }

    fn parse_markdown_file(&self, _path: &Path, content: &str) -> Result<MemoryEntry> {
        // Extrai front-matter YAML (entre --- e ---)
        let (front_matter, body) = match content.strip_prefix("---\n") {
            Some(after) => match after.find("\n---\n") {
                Some(end) => (Some(&after[..end]), &after[end + 5..]),
                None => (None, content),
            },
            None => (None, content),
        };

        let entry = if let Some(fm) = front_matter {
            serde_yaml::from_str(fm).map_err(mem)?
        } else {
            // Fallback for a file without front-matter: treat the whole body as a Fact.
            MemoryEntry::new(MemoryKind::Fact, body.to_string(), HashSet::new(), None)
        };

        Ok(entry)
    }

    fn render_markdown_file(&self, entry: &MemoryEntry) -> Result<String> {
        let front_matter = serde_yaml::to_string(entry).map_err(mem)?;
        Ok(format!("---\n{}---\n\n{}", front_matter, entry.content))
    }
}

#[async_trait]
impl ProjectMemory for FileProjectMemory {
    async fn init(&self) -> Result<()> {
        self.ensure_dirs()?;
        self.load_index().await?;
        Ok(())
    }

    async fn save(&self, entry: &MemoryEntry) -> Result<()> {
        let path = self.entry_path(entry.kind, &entry.id);
        let content = self.render_markdown_file(entry)?;

        // Garante que o diretório pai existe
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }

        fs::write(&path, content).await?;

        // Atualiza índice. The path is built by joining `root`, so stripping it back cannot fail; the
        // fallback to the full path keeps this total without an `unwrap`.
        let rel = path.strip_prefix(&self.root).unwrap_or(path.as_path());
        let mut index = self.index.write().await;
        index.entries.insert(
            entry.id.clone(),
            IndexEntry {
                path: rel.to_string_lossy().to_string(),
                kind: entry.kind,
                tags: entry.tags.iter().cloned().collect(),
                updated_at: entry.updated_at.clone(),
            },
        );
        drop(index);
        self.save_index().await?;
        Ok(())
    }

    async fn load(&self, id: &str) -> Result<Option<MemoryEntry>> {
        let index = self.index.read().await;
        let Some(index_entry) = index.entries.get(id) else {
            return Ok(None);
        };
        let path = self.root.join(&index_entry.path);
        drop(index);

        let content = fs::read_to_string(&path).await?;
        let entry = self.parse_markdown_file(&path, &content)?;
        Ok(Some(entry))
    }

    async fn delete(&self, id: &str) -> Result<bool> {
        let mut index = self.index.write().await;
        let Some(index_entry) = index.entries.remove(id) else {
            return Ok(false);
        };
        let path = self.root.join(&index_entry.path);
        drop(index);

        if path.exists() {
            fs::remove_file(&path).await?;
        }
        self.save_index().await?;
        Ok(true)
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>> {
        let index = self.index.read().await;
        let mut results = Vec::new();

        for index_entry in index.entries.values() {
            if results.len() >= limit {
                break;
            }
            let path = self.root.join(&index_entry.path);
            if let Ok(content) = fs::read_to_string(&path).await
                && let Ok(entry) = self.parse_markdown_file(&path, &content)
                && entry.matches_query(query)
            {
                results.push(entry);
            }
        }
        Ok(results)
    }

    async fn list(&self, offset: usize, limit: usize) -> Result<Vec<MemoryEntry>> {
        let index = self.index.read().await;
        let ids: Vec<String> = index.entries.keys().cloned().collect();
        drop(index);

        let mut results = Vec::new();
        for id in ids.into_iter().skip(offset).take(limit) {
            if let Some(entry) = self.load(&id).await? {
                results.push(entry);
            }
        }
        Ok(results)
    }

    async fn list_by_kind(&self, kind: MemoryKind, limit: usize) -> Result<Vec<MemoryEntry>> {
        let index = self.index.read().await;
        let ids: Vec<String> = index
            .entries
            .iter()
            .filter(|(_, e)| e.kind == kind)
            .map(|(id, _)| id.clone())
            .collect();
        drop(index);

        let mut results = Vec::new();
        for id in ids.into_iter().take(limit) {
            if let Some(entry) = self.load(&id).await? {
                results.push(entry);
            }
        }
        Ok(results)
    }

    async fn list_by_tag(&self, tag: &str, limit: usize) -> Result<Vec<MemoryEntry>> {
        let index = self.index.read().await;
        let ids: Vec<String> = index
            .entries
            .iter()
            .filter(|(_, e)| e.tags.iter().any(|t| t == tag))
            .map(|(id, _)| id.clone())
            .collect();
        drop(index);

        let mut results = Vec::new();
        for id in ids.into_iter().take(limit) {
            if let Some(entry) = self.load(&id).await? {
                results.push(entry);
            }
        }
        Ok(results)
    }

    async fn count(&self) -> Result<usize> {
        let index = self.index.read().await;
        Ok(index.entries.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::memory::domain::entry::{MemoryEntry, MemoryKind};
    use std::collections::HashSet;
    use tempfile::TempDir;

    #[tokio::test]
    async fn file_project_memory_save_and_load() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join(".kiri").join("memory");
        let memory = FileProjectMemory::new(root.clone());
        memory.init().await.unwrap();

        let entry = MemoryEntry::new(
            MemoryKind::Pattern,
            "Always use ?. for optional chaining".into(),
            ["rust", "style"].into_iter().map(String::from).collect(),
            None,
        );

        memory.save(&entry).await.unwrap();

        let loaded = memory.load(&entry.id).await.unwrap().unwrap();
        assert_eq!(loaded.id, entry.id);
        assert_eq!(loaded.kind, MemoryKind::Pattern);
        assert_eq!(loaded.content, entry.content);
        assert!(loaded.tags.contains("rust"));
        assert!(loaded.tags.contains("style"));
    }

    #[tokio::test]
    async fn file_project_memory_search() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join(".kiri").join("memory");
        let memory = FileProjectMemory::new(root.clone());
        memory.init().await.unwrap();

        memory
            .save(&MemoryEntry::new(
                MemoryKind::Pattern,
                "Use Option<T> for nullable values".into(),
                ["rust"].into_iter().map(String::from).collect(),
                None,
            ))
            .await
            .unwrap();
        memory
            .save(&MemoryEntry::new(
                MemoryKind::Fact,
                "Python has None".into(),
                ["python"].into_iter().map(String::from).collect(),
                None,
            ))
            .await
            .unwrap();

        let results = memory.search("Option", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("Option"));
    }

    #[tokio::test]
    async fn file_project_memory_list_by_kind() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join(".kiri").join("memory");
        let memory = FileProjectMemory::new(root.clone());
        memory.init().await.unwrap();

        memory
            .save(&MemoryEntry::new(
                MemoryKind::Pattern,
                "pattern 1".into(),
                HashSet::new(),
                None,
            ))
            .await
            .unwrap();
        memory
            .save(&MemoryEntry::new(
                MemoryKind::Fact,
                "fact 1".into(),
                HashSet::new(),
                None,
            ))
            .await
            .unwrap();

        let patterns = memory.list_by_kind(MemoryKind::Pattern, 10).await.unwrap();
        assert_eq!(patterns.len(), 1);
    }

    #[tokio::test]
    async fn file_project_memory_delete() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join(".kiri").join("memory");
        let memory = FileProjectMemory::new(root.clone());
        memory.init().await.unwrap();

        let entry = MemoryEntry::new(MemoryKind::Fact, "to delete".into(), HashSet::new(), None);
        memory.save(&entry).await.unwrap();

        let deleted = memory.delete(&entry.id).await.unwrap();
        assert!(deleted);

        let loaded = memory.load(&entry.id).await.unwrap();
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn file_project_memory_persists_index() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join(".kiri").join("memory");

        // Primeira instância salva
        {
            let memory = FileProjectMemory::new(root.clone());
            memory.init().await.unwrap();
            let entry = MemoryEntry::new(
                MemoryKind::Heuristic,
                "persist test".into(),
                HashSet::new(),
                None,
            );
            memory.save(&entry).await.unwrap();
        }

        // Segunda instância carrega do índice
        {
            let memory = FileProjectMemory::new(root.clone());
            memory.init().await.unwrap();
            let count = memory.count().await.unwrap();
            assert_eq!(count, 1);
        }
    }
}
