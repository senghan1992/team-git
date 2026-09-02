//! Install / uninstall the pre-push hook into a target git repository.
use crate::config_store::hooks_dir;
use crate::error::{AppError, AppResult};
use crate::git::run;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::Path;

pub const TEMPLATE: &str = include_str!("../templates/pre-push");

/// Install pre-push hook for `repo_path`.
///
/// Copies the embedded template to `<hooks_dir>/pre-push`, makes it executable,
/// and configures `core.hooksPath` so git picks it up. Also creates a
/// backup symlink at `.git/hooks/pre-push` for repos that ignore
/// `core.hooksPath`.
pub fn install(repo_path: &Path) -> AppResult<()> {
    // If the repo already manages its own hooks (core.hooksPath set to
    // something else), never clobber it — the app hook is only for team push
    // notifications and is fail-open anyway.
    if let Ok(o) = run(Some(repo_path), ["config", "--get", "core.hooksPath"]) {
        let existing = o.stdout.trim().to_string();
        let ours = hooks_dir()?.to_string_lossy().to_string();
        if !existing.is_empty() && existing != ours {
            return Ok(());
        }
    }
    let dir = hooks_dir()?;
    std::fs::create_dir_all(&dir)?;
    let hook_path = dir.join("pre-push");
    std::fs::write(&hook_path, TEMPLATE)?;
    std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755))?;

    // Backup symlink under .git/hooks (in case hooksPath is ignored).
    let backup = repo_path.join(".git").join("hooks").join("pre-push");
    if let Some(parent) = backup.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if backup.exists() || std::fs::symlink_metadata(&backup).is_ok() {
        let _ = std::fs::remove_file(&backup);
    }
    let _ = symlink(&hook_path, &backup);

    run(
        Some(repo_path),
        ["config", "core.hooksPath", &dir.to_string_lossy()],
    )?;
    Ok(())
}

pub fn uninstall(repo_path: &Path) -> AppResult<()> {
    run(Some(repo_path), ["config", "--unset", "core.hooksPath"])?;
    let backup = repo_path.join(".git").join("hooks").join("pre-push");
    if backup.exists() || std::fs::symlink_metadata(&backup).is_ok() {
        let _ = std::fs::remove_file(&backup);
    }
    Ok(())
}

pub fn is_installed(repo_path: &Path) -> AppResult<bool> {
    let out = run(Some(repo_path), ["config", "--get", "core.hooksPath"]);
    match out {
        Ok(o) => Ok(o.ok() && !o.stdout.trim().is_empty()),
        Err(AppError::Git(_)) => Ok(false),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_is_executable_shell() {
        assert!(TEMPLATE.starts_with("#!/usr/bin/env bash"));
        assert!(TEMPLATE.contains("hook emit"));
    }

    #[test]
    fn install_never_clobbers_repos_with_their_own_hook_path() {
        let td = tempfile::TempDir::new().unwrap();
        run(Some(td.path()), ["init", "-q"]).unwrap();
        run(
            Some(td.path()),
            ["config", "core.hooksPath", "scripts/git-hooks"],
        )
        .unwrap();

        install(td.path()).unwrap();

        let out = run(Some(td.path()), ["config", "--get", "core.hooksPath"]).unwrap();
        assert_eq!(
            out.stdout.trim(),
            "scripts/git-hooks",
            "repo's own hooksPath untouched"
        );
        assert!(
            !td.path().join(".git/hooks/pre-push").exists(),
            "no backup symlink created"
        );
    }
}
