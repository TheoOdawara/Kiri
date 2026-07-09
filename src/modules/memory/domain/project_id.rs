/// A deterministic blake3 hash of the workspace path. Pure — callers canonicalize first when they want
/// symlinks resolved, so one project hashes to one id however its path is spelled.
pub fn project_id_from_path(path: &std::path::Path) -> String {
    use blake3::Hasher;
    let path_str = path.to_string_lossy();
    let mut hasher = Hasher::new();
    hasher.update(path_str.as_bytes());
    let hash = hasher.finalize();
    // Use only the first 16 chars (64 bits) for readability.
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
