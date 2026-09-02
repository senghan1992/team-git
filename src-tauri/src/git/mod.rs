//! Thin wrapper around the system `git` binary.
//!
//! v1 deliberately avoids libgit2. We invoke `git` via `std::process::Command`
//! with a 30s timeout (enforced by the parent process via tokio::time::timeout).
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

pub mod auto;
pub mod branches;
pub mod fetch;
pub mod log;
pub mod merge;
pub mod ops;
pub mod push;
pub mod status;
pub mod sync;

pub use branches::{list_branches, list_branches_at, Branch};
pub use fetch::fetch_origin;
pub use log::{parse_log, Commit, CommitsPage};
pub use push::{push_branch, PushResult};
pub use status::{parse_status, FileChange, FileChangeKind, WorkingTreeStatus};
pub use sync::{run_merge, run_pull_and_merge, sync_to_base, SyncResult};

/// Re-export automatic-resolution types for use by commands.
pub use auto::{
    auto_resolve_merge, list_backups, restore_backup, AutoResolveOptions, AutoResolveReport,
    BackupEntry, FileResolution, SideChoice,
};

/// Re-export ops types for use by commands.
pub use ops::{
    add, changed_files, checkout_branch, commit, create_branch, diff, list_commits, list_stashes,
    list_status, pull, push, stash, ChangedFilesOutcome, CommitResult, DiffOpts, PullOutcome,
    PushOutcome, StashAction, StatusScope,
};

/// Re-export merge-center types for use by commands.
pub use merge::{
    abort_merge, complete_merge, conflict_detail, list_pending_branches, merge_in_progress,
    remaining_conflicts, resolve_conflict, start_merge, ChangedPath, ConflictDetail, MergeOutcome,
    PendingBranch, Resolution,
};

/// Target for git operations — local path or remote SSH host.
#[derive(Debug, Clone)]
pub enum Target {
    Local(PathBuf),
    Ssh {
        user: String,
        host: String,
        key: String,
        password: String,
        port: u16,
        path: PathBuf,
    },
}

impl Target {
    /// Build a Target from repository fields.
    pub fn from_repo(
        path: &str,
        ssh_host: &str,
        ssh_user: &str,
        ssh_key_path: &str,
        ssh_password: &str,
        ssh_port: u16,
    ) -> Self {
        if ssh_host.is_empty() {
            Target::Local(PathBuf::from(path))
        } else {
            Target::Ssh {
                user: ssh_user.to_string(),
                host: ssh_host.to_string(),
                key: ssh_key_path.to_string(),
                password: ssh_password.to_string(),
                port: ssh_port,
                path: PathBuf::from(path),
            }
        }
    }

    /// Working directory / remote path.
    pub fn path(&self) -> &Path {
        match self {
            Target::Local(p) => p,
            Target::Ssh { path, .. } => path,
        }
    }
}

/// Run a git subcommand targeting a local path or remote SSH host.
/// Captures stdout+stderr.
pub fn run_at_target<I, S>(target: &Target, args: I) -> AppResult<GitOutput>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let git_args: Vec<String> = args
        .into_iter()
        .map(|s| s.as_ref().to_string_lossy().into_owned())
        .collect();

    match target {
        Target::Local(dir) => run(Some(dir), git_args.iter().map(|s| s.as_str())),
        Target::Ssh {
            user,
            host,
            key,
            password,
            port,
            path,
        } => run_ssh(user, host, key, password, *port, path, &git_args),
    }
}

