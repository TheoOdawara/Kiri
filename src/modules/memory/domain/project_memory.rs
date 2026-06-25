use crate::modules::memory::domain::entry::{MemoryEntry, MemoryKind};
use crate::shared::kernel::error::AgentError;
use async_trait::async_trait;

type Result<T> = std::result::Result<T, AgentError>;

/// Port para persistência de memória específica do projeto.
/// Implementado por `FileProjectMemory` (arquivos Markdown em `.kiri/memory/`).
/// É o contrato CRUD+consulta completo do store; init/save/search/list já são usados pelo wiring e
/// pelas tools, e os demais (load/delete/count/list_by_*) são exercidos por testes e reservados para a
/// futura UI de gestão de memória.
#[allow(dead_code)]
#[async_trait]
pub trait ProjectMemory: Send + Sync {
    /// Inicializa o armazenamento (cria diretórios, índice, etc.).
    async fn init(&self) -> Result<()>;

    /// Salva uma entrada (cria ou atualiza por ID).
    async fn save(&self, entry: &MemoryEntry) -> Result<()>;

    /// Carrega uma entrada por ID.
    async fn load(&self, id: &str) -> Result<Option<MemoryEntry>>;

    /// Remove uma entrada por ID.
    async fn delete(&self, id: &str) -> Result<bool>;

    /// Busca entradas por query textual (content, tags, kind).
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>>;

    /// Lista todas as entradas (com paginação opcional).
    async fn list(&self, offset: usize, limit: usize) -> Result<Vec<MemoryEntry>>;

    /// Lista entradas por tipo.
    async fn list_by_kind(&self, kind: MemoryKind, limit: usize) -> Result<Vec<MemoryEntry>>;

    /// Lista entradas por tag.
    async fn list_by_tag(&self, tag: &str, limit: usize) -> Result<Vec<MemoryEntry>>;

    /// Conta total de entradas.
    async fn count(&self) -> Result<usize>;
}

/// Port para persistência de memória compartilhada entre projetos.
/// Implementado por `SqliteSharedMemory` (SQLite em `~/.kiri/memory/shared.db`).
/// Contrato CRUD+consulta completo; os métodos ainda não chamados pelo agent loop são exercidos por
/// testes e reservados para a futura UI de gestão de memória.
#[allow(dead_code)]
#[async_trait]
pub trait SharedMemory: Send + Sync {
    /// Inicializa o armazenamento (cria DB, tabelas, índices).
    async fn init(&self) -> Result<()>;

    /// Salva uma entrada (cria ou atualiza por ID).
    async fn save(&self, entry: &MemoryEntry) -> Result<()>;

    /// Carrega uma entrada por ID.
    async fn load(&self, id: &str) -> Result<Option<MemoryEntry>>;

    /// Remove uma entrada por ID.
    async fn delete(&self, id: &str) -> Result<bool>;

    /// Busca entradas por query textual.
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>>;

    /// Lista todas as entradas (com paginação).
    async fn list(&self, offset: usize, limit: usize) -> Result<Vec<MemoryEntry>>;

    /// Lista entradas por tipo.
    async fn list_by_kind(&self, kind: MemoryKind, limit: usize) -> Result<Vec<MemoryEntry>>;

    /// Lista entradas por tag.
    async fn list_by_tag(&self, tag: &str, limit: usize) -> Result<Vec<MemoryEntry>>;

    /// Lista entradas de um projeto específico (via project_id).
    async fn list_by_project(&self, project_id: &str, limit: usize) -> Result<Vec<MemoryEntry>>;

    /// Conta total de entradas.
    async fn count(&self) -> Result<usize>;

    /// Conta entradas de um projeto.
    async fn count_by_project(&self, project_id: &str) -> Result<usize>;
}

/// Gera um ID de projeto determinístico a partir do path do workspace.
/// Usa blake3 para produzir um hash curto e estável.
pub fn project_id_from_path(path: &std::path::Path) -> String {
    use blake3::Hasher;
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let path_str = canonical.to_string_lossy();
    let mut hasher = Hasher::new();
    hasher.update(path_str.as_bytes());
    let hash = hasher.finalize();
    // Usar apenas os primeiros 16 chars (64 bits) para legibilidade
    hash.to_hex().as_str()[..16].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_id_is_deterministic() {
        let path = std::path::Path::new("/tmp/test-project");
        let id1 = project_id_from_path(path);
        let id2 = project_id_from_path(path);
        assert_eq!(id1, id2);
        assert_eq!(id1.len(), 16);
    }

    #[test]
    fn different_paths_different_ids() {
        let id1 = project_id_from_path(std::path::Path::new("/tmp/proj-a"));
        let id2 = project_id_from_path(std::path::Path::new("/tmp/proj-b"));
        assert_ne!(id1, id2);
    }
}
