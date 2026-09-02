//! Tauri commands for repository management.
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::process::Command;
use uuid::Uuid;

use crate::commands::config::fetch_ed25519_fingerprint;
use crate::config_store::{self, Repository};
use crate::error::{AppError, AppResult};
use crate::git::resolve_repo_path;
use crate::pre_push_hook;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoPatch {
    pub display_name: Option<String>,
    pub working_branch: Option<String>,
    pub ssh_user: Option<String>,
    pub ssh_host: Option<String>,
    pub ssh_key_path: Option<String>,
    #[serde(default)]
    pub ssh_password: Option<String>,
    #[serde(default)]
    pub ssh_port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterProjectArgs {
    pub ssh_user: String,
    pub ssh_host: String,
    pub ssh_key_path: String,
    #[serde(default)]
    pub ssh_password: String,
    #[serde(default = "default_args_ssh_port")]
    pub ssh_port: u16,
    pub project_path: String,
}

impl Default for RegisterProjectArgs {
    /// 로컬 저장소 등록용 기본값 — SSH 항목은 모두 비어 있다.
    fn default() -> Self {
        Self {
            ssh_user: String::new(),
            ssh_host: String::new(),
            ssh_key_path: String::new(),
            ssh_password: String::new(),
            ssh_port: default_args_ssh_port(),
            project_path: String::new(),
        }
    }
}

fn default_args_ssh_port() -> u16 {
    22
}

/// Connection parameters for an SSH target (host/user/key/password/port).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshTarget {
    pub ssh_user: String,
    pub ssh_host: String,
    pub ssh_key_path: String,
    #[serde(default)]
    pub ssh_password: String,
    #[serde(default = "default_args_ssh_port")]
    pub ssh_port: u16,
}

/// One entry of a remote directory listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshDirEntry {
    pub name: String,
    pub is_dir: bool,
    pub is_symlink: bool,
}

/// Result of browsing one remote directory: resolved path, whether it is
/// inside a git work tree, and the entries (`ls -1FA` semantics, markers
/// stripped).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshDirListing {
    pub path: String,
    pub git_repo: bool,
    pub entries: Vec<SshDirEntry>,
}

/// Shell-quote a path for use in the remote command.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Parse `ls -1FA` output lines into entries. Marker characters (`/` for
/// directories, `*` executables, `@` symlinks, `|` FIFOs, `=` sockets) are
/// stripped and recorded as flags. Hidden entries are included as-is.
pub fn parse_ls_f(lines: &str) -> Vec<SshDirEntry> {
    let mut out = Vec::new();
    for line in lines.lines() {
        let name = line.trim_end_matches(['/', '*', '@', '|', '=']);
        out.push(SshDirEntry {
            is_dir: line.ends_with('/'),
            is_symlink: line.ends_with('@'),
            name: name.to_string(),
        });
    }
    out
}

fn run_ssh(args: &RegisterProjectArgs, cmd: &str) -> AppResult<std::process::Output> {
    ssh_run(
        &args.ssh_user,
        &args.ssh_host,
        &args.ssh_key_path,
        &args.ssh_password,
        args.ssh_port,
        cmd,
    )
}

fn ssh_run(
    user: &str,
    host: &str,
    key: &str,
    password: &str,
    port: u16,
    cmd: &str,
) -> AppResult<std::process::Output> {
    crate::git::run_ssh_cmd(user, host, key, port, password, cmd)
}

/// Browse a remote directory over SSH: `cd <path> && pwd && ls -1FA`, plus a
/// cheap git work-tree check for the current path. Empty `path` starts at the
/// user's home directory.
#[tauri::command]
pub fn browse_ssh_dir(target: SshTarget, path: String) -> AppResult<SshDirListing> {
    if target.ssh_host.is_empty() {
        return Err(AppError::SshAuth(
            "SSH 호스트를 먼저 입력하세요.".to_string(),
        ));
    }
    let quoted = if path.trim().is_empty() {
        "~".to_string()
    } else {
        shell_quote(&path)
    };
    let cmd = format!(
        "cd {quoted} && pwd && ls -1FA && (git rev-parse --is-inside-work-tree 2>/dev/null || true)"
    );
    let out = ssh_run(
        &target.ssh_user,
        &target.ssh_host,
        &target.ssh_key_path,
        &target.ssh_password,
        target.ssh_port,
        &cmd,
    )?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr);
        return Err(AppError::SshAuth(format!(
            "디렉터리를 열 수 없습니다: {}",
            msg.trim()
        )));
    }
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let mut lines: Vec<&str> = stdout.lines().collect();
    if lines.is_empty() {
        return Err(AppError::SshAuth("원격에서 응답이 없습니다.".to_string()));
    }
    let cwd = lines.remove(0).to_string();
    let mut git_repo = false;
    if lines.last().map(|l| l.trim() == "true").unwrap_or(false) {
        git_repo = true;
        lines.pop();
    }
    Ok(SshDirListing {
        path: cwd,
        git_repo,
        entries: parse_ls_f(&lines.join("\n")),
    })
}

