//! Thin wrapper around the system `git` binary.
//!
//! v1 deliberately avoids libgit2. We invoke `git` via `std::process::Command`.
//! Every invocation goes through `output_with_timeout` (hard cap, see
//! `EXEC_TIMEOUT`); SSH sessions additionally carry keepalive options so a
//! dead connection fails in seconds instead of pinning a worker for minutes.
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

/// Windows `CREATE_NO_WINDOW` — 콘솔 프로그램(git·ssh·sshpass·ssh-keyscan…)을
/// spawn할 때 콘솔 창을 만들지 않는다. 이 앱은 GUI 서브시스템(`windows_subsystem
/// = "windows"`)이라 부모에게 상속할 콘솔이 없어서, 플래그 없이는 SSH 연결·
/// 저장소 작업마다 검은 cmd 창이 깞박였다 사라졌다 한다. 창만 없을 뿐
/// stdout/stderr 파이프는 그대로 동작하므로 출력 수집·타임아웃 로직은
/// 영향을 받지 않는다.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 모든 자식 프로세스는 이 생성자로 만든다 — Windows에서는 자동으로
/// `CREATE_NO_WINDOW`가 걸린다.
pub fn new_command(program: &str) -> Command {
    #[allow(unused_mut)] // Windows에서만 creation_flags 호출로 mut가 필요하다.
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

pub mod auto;
pub mod branches;
pub mod fetch;
pub mod log;
pub mod merge;
pub mod ops;
pub mod push;
pub mod status;
pub mod sync;
pub mod timeline;

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
            cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
            let out = output_with_timeout(&mut cmd, EXEC_TIMEOUT)
                .map_err(|e| AppError::Io(e.to_string()))?;
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

/// git 이 C-quoting 으로 감싼 경로를 원래 문자열로 되돌린다.
///
/// `core.quotepath=off` 로 한글 등 비ASCII는 그대로 나오지만, `"` 나 제어
/// 문자가 든 파일명은 여전히 `"a\"b.txt"` 형태로 감싸여 나온다 — 그대로
/// 반환하면 그 경로로 하는 diff/add/해결이 전부 "파일 없음"이 된다.
pub(crate) fn unquote_git_path(s: &str) -> String {
    let s = s.trim_end_matches('\r');
    if s.len() < 2 || !s.starts_with('"') || !s.ends_with('"') {
        return s.to_string();
    }
    let inner = &s[1..s.len() - 1];
    let mut bytes: Vec<u8> = Vec::with_capacity(inner.len());
    let mut it = inner.bytes().peekable();
    while let Some(b) = it.next() {
        if b != b'\\' {
            bytes.push(b);
            continue;
        }
        match it.next() {
            Some(b'\\') => bytes.push(b'\\'),
            Some(b'"') => bytes.push(b'"'),
            Some(b't') => bytes.push(b'\t'),
            Some(b'n') => bytes.push(b'\n'),
            Some(b'r') => bytes.push(b'\r'),
            Some(d @ b'0'..=b'7') => {
                // 8진수 최대 3자리 (UTF-8 바이트 단위).
                let mut v = (d - b'0') as u32;
                for _ in 0..2 {
                    match it.peek() {
                        Some(&n @ b'0'..=b'7') => {
                            v = v * 8 + (n - b'0') as u32;
                            it.next();
                        }
                        _ => break,
                    }
                }
                bytes.push(v as u8);
            }
            Some(other) => {
                bytes.push(b'\\');
                bytes.push(other);
            }
            None => bytes.push(b'\\'),
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(test)]
mod unquote_tests {
    use super::unquote_git_path;

    #[test]
    fn plain_and_quoted_paths_roundtrip() {
        assert_eq!(unquote_git_path("src/한글 파일.ts"), "src/한글 파일.ts");
        assert_eq!(unquote_git_path("\"a\\\"b.txt\""), "a\"b.txt");
        assert_eq!(unquote_git_path("\"tab\\there\""), "tab\there");
        assert_eq!(unquote_git_path("\"back\\\\slash\""), "back\\slash");
        // 8진수 UTF-8 바이트 (quotepath=on 환경 대비): "한" = \354\225\234 아님,
        // 실제 값으로 검증 — '한' = ED 95 9C.
        assert_eq!(unquote_git_path("\"\\355\\225\\234\""), "한");
        assert_eq!(unquote_git_path("\"\""), "");
        assert_eq!(unquote_git_path("no-quotes"), "no-quotes");
    }
}

/// Build an `ssh` command targeting `user@host`, pre-loaded with the same auth
/// options the rest of the app uses. Call sites append their remote command.
/// The password is passed via environment variables only, never on argv:
/// - `sshpass -e` where sshpass is installed (Linux desktop, dev server);
/// - otherwise OpenSSH's `SSH_ASKPASS` mechanism (Windows, plain macOS) — the
///   helper is this app's own executable (`askpass` subcommand), which prints
///   the `SSHPASS` env var. `SSH_ASKPASS_REQUIRE=force` makes ssh call it even
///   without a console or DISPLAY (OpenSSH ≥ 8.4; Git for Windows ≥ 8.4).
pub fn build_ssh_cmd(
    user: &str,
    host: &str,
    key: &str,
    port: u16,
    password: &str,
) -> std::process::Command {
    build_ssh_cmd_timeout(user, host, key, port, password, 5)
}

/// `build_ssh_cmd` + 커넥션 타임아웃(초) 지정 — SSH 연결 테스트가 쓴다.
pub fn build_ssh_cmd_timeout(
    user: &str,
    host: &str,
    key: &str,
    port: u16,
    password: &str,
    connect_timeout_secs: u16,
) -> std::process::Command {
    let password_auth = !password.is_empty();
    let mut cmd = if password_auth {
        if sshpass_available() {
            let mut c = new_command("sshpass");
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
            // sshpass 가 없는 환경 (Windows, 기본 macOS): ssh 자체의
            // SSH_ASKPASS 헬퍼로 비밀번호를 넘긴다. 헬퍼는 이 앱의 실행
            // 파일(askpass 서브커맨드) — 별도 배포 파일이 필요 없다.
            let mut c = new_command("ssh");
            c.arg("-o")
                .arg("StrictHostKeyChecking=accept-new")
                .arg("-o")
                .arg("PreferredAuthentications=password")
                .arg("-o")
                .arg("PubkeyAuthentication=no")
                .arg("-o")
                .arg("NumberOfPasswordPrompts=1");
            c.env("SSHPASS", password);
            c.env("SSH_ASKPASS", askpass_helper_path());
            c.env("SSH_ASKPASS_REQUIRE", "force");
            c
        }
    } else {
        let mut c = new_command("ssh");
        if !key.is_empty() {
            c.arg("-i").arg(key);
        }
        c.arg("-o").arg("BatchMode=yes");
        c.arg("-o").arg("StrictHostKeyChecking=accept-new");
        c
    };
    cmd.arg("-o")
        .arg(format!("ConnectTimeout={connect_timeout_secs}"));
    // 연결 후 네트워크가 끊기면 TCP만으로는 몇 분씩 매달린다 — keepalive 로
    // 죽은 세션을 ~15초(5s×3회) 안에 끊는다. 데이터가 오가는 동안에는
    // 발동하지 않으므로 오래 걸리는 정상 push/fetch 는 죽이지 않는다.
    cmd.arg("-o").arg("ServerAliveInterval=5");
    cmd.arg("-o").arg("ServerAliveCountMax=3");
    if port != 22 {
        cmd.arg("-p").arg(port.to_string());
    }
    if user.is_empty() {
        cmd.arg(host);
    } else {
        cmd.arg(format!("{user}@{host}"));
    }
    cmd.env("LC_ALL", "C.UTF-8").env("LANG", "C.UTF-8");
    cmd
}

/// sshpass 가 PATH 에 있는지 — 최초 한 번만 프로세스 확인 후 캐시한다.
pub fn sshpass_available() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| {
        new_command("sshpass")
            .arg("-V")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

/// SSH_ASKPASS 헬퍼 경로 — 이 앱 실행 파일 자체 (`askpass` 서브커맨드).
/// 번들(설치) 환경에서도 current_exe 가 설치본을 가리키므로 추가 파일이
/// 필요 없다. OpenSSH 는 헬퍼를 `<경로> <프롬프트>` 로 호출한다.
pub fn askpass_helper_path() -> String {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "git-companion".into())
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
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        output_with_timeout(&mut cmd, EXEC_TIMEOUT).map_err(|e| map_ssh_spawn_err(e, use_password))
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
        let output = output_with_timeout(&mut cmd, EXEC_TIMEOUT)
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

/// 프로세스 실행 하드 캡. keepalive/ConnectTimeout 이 잡지 못하는 나머지
/// (스크립트가 멈춘 훅, 응답 없는 자격증명 헬퍼 등)를 위한 최후의 보루다.
/// 큰 저장소의 정상 push/fetch 를 죽이지 않도록 넉넉하게 잡는다.
const EXEC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// `Command::output()` 대체 — stdout/stderr 를 스레드로 계속 읽으면서
/// (파이프가 가득 차 자식이 멈추는 교착 방지) 시한을 넘기면 프로세스를
/// 죽이고 오류를 돌려준다. UI 가 영원히 "…중"에 머무는 일이 없게 한다.
fn output_with_timeout(
    cmd: &mut Command,
    timeout: std::time::Duration,
) -> std::io::Result<std::process::Output> {
    use std::io::Read;
    use std::time::Instant;

    cmd.stdin(Stdio::null());
    let mut child = cmd.spawn()?;
    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();
    let out_t = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = out_pipe.as_mut() {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });
    let err_t = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = err_pipe.as_mut() {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break st;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            // 읽기 스레드는 파이프가 닫히며 스스로 끝난다.
            let _ = out_t.join();
            let _ = err_t.join();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "{}초 안에 끝나지 않아 중단했습니다 — 네트워크/원격 상태를 확인하세요.",
                    timeout.as_secs()
                ),
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    Ok(std::process::Output {
        status,
        stdout: out_t.join().unwrap_or_default(),
        stderr: err_t.join().unwrap_or_default(),
    })
}

/// Run a git subcommand in `cwd`. Captures stdout+stderr.
pub fn run<I, S>(cwd: Option<&Path>, args: I) -> AppResult<GitOutput>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = new_command("git");
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
    let output = output_with_timeout(&mut cmd, EXEC_TIMEOUT)
        .map_err(|e| AppError::Git(format!("git 실행 실패: {e}")))?;
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
    let mut cmd = new_command("git");
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
    let output = output_with_timeout(&mut cmd, EXEC_TIMEOUT)
        .map_err(|e| AppError::Git(format!("git 실행 실패: {e}")))?;
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

// ── 원격 URL 정규화 ─────────────────────────────────────────────────────────

/// 원격 URL을 팀원끼리 비교 가능한 열쇠로 정규화한다: `host/path` 형태.
///
/// 알림 이벤트는 이 값으로 "받는 쪽의 어느 등록 저장소 이야기인가"를 찾는다 —
/// 저장소 폴더 이름은 사람마다 다르지만 origin은 팀이 공유하기 때문이다.
/// **자격증명(`https://user:token@host/…`)은 반드시 벗겨 낸다** — 이벤트
/// payload는 팀 전체에 배달되므로 그대로 두면 push 토큰이 유출된다.
///
/// 처리: scheme 제거 → userinfo 제거 → scp 형식(`git@host:path`)의 `:`를
/// `/`로 (포트 번호는 유지) → 끝의 `/`와 `.git` 제거 → 호스트만 소문자.
pub fn normalize_remote_url(url: &str) -> String {
    let mut s = url.trim().to_string();
    if let Some(idx) = s.find("://") {
        s = s[idx + 3..].to_string();
    }
    if let Some(at) = s.rfind('@') {
        // `user:pass@host/...` 와 scp 형식 `git@host:path` 모두 여기서 벗겨진다.
        s = s[at + 1..].to_string();
    }
    if let Some(colon) = s.find(':') {
        let after = &s[colon + 1..];
        let head = after.split('/').next().unwrap_or("");
        let is_port = !head.is_empty() && head.chars().all(|c| c.is_ascii_digit());
        if !is_port {
            s.replace_range(colon..=colon, "/");
        }
    }
    let s = s.trim_end_matches('/');
    let s = s.strip_suffix(".git").unwrap_or(s);
    let s = s.trim_end_matches('/');
    match s.find('/') {
        Some(i) => format!("{}{}", s[..i].to_lowercase(), &s[i..]),
        None => s.to_lowercase(),
    }
}

#[cfg(test)]
mod remote_url_tests {
    use super::normalize_remote_url;

    #[test]
    fn same_repo_many_spellings_normalize_equal() {
        let expect = "github.com/team/app";
        for u in [
            "https://github.com/team/app.git",
            "https://github.com/team/app",
            "http://github.com/team/app.git/",
            "git@github.com:team/app.git",
            "ssh://git@github.com/team/app.git",
            "GITHUB.com/team/app",
        ] {
            assert_eq!(normalize_remote_url(u), expect, "input: {u}");
        }
    }

    #[test]
    fn credentials_never_survive() {
        let n = normalize_remote_url("http://oauth2:glpat-secret@git.corp.com/hub/team/app.git");
        assert_eq!(n, "git.corp.com/hub/team/app");
        assert!(!n.contains("secret") && !n.contains("oauth2"));
    }

    #[test]
    fn ports_are_kept_and_paths_stay_case_sensitive() {
        // 포트는 `:` 그대로 유지 — 열쇠는 양쪽이 같은 규칙으로만 만들면 된다.
        assert_eq!(
            normalize_remote_url("ssh://git@Host.com:2222/Team/App.git"),
            "host.com:2222/Team/App"
        );
        assert_eq!(
            normalize_remote_url("https://host.com:8443/team/app.git"),
            "host.com:8443/team/app"
        );
        // scp 형식의 `:`(포트 아님)는 경로 구분자로 바뀐다.
        assert_eq!(
            normalize_remote_url("git@host.com:team/app.git"),
            "host.com/team/app"
        );
    }
}
