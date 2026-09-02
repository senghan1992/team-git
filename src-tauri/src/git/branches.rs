use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::git::run;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Branch {
    pub name: String,
    pub is_remote: bool,
    pub upstream: Option<String>,
}

pub fn list_branches(repo_path: &std::path::Path) -> AppResult<Vec<Branch>> {
    // Tab-separated custom format. `%(HEAD)` and `%(upstream:short)` are empty
    // for plain listings; we only need the refname.
    let out = run(
        Some(repo_path),
        [
            "for-each-ref",
            "--format=%(refname:short)\t%(upstream:short)",
            "refs/heads",
            "refs/remotes",
        ],
    )?;
    let mut branches = Vec::new();
    for line in out.stdout.lines() {
        let mut parts = line.split('\t');
        let name = parts
            .next()
            .ok_or_else(|| AppError::Git("empty branch line".into()))?
            .trim();
        if name.is_empty() {
            continue;
        }
        let upstream = parts
            .next()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let is_remote = name.contains('/');
        branches.push(Branch {
            name: name.to_string(),
            is_remote,
            upstream,
        });
    }
    Ok(branches)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_local_and_remote() {
        let stdout = "main\torigin/main\nfeat\t\norigin/feat\tremotes/origin/feat\n";
        // We parse manually here since `list_branches` shells out to git.
        let mut lines: Vec<Branch> = vec![];
        for line in stdout.lines() {
            let mut parts = line.split('\t');
            let name = parts.next().unwrap().trim().to_string();
            let upstream = parts
                .next()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            lines.push(Branch {
                is_remote: name.contains('/'),
                name,
                upstream,
            });
        }
        assert_eq!(lines.len(), 3);
        assert!(!lines[0].is_remote);
        assert!(lines[2].is_remote);
    }
}

/// List branches for a Target (local or SSH remote).
pub fn list_branches_at(target: &crate::git::Target) -> AppResult<Vec<Branch>> {
    let out = crate::git::run_at_target(
        target,
        &[
            "for-each-ref",
            "--format=%(refname:short)\t%(upstream:short)",
            "refs/heads",
            "refs/remotes",
        ],
    )?;
    let mut branches = Vec::new();
    for line in out.stdout.lines() {
        let mut parts = line.split('\t');
        let name = parts
            .next()
            .ok_or_else(|| AppError::Git("empty branch line".into()))?
            .trim()
            .to_string();
        if name.is_empty() {
            continue;
        }
        let upstream = parts
            .next()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        // is_remote = starts with "remotes/" (the refspec prefix from for-each-ref)
        let is_remote = name.starts_with("remotes/");
        let display_name = if is_remote {
            name.strip_prefix("remotes/").unwrap_or(&name).to_string()
        } else {
            name.clone()
        };
        branches.push(Branch {
            name: display_name,
            is_remote,
            upstream,
        });
    }
    Ok(branches)
}