/// Verify SSH connectivity to the remote host.
fn ping_remote(args: &RegisterProjectArgs) -> AppResult<()> {
    if args.ssh_host.is_empty() {
        return Ok(());
    }
    let out = run_ssh(args, "echo ok")?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr);
        return Err(AppError::SshAuth(format!(
            "SSH connect failed: {}",
            msg.trim()
        )));
    }
    Ok(())
}

/// Run a git command over SSH on the remote host.
fn git_over_ssh(
    args: &RegisterProjectArgs,
    repo_path: &str,
    git_cmd: &[&str],
) -> AppResult<std::process::Output> {
    // Build the remote command: cd <path> && git <cmd...> (each piece
    // shell-quoted so spaces in paths or args survive the remote shell).
    let git_args = git_cmd
        .iter()
        .map(|a| shell_quote(a))
        .collect::<Vec<_>>()
        .join(" ");
    crate::git::run_ssh_cmd(
        &args.ssh_user,
        &args.ssh_host,
        &args.ssh_key_path,
        args.ssh_port,
        &args.ssh_password,
        &format!("cd {} && git {}", shell_quote(repo_path), git_args),
    )
}

/// Run a git command locally.
fn git_local(repo_path: &str, git_cmd: &[&str]) -> AppResult<std::process::Output> {
    let mut c = Command::new("git");
    c.arg("-C").arg(repo_path);
    c.args(git_cmd);
    Ok(c.output()?)
}

fn run_git(
    args: &RegisterProjectArgs,
    repo_path: &str,
    git_cmd: &[&str],
) -> AppResult<std::process::Output> {
    if args.ssh_host.is_empty() {
        git_local(repo_path, git_cmd)
    } else {
        git_over_ssh(args, repo_path, git_cmd)
    }
}

#[tauri::command]
pub fn list_repositories() -> AppResult<Vec<Repository>> {
    Ok(config_store::load()?.repositories)
}

/// 아직 git 저장소가 아닌 폴더를 저장소로 만들고 바로 등록한다.
///
/// 처음 git 을 쓰는 사람은 "이 폴더는 git 저장소가 아닙니다"에서 막힌다 —
/// 무엇을 해야 하는지 모르고, 터미널로 나가야 한다는 뜻이기도 하다.
/// 되돌릴 수 있는 안전한 동작이므로(`.git` 폴더만 생긴다) 앱에서 해 준다.
#[tauri::command]
pub fn init_repository(path: String) -> AppResult<Repository> {
    let expanded = crate::git::expand_tilde(&path);
    let p = std::path::Path::new(&expanded);
    if !p.exists() {
        return Err(AppError::RepoNotFound(format!(
            "그 경로에 폴더가 없습니다: {expanded}"
        )));
    }
    if !p.is_dir() {
        return Err(AppError::RepoNotFound(format!(
            "폴더가 아닙니다: {expanded}"
        )));
    }
    if p.join(".git").exists() {
        // 이미 저장소라면 새로 만들지 않고 그대로 등록한다 — init 을 다시
        // 돌리면 기존 저장소를 건드릴 수 있다.
        return register_repository(RegisterProjectArgs {
            project_path: expanded,
            ..Default::default()
        });
    }
    let out = crate::git::run(Some(p), ["init", "-b", "main"])?;
    if !out.ok() {
        return Err(AppError::Git(format!(
            "git 저장소로 만들지 못했습니다: {}",
            out.stderr.trim()
        )));
    }
    register_repository(RegisterProjectArgs {
        project_path: expanded,
        ..Default::default()
    })
}

