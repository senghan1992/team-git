use serde::{Deserialize, Serialize};

use crate::error::AppResult;
use crate::git::run;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushResult {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Push HEAD to `origin <branch>`.
pub fn push_branch(repo_path: &std::path::Path, branch: &str) -> AppResult<PushResult> {
    let out = run(
        Some(repo_path),
        ["push", "origin", &format!("HEAD:{branch}")],
    )?;
    Ok(PushResult {
        status: out.status,
        stdout: out.stdout,
        stderr: out.stderr,
    })
}