/// Write arbitrary bytes to `<root>/<rel_path>` for a Local target, or via
/// `ssh ... cat > <path>` for an Ssh target. Used by the conflict resolver to
/// drop in a manually edited file body.
pub fn write_file_at_target(target: &Target, rel_path: &str, contents: &[u8]) -> AppResult<()> {
    match target {
        Target::Local(root) => {
            let full = root.join(rel_path);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&full, contents)?;
        }
        Target::Ssh {
            user,
            host,
            key,
            password,
            port,
            path,
        } => {
            let pw = !password.is_empty();
            let mut cmd = build_ssh_cmd(user, host, key, *port, password);
            cmd.arg("--").arg(format!(
                "cat > {}",
                shell_quote(&format!("{}/{}", path.to_string_lossy(), rel_path))
            ));
            cmd.stdin(Stdio::piped());
            let mut child = cmd.spawn().map_err(|e| map_ssh_spawn_err(e, pw))?;
            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                stdin.write_all(contents)?;
            }
            let out = child
                .wait_with_output()
                .map_err(|e| AppError::Git(format!("ssh wait failed: {e}")))?;
            if !out.status.success() {
                return Err(AppError::Git(format!(
                    "ssh write failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                )));
            }
        }
    }
    Ok(())
}

/// Read `<root>/<rel_path>` for either target kind. Local: `fs::read`; Ssh:
/// `ssh <user>@<host> -p <port> [-i key] cat <root>/<path>`. Used by the
/// conflict resolver to fetch the current worktree body (which lives outside
/// the index while a merge is in progress, so `git show :<path>` would fail).
pub fn read_file_at_target(target: &Target, rel_path: &str) -> AppResult<Vec<u8>> {
    match target {
        Target::Local(root) => {
            let full = root.join(rel_path);
            Ok(std::fs::read(&full)?)
        }
        Target::Ssh {
            user,
            host,
            key,
            password,
            port,
            path,
        } => {
            let full = format!("{}/{}", path.to_string_lossy(), rel_path);
            let mut cmd = build_ssh_cmd(user, host, key, *port, password);
            cmd.arg("--").arg(format!("cat {}", shell_quote(&full)));
            let out = cmd.output().map_err(|e| AppError::Io(e.to_string()))?;
            if !out.status.success() {
                // Empty file or unreachable host — caller treats empty body as
                // "no working copy available", which is the safe default for
                // a UI that just needs to render markers.
                return Ok(Vec::new());
            }
            Ok(out.stdout)
        }
    }
}

/// Shell-quote a string for use in a remote command (single-quote wrap with
/// `'\''` escaping). Safe against injection and spaces/quotes in paths.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Build an `ssh` (or `sshpass … ssh` for password auth) command targeting
/// `user@host`, pre-loaded with the same auth options the rest of the app
/// uses. Call sites append their remote command. The password is passed via
/// the `SSHPASS` env var so it never appears on the process command line.
pub fn build_ssh_cmd(
    user: &str,
    host: &str,
    key: &str,
    port: u16,
    password: &str,
) -> std::process::Command {
    let password_auth = !password.is_empty();
    let mut cmd = if password_auth {
        let mut c = std::process::Command::new("sshpass");
        c.arg("-e").arg("ssh");
        c.arg("-o")
            .arg("StrictHostKeyChecking=accept-new")
            .arg("-o")
            .arg("PreferredAuthentications=password")
            .arg("-o")
            .arg("PubkeyAuthentication=no")
            .arg("-o")
            .arg("NumberOfPasswordPrompts=1");
        c.env("SSHPASS", password);
        c
    } else {
        let mut c = std::process::Command::new("ssh");
        if !key.is_empty() {
            c.arg("-i").arg(key);
        }
        c.arg("-o").arg("BatchMode=yes");
        c.arg("-o").arg("StrictHostKeyChecking=accept-new");
        c
    };
    cmd.arg("-o").arg("ConnectTimeout=5");
    if port != 22 {
        cmd.arg("-p").arg(port.to_string());
    }
    cmd.arg(format!("{user}@{host}"));
    cmd.env("LC_ALL", "C.UTF-8").env("LANG", "C.UTF-8");
    cmd
}

/// Run one remote command, returning its output. When both a key and a
/// password are configured, the password is tried first (the user's
/// preference) and the key is used as a fallback when the server rejects
/// it — e.g. Ubuntu servers default to `PermitRootLogin prohibit-password`,
/// which refuses root password logins while still accepting keys. Returns
/// the last attempt's output so callers surface a truthful error.
pub fn run_ssh_cmd(
    user: &str,
    host: &str,
    key: &str,
    port: u16,
    password: &str,
    remote_cmd: &str,
) -> AppResult<std::process::Output> {
    let password_auth = !password.is_empty();
    let attempt = |use_password: bool| -> AppResult<std::process::Output> {
        let mut cmd = build_ssh_cmd(
            user,
            host,
            key,
            port,
            if use_password { password } else { "" },
        );
        cmd.arg(remote_cmd);
        cmd.output().map_err(|e| map_ssh_spawn_err(e, use_password))
    };
    if password_auth && !key.is_empty() {
        let first = attempt(true)?;
        if first.status.success()
            || !String::from_utf8_lossy(&first.stderr).contains("Permission denied")
        {
            return Ok(first);
        }
        return attempt(false);
    }
    attempt(password_auth)
}

/// Map a spawn failure to a helpful error. Password auth needs `sshpass`;
/// everything else is an ssh spawn/environment problem.
pub fn map_ssh_spawn_err(e: std::io::Error, password_auth: bool) -> AppError {
    if password_auth && e.kind() == std::io::ErrorKind::NotFound {
        AppError::SshAuth(
            "비밀번호 인증에는 sshpass가 필요합니다. (sudo apt install sshpass)".to_string(),
        )
    } else {
        AppError::Git(format!("failed to spawn ssh: {e}"))
    }
}

fn run_ssh(
    user: &str,
    host: &str,
    key: &str,
    password: &str,
    port: u16,
    path: &Path,
    git_args: &[String],
) -> AppResult<GitOutput> {
    // OpenSSH joins argv with spaces and the remote shell re-parses it, so
    // anything that can contain spaces (paths, messages, pathspecs…) must be
    // shell-quoted here.
    let remote = format!(
        "git -C {} {}",
        shell_quote(&path.to_string_lossy()),
        git_args
            .iter()
            .map(|a| shell_quote(a))
            .collect::<Vec<_>>()
            .join(" ")
    );
    run_ssh_command(user, host, key, password, port, &remote)
}

/// Execute an arbitrary remote shell command over SSH with the same auth
/// fallback policy as `run_ssh`.
pub fn run_ssh_command(
    user: &str,
    host: &str,
    key: &str,
    password: &str,
    port: u16,
    remote: &str,
) -> AppResult<GitOutput> {
    let pw = !password.is_empty();
    let run_once = |use_password: bool| -> AppResult<GitOutput> {
        let mut cmd = build_ssh_cmd(
            user,
            host,
            key,
            port,
            if use_password { password } else { "" },
        );
        cmd.arg("--").arg(remote);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let output = cmd
            .output()
            .map_err(|e| map_ssh_spawn_err(e, use_password))?;
        Ok(GitOutput {
            status: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    };
    if pw && !key.is_empty() {
        // 비밀번호를 먼저 시도하고, 서버가 거부하면 키로 재시도한다.
        let first = run_once(true)?;
        if first.status == 0 || !first.stderr.contains("Permission denied") {
            return Ok(first);
        }
        return run_once(false);
    }
    run_once(pw)
}

/// Run a git subcommand in `cwd`. Captures stdout+stderr.
pub fn run<I, S>(cwd: Option<&Path>, args: I) -> AppResult<GitOutput>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = Command::new("git");
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.arg("-c").arg("core.quotepath=off");
    cmd.arg("-c").arg("i18n.log.outputEncoding=UTF-8");
    for a in args {
        cmd.arg(a);
    }
    cmd.env("LC_ALL", "C.UTF-8");
    cmd.env("LANG", "C.UTF-8");
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let output = cmd
        .output()
        .map_err(|e| AppError::Git(format!("failed to spawn git: {e}")))?;
    Ok(GitOutput {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// Run a git subcommand in `cwd` with extra environment variables.
/// Used for GIT_ASKPASS-based credential injection (push).
pub fn run_with_env<I, S>(
    cwd: Option<&Path>,
    args: I,
    extra_env: &[(&str, &str)],
) -> AppResult<GitOutput>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = Command::new("git");
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.arg("-c").arg("core.quotepath=off");
    cmd.arg("-c").arg("i18n.log.outputEncoding=UTF-8");
    for a in args {
        cmd.arg(a);
    }
    cmd.env("LC_ALL", "C.UTF-8");
    cmd.env("LANG", "C.UTF-8");
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let output = cmd
        .output()
        .map_err(|e| AppError::Git(format!("failed to spawn git: {e}")))?;
    Ok(GitOutput {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// Run a remote git command with extra environment variables (prefixes the
/// remote shell command with `VAR=value …`).
pub fn run_at_target_env<I, S>(
    target: &Target,
    args: I,
    extra_env: &[(&str, &str)],
) -> AppResult<GitOutput>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let git_args: Vec<String> = args
        .into_iter()
        .map(|s| s.as_ref().to_string_lossy().into_owned())
        .collect();
    match target {
        Target::Local(dir) => {
            run_with_env(Some(dir), git_args.iter().map(|s| s.as_str()), extra_env)
        }
        Target::Ssh {
            user,
            host,
            key,
            password,
            port,
            path,
        } => {
            let assignments = extra_env
                .iter()
                .map(|(k, v)| format!("{}={}", k, shell_quote(v)))
                .collect::<Vec<_>>()
                .join(" ");
            let remote = format!(
                "{} git -C {} {}",
                assignments,
                shell_quote(&path.to_string_lossy()),
                git_args
                    .iter()
                    .map(|a| shell_quote(a))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            run_ssh_command(user, host, key, password, *port, &remote)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl GitOutput {
    pub fn ok(&self) -> bool {
        self.status == 0
    }
    pub fn into_result(self) -> AppResult<Self> {
        if self.ok() {
            Ok(self)
        } else {
            Err(AppError::Git(format!(
                "git exited {}: {}",
                self.status,
                self.stderr.trim()
            )))
        }
    }
}

/// `~` 또는 `~/…` 를 사용자 홈으로 펼친다.
///
/// 사람들은 경로를 손으로 칠 때 습관적으로 `~` 를 쓴다. 예전에는 그대로
/// 넘겨서 "경로가 없습니다"로 끝났고, 게다가 SSH 키 경로 placeholder 는
/// `~/.ssh/id_ed25519` 였으니 한쪽은 되고 한쪽은 안 되는 셈이었다.
pub fn expand_tilde(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed == "~" {
        return dirs::home_dir()
            .map(|h| h.to_string_lossy().into_owned())
            .unwrap_or_else(|| trimmed.to_string());
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    }
    trimmed.to_string()
}

/// 입력한 경로를 검증한다. 실패 이유마다 다른 한국어 메시지를 준다 —
/// "없는 경로"와 "git 저장소가 아님"은 사용자가 해야 할 일이 전혀 다르다.
pub fn resolve_repo_path(input: &str) -> AppResult<std::path::PathBuf> {
    let expanded = expand_tilde(input);
    let p = std::path::PathBuf::from(&expanded);
    if expanded.is_empty() {
        return Err(AppError::RepoNotFound(
            "저장소 폴더 경로를 입력하세요.".into(),
        ));
    }
    if !p.exists() {
        return Err(AppError::RepoNotFound(format!(
            "그 경로에 폴더가 없습니다: {expanded}\n경로를 다시 확인하세요. 전체 경로(예: /home/이름/projects/my-app)로 입력해야 합니다."
        )));
    }
    if !p.is_dir() {
        return Err(AppError::RepoNotFound(format!(
            "폴더가 아니라 파일입니다: {expanded}\n저장소 폴더 자체를 고르세요."
        )));
    }
    if !p.join(".git").exists() {
        // 상위 폴더가 저장소인 흔한 실수(하위 폴더를 고름)를 잡아 준다.
        let hint = p
            .ancestors()
            .skip(1)
            .find(|a| a.join(".git").exists())
            .map(|a| format!("\n혹시 이 폴더를 찾으셨나요? {}", a.display()))
            .unwrap_or_else(|| {
                "\ngit clone 으로 받은 폴더를 고르거나, 이 폴더를 저장소로 만들려면 아래 ‘git 저장소로 만들기’를 쓰세요.".to_string()
            });
        return Err(AppError::RepoNotFound(format!(
            "이 폴더는 git 저장소가 아닙니다 (.git 이 없습니다): {expanded}{hint}"
        )));
    }
    Ok(p)
}