#[tauri::command]
pub fn register_repository(args: RegisterProjectArgs) -> AppResult<Repository> {
    // 1. Verify .git exists.
    //
    // 로컬 경로는 `~` 를 펼쳐 **확장된 경로로 저장**한다. 그러지 않으면 등록은
    // 되지만 이후 모든 git 호출이 존재하지 않는 "~/..." 를 향하게 된다.
    let mut args = args;
    if args.ssh_host.is_empty() {
        args.project_path = crate::git::expand_tilde(&args.project_path);
    }
    let args = args;
    let repo_path = &args.project_path;
    if args.ssh_host.is_empty() {
        resolve_repo_path(repo_path)?;
    } else {
        let out = run_ssh(
            &args,
            &format!("cd {} && git rev-parse --git-dir", shell_quote(repo_path)),
        )?;
        if !out.status.success() {
            return Err(AppError::RepoNotFound(format!(
                "no .git directory at {} on {}",
                repo_path, args.ssh_host
            )));
        }
    }

    // 2. Ping SSH (if remote).
    ping_remote(&args)?;

    // 3. Read remote.origin.url via git.
    let out = run_git(&args, repo_path, &["remote", "get-url", "origin"])?;
    let remote_url = if out.status.success() {
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    } else {
        String::new()
    };

    // 4. Read current branch.
    let out = run_git(&args, repo_path, &["symbolic-ref", "--short", "HEAD"])?;
    let current_branch = if out.status.success() {
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    } else {
        "main".to_string()
    };

    // 5. Get display name from directory.
    let display_name = std::path::Path::new(repo_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    // 6. Install pre-push hook.
    if args.ssh_host.is_empty() {
        let p = std::path::Path::new(repo_path);
        pre_push_hook::install(p).ok();
    }

    let repo = Repository {
        id: Uuid::new_v4(),
        path: repo_path.clone(),
        display_name,
        default_branch: current_branch.clone(),
        working_branch: current_branch,
        ssh_host: args.ssh_host.clone(),
        ssh_user: args.ssh_user.clone(),
        ssh_key_path: args.ssh_key_path.clone(),
        ssh_password: args.ssh_password.clone(),
        ed25519_fingerprint: if args.ssh_host.is_empty() {
            String::new()
        } else {
            fetch_ed25519_fingerprint(&args.ssh_host, args.ssh_port)
        },
        ssh_port: args.ssh_port,
        remote_url,
        created_at: Utc::now(),
    };

    let mut cfg = config_store::load()?;
    cfg.repositories.push(repo.clone());
    config_store::save(&cfg)?;
    Ok(repo)
}

#[tauri::command]
pub fn remove_repository(id: Uuid) -> AppResult<()> {
    let mut cfg = config_store::load()?;
    let before = cfg.repositories.len();
    cfg.repositories.retain(|r| r.id != id);
    if cfg.repositories.len() == before {
        return Err(AppError::RepoNotFound(id.to_string()));
    }
    config_store::save(&cfg)?;
    Ok(())
}

#[tauri::command]
pub fn update_repository(id: Uuid, patch: RepoPatch) -> AppResult<Repository> {
    let mut cfg = config_store::load()?;
    let repo = cfg
        .repositories
        .iter_mut()
        .find(|r| r.id == id)
        .ok_or_else(|| AppError::RepoNotFound(id.to_string()))?;
    if let Some(dn) = patch.display_name {
        repo.display_name = dn;
    }
    if let Some(wb) = patch.working_branch {
        repo.working_branch = wb;
    }
    if let Some(su) = patch.ssh_user {
        repo.ssh_user = su;
    }
    if let Some(sh) = patch.ssh_host {
        repo.ssh_host = sh;
    }
    if let Some(sk) = patch.ssh_key_path {
        repo.ssh_key_path = sk;
    }
    if let Some(sp) = patch.ssh_password {
        repo.ssh_password = sp;
    }
    if let Some(sp) = patch.ssh_port {
        repo.ssh_port = sp;
    }
    let repo = repo.clone();
    config_store::save(&cfg)?;
    Ok(repo)
}

#[cfg(test)]
mod tests {
    use super::{parse_ls_f, shell_quote};

    #[test]
    fn parses_ls_f_markers() {
        let out = parse_ls_f("src/\nmain.rs\nrun.sh*\nlink@\nfoo@bar\n.git/\n");
        let names: Vec<&str> = out.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            ["src", "main.rs", "run.sh", "link", "foo@bar", ".git"]
        );
        assert!(out[0].is_dir, "dir flag");
        assert!(!out[1].is_dir, "file flag");
        assert!(out[3].is_symlink, "symlink flag");
        assert!(out[4].is_symlink == false, "@ inside a name untouched");
        assert!(out[5].is_dir, "hidden dirs included");
    }

    #[test]
    fn shell_quote_handles_spaces_and_quotes() {
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(shell_quote("/home/me/proj"), "'/home/me/proj'");
    }
}
