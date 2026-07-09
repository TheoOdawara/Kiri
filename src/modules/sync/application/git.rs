use std::path::Path;

use crate::shared::kernel::error::AgentResult;

pub struct GitOutput {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
}

#[async_trait::async_trait]
pub trait Git: Send + Sync {
    /// A non-zero exit lands in `GitOutput.success`; only a failure to launch the process is `Err`.
    async fn run(&self, args: &[&str], cwd: &Path) -> AgentResult<GitOutput>;
}
