//! Git subcommand wrappers for in-app commit/push/pull workflow.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config_store::PushCredential;
use crate::error::{AppError, AppResult};
use crate::git::status::{FileChange, FileChangeKind};
use crate::git::{
    log, run_at_target, run_ssh_command, shell_quote, status, write_file_at_target, Target,
    WorkingTreeStatus,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushOutcome {
    pub ok: bool,
    pub pushed_sha: Option<String>,
    pub message: String,
    /// True when the push failed because the remote is HTTPS and no (or bad)
    /// credentials were supplied — the UI then asks for username/password.
    #[serde(default)]
    pub auth_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullOutcome {
    pub ok: bool,
    pub message: String,
    pub conflicted_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitResult {
    pub ok: bool,
    pub sha: Option<String>,
    pub message: String,
}

pub type ChangedFilesOutcome = WorkingTreeStatus;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatusScope {
    Staged,
    Unstaged,
    Untracked,
    All,
}

#[derive(Debug, Clone, Default)]
pub struct DiffOpts {
    pub pathspec: Option<String>,
    pub staged: bool,
    pub stat: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StashAction {
    Save {
        message: Option<String>,
    },
    Pop,
    /// `git stash pop <index>` — restore a specific stash entry (e.g. `stash@{1}`).
    PopIndex(String),
    List,
    Drop,
    /// `git stash drop <index>` — discard a specific stash entry.
    DropIndex(String),
    Clear,
}

// ── add ─────────────────────────────────────────────────────────────────────────

pub fn add(target: &Target, paths: &[String]) -> AppResult<()> {
    let args: Vec<&str> = if paths.is_empty() {
        vec!["add", "-A"]
    } else {
        let mut a = vec!["add"];
        a.extend(paths.iter().map(|s| s.as_str()));
        a
    };
    run_at_target(target, &args)?;
    Ok(())
}

// ── commit ─────────────────────────────────────────────────────────────────────

pub fn commit(target: &Target, message: &str, stage_all: bool) -> AppResult<CommitResult> {
    if stage_all {
        run_at_target(target, ["add", "-A"])?;
    }
    let out = run_at_target(target, ["commit", "-m", message])?;
    if out.ok() {
        let sha = run_at_target(target, ["rev-parse", "HEAD"])
            .ok()
            .map(|o| o.stdout.trim().to_string())
            .filter(|s| !s.is_empty());
        Ok(CommitResult {
            ok: true,
            sha,
            message: out.stdout,
        })
    } else {
        Ok(CommitResult {
            ok: false,
            sha: None,
            message: explain_commit_failure(&out.stdout, &out.stderr),
        })
    }
}

/// 커밋 실패 이유를 사람 말로 바꾼다.
///
/// git 은 "nothing to commit" 같은 가장 흔한 실패를 **stdout** 에 쓴다.
/// 예전에는 stderr 만 읽어서 메시지가 빈 문자열이 됐고, 화면에는
/// "커밋 실패: " 만 떴다 — 커밋할 변경이 없을 때 버튼을 누르는 건 처음
/// 쓰는 사람이 가장 자주 하는 일인데, 아무 설명이 없었다.
pub fn explain_commit_failure(stdout: &str, stderr: &str) -> String {
    let all = format!("{stdout}\n{stderr}");
    if all.contains("nothing to commit")
        || all.contains("no changes added to commit")
        || all.contains("nothing added to commit")
    {
        return "커밋할 변경이 없습니다. 파일을 수정한 뒤 다시 커밋하세요.".into();
    }
    if all.contains("empty commit message") || all.contains("Aborting commit due to empty") {
        return "커밋 메시지를 입력하세요.".into();
    }
    if all.contains("Please tell me who you are") || all.contains("unable to auto-detect email") {
        return "git 사용자 정보가 없어 커밋할 수 없습니다. 터미널에서 한 번 설정하세요:\n  git config --global user.name \"이름\"\n  git config --global user.email \"메일@example.com\"".into();
    }
    if all.contains("index.lock") {
        return "다른 git 작업이 진행 중입니다(.git/index.lock). 잠시 후 다시 시도하세요.".into();
    }
    if all.contains("unmerged") || all.contains("Unmerged paths") {
        return "해결하지 않은 충돌이 남아 있습니다. 병합 탭에서 먼저 마무리하세요.".into();
    }
    // 알 수 없는 실패 — 최소한 git 이 한 말은 보여 준다.
    let raw = if !stderr.trim().is_empty() {
        stderr.trim()
    } else {
        stdout.trim()
    };
    if raw.is_empty() {
        "알 수 없는 이유로 커밋에 실패했습니다.".into()
    } else {
        raw.to_string()
    }
}

// ── push ───────────────────────────────────────────────────────────────────────

/// Push HEAD to `origin/<branch>`. `credentials` (username/password) are used
/// for HTTPS remotes via a temporary GIT_ASKPASS script; SSH remotes use the
/// app's existing ssh/key auth and ignore them. When the remote is HTTPS and
/// no credentials are given, the outcome reports `auth_required: true` instead
/// of letting git hang on an interactive prompt.
pub fn push(
    target: &Target,
    branch: Option<&str>,
    credentials: Option<&PushCredential>,
) -> AppResult<PushOutcome> {
    let branch_ref = if let Some(b) = branch {
        b.to_string()
    } else {
        run_at_target(target, ["rev-parse", "--abbrev-ref", "HEAD"])?
            .stdout
            .trim()
            .to_string()
    };
    if branch_ref.is_empty() {
        return Err(AppError::Git("cannot determine current branch".into()));
    }
    let remote_url = run_at_target(target, ["remote", "get-url", "origin"]).ok();
    let https = remote_url
        .as_ref()
        .map(|o| is_https_url(o.stdout.trim()))
        .unwrap_or(false);

    let out = match credentials {
        Some(cred) if https => push_with_askpass(target, &branch_ref, cred)?,
        Some(_) | None if https => {
            // HTTPS + no credentials: don't even try — git would block on a
            // terminal prompt we can't answer.
            return Ok(PushOutcome {
                ok: false,
                pushed_sha: None,
                message: "Git 호스트 로그인이 필요합니다. 푸시할 때 아이디/비밀번호를 입력하세요."
                    .to_string(),
                auth_required: true,
            });
        }
        _ => run_at_target(target, ["push", "origin", &format!("HEAD:{branch_ref}")])?,
    };
    if out.ok() {
        let pushed_sha = out
            .stdout
            .lines()
            .find(|l| l.contains("HEAD ->"))
            .and_then(|l| l.split_whitespace().find(|t| t.contains("..")))
            .map(|t| t.split("..").nth(1).unwrap_or(t).to_string());
        Ok(PushOutcome {
            ok: true,
            pushed_sha,
            message: out.stdout.trim().to_string(),
            auth_required: false,
        })
    } else {
        let mut auth_required = false;
        let mut message = friendly_git_error(&out.stderr);
        if https && is_auth_failure(&out.stderr) {
            auth_required = true;
            message = "Git 호스트 로그인이 실패했거나 저장되지 않았습니다. 아이디/비밀번호를 다시 입력하세요."
                .to_string();
        }
        Ok(PushOutcome {
            ok: false,
            pushed_sha: None,
            message,
            auth_required,
        })
    }
}

pub fn is_https_url(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

pub fn is_auth_failure(stderr: &str) -> bool {
    let e = stderr.to_lowercase();
    [
        "could not read username",
        "could not read password",
        "authentication failed",
        "invalid username or password",
        "http basic: access denied",
        "access denied",
        "requested url returned error: 401",
        "requested url returned error: 403",
        "error: 401",
        "error: 403",
    ]
    .iter()
    .any(|m| e.contains(m))
}

/// Build the GIT_ASKPASS script body. `$1` is the git prompt; the username
/// prompt always contains "Username", everything else is the password.
pub fn askpass_script(user: &str, pass: &str) -> String {
    let esc = |s: &str| s.replace('\'', "'\\''");
    format!(
        "#!/bin/sh\ncase \"$1\" in\n  *Username*|*username*) echo '{}' ;;\n  *) echo '{}' ;;\nesac\n",
        esc(user),
        esc(pass)
    )
}

/// Push over HTTPS with credentials injected via GIT_ASKPASS. The password
/// lives only inside a temporary 0700 script file — never in argv or env.
fn push_with_askpass(
    target: &Target,
    branch: &str,
    cred: &PushCredential,
) -> AppResult<crate::git::GitOutput> {
    let script = askpass_script(&cred.username, &cred.password);
    match target {
        Target::Local(_) => {
            let path = std::env::temp_dir().join(format!("gc-askpass-{}.sh", Uuid::new_v4()));
            write_askpass_local(&path, &script)?;
            let result = crate::git::run_with_env(
                Some(target.path()),
                ["push", "origin", &format!("HEAD:{branch}")],
                &[
                    ("GIT_ASKPASS", path.to_string_lossy().as_ref()),
                    ("GIT_TERMINAL_PROMPT", "0"),
                ],
            );
            let _ = std::fs::remove_file(&path);
            result
        }
        Target::Ssh { .. } => {
            let rel = format!("../.gc-askpass-{}.sh", Uuid::new_v4());
            write_file_at_target(target, &rel, script.as_bytes())?;
            let remote = format!(
                "GIT_ASKPASS='{}' GIT_TERMINAL_PROMPT='0' git -C {} push origin 'HEAD:{}'",
                rel.replace('\'', "'\\''"),
                shell_quote(&target.path().to_string_lossy()),
                branch.replace('\'', "'\\''")
            );
            let result = match target {
                Target::Ssh {
                    user,
                    host,
                    key,
                    password,
                    port,
                    ..
                } => run_ssh_command(user, host, key, password, *port, &remote),
                Target::Local(_) => unreachable!(),
            };
            let cmd = format!("rm -f '{}'", rel.replace('\'', "'\\''"));
            if let Target::Ssh {
                user,
                host,
                key,
                password,
                port,
                ..
            } = target
            {
                let _ = run_ssh_command(user, host, key, password, *port, &cmd);
            }
            result
        }
    }
}

fn write_askpass_local(path: &std::path::Path, script: &str) -> AppResult<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, script)?;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o700);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

// ── pull ───────────────────────────────────────────────────────────────────────

pub fn pull(target: &Target) -> AppResult<PullOutcome> {
    let branch_ref = run_at_target(target, ["rev-parse", "--abbrev-ref", "HEAD"])?;
    let branch = branch_ref.stdout.trim();
    if branch.is_empty() {
        return Err(AppError::Git("cannot determine current branch".into()));
    }
    // Fast-forward when possible; otherwise merge (--no-rebase so a user-level
    // `pull.rebase` config can't change this). Conflicts from the merge are
    // returned to the UI, which routes them into the merge center — the
    // resolver picks up MERGE_HEAD and lets the reviewer resolve them there.
    let out = run_at_target(target, ["pull", "--no-rebase", "origin", branch])?;
    if out.ok() {
        Ok(PullOutcome {
            ok: true,
            message: out.stdout.trim().to_string(),
            conflicted_files: vec![],
        })
    } else {
        let conflicts = list_conflicted_files(target)?;
        if !conflicts.is_empty() {
            return Ok(PullOutcome {
                ok: false,
                message: format!(
                    "충돌 {}개가 발생했습니다. 병합 탭에서 해결하세요.",
                    conflicts.len()
                ),
                conflicted_files: conflicts,
            });
        }
        Ok(PullOutcome {
            ok: false,
            message: friendly_git_error(&out.stderr),
            conflicted_files: vec![],
        })
    }
}

/// Full list of stash entries (`index`, `subject`), e.g. `stash@{0}` with its
/// message — rendered by the work tab's stash modal.
pub fn list_stashes(target: &Target) -> AppResult<Vec<StashEntry>> {
    let out = run_at_target(target, ["stash", "list", "--format=%gd|%gs"])?;
    let mut entries = Vec::new();
    for line in out.stdout.lines() {
        let mut parts = line.splitn(2, '|');
        let index = parts.next().unwrap_or("").trim().to_string();
        let subject = parts.next().unwrap_or("").trim().to_string();
        if index.is_empty() {
            continue;
        }
        entries.push(StashEntry { index, subject });
    }
    Ok(entries)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StashEntry {
    /// Reflog selector like `stash@{0}`.
    pub index: String,
    /// The message captured by `git stash push -m ...` (or the default).
    pub subject: String,
}

fn list_conflicted_files(target: &Target) -> AppResult<Vec<String>> {
    let out = run_at_target(target, ["diff", "--name-only", "--diff-filter=U"])?;
    Ok(out
        .stdout
        .lines()
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .collect())
}

// ── status ─────────────────────────────────────────────────────────────────────

pub fn list_status(target: &Target) -> AppResult<WorkingTreeStatus> {
    let out = run_at_target(
        target,
        [
            "status",
            "--porcelain=v2",
            "--branch",
            "--untracked-files=normal",
        ],
    )?;
    status::parse_status(&out.stdout)
}

// ── changed files ───────────────────────────────────────────────────────────────

pub fn changed_files(target: &Target, scope: StatusScope) -> AppResult<Vec<FileChange>> {
    let full = list_status(target)?;
    match scope {
        StatusScope::Staged => Ok(full.files.into_iter().filter(|f| f.staged).collect()),
        StatusScope::Unstaged => Ok(full.files.into_iter().filter(|f| f.unstaged).collect()),
        StatusScope::Untracked => Ok(full
            .files
            .into_iter()
            .filter(|f| matches!(f.kind, FileChangeKind::Untracked))
            .collect()),
        StatusScope::All => Ok(full.files),
    }
}

// ── diff ───────────────────────────────────────────────────────────────────────

pub fn diff(target: &Target, opts: DiffOpts) -> AppResult<String> {
    let mut args = vec!["diff"];
    if opts.staged {
        args.push("--staged");
    }
    if opts.stat {
        args.push("--stat");
    }
    if let Some(ref ps) = opts.pathspec {
        args.push("--");
        args.push(ps);
    }
    let out = run_at_target(target, &args)?;
    if out.ok() || out.status == 1 {
        Ok(out.stdout)
    } else {
        Err(AppError::Git(out.stderr.trim().to_string()))
    }
}

// ── log ───────────────────────────────────────────────────────────────────────

pub fn list_commits(
    target: &Target,
    branch: &str,
    count: u32,
) -> AppResult<Vec<crate::git::Commit>> {
    let out = run_at_target(
        target,
        &[
            "log",
            "--date=iso-strict",
            &format!("--pretty=format:%H\x1f%s\x1f%an\x1f%aI\x1f%P%n"),
            "-n",
            &count.to_string(),
            branch,
        ],
    )?;
    log::parse_log(&out.stdout)
}

// ── stash ─────────────────────────────────────────────────────────────────────

pub fn stash(target: &Target, action: StashAction) -> AppResult<()> {
    match action {
        StashAction::Save { message } => {
            let mut args: Vec<String> = vec!["stash".into(), "push".into()];
            if let Some(msg) = message {
                args.push("-m".into());
                args.push(msg);
            }
            run_at_target(target, &args.iter().map(|s| s.as_str()).collect::<Vec<_>>())?;
            Ok(())
        }
        StashAction::Pop => {
            run_at_target(target, ["stash", "pop"])?;
            Ok(())
        }
        StashAction::PopIndex(index) => {
            run_at_target(target, ["stash", "pop", &index])?;
            Ok(())
        }
        StashAction::List => {
            run_at_target(target, ["stash", "list"])?;
            Ok(())
        }
        StashAction::Drop => {
            run_at_target(target, ["stash", "drop"])?;
            Ok(())
        }
        StashAction::DropIndex(index) => {
            run_at_target(target, ["stash", "drop", &index])?;
            Ok(())
        }
        StashAction::Clear => {
            run_at_target(target, ["stash", "clear"])?;
            Ok(())
        }
    }
}

// ── branch creation / checkout ─────────────────────────────────────────────────

pub fn create_branch(target: &Target, branch: &str) -> AppResult<()> {
    run_at_target(target, ["checkout", "-b", branch])?;
    Ok(())
}

pub fn checkout_branch(target: &Target, branch: &str) -> AppResult<()> {
    // 브랜치 목록에는 원격 트래킹 이름(origin/…)도 포함되므로 로컬 이름으로 정규화한다.
    // (그대로 쓰면 아래 폴백이 `origin/origin/…`을 만들며 실패한다.)
    let local = branch.strip_prefix("origin/").unwrap_or(branch);
    let out = run_at_target(target, ["checkout", local])?;
    if out.ok() {
        return Ok(());
    }
    // 작업 트리가 더럽거나 미추적 파일이 겹치면 전환 자체가 불가능하다.
    // 그런 경우 원격 폴백을 시도하지 말고 한글로 친절하게 알려준다.
    if dirty_tree_error(&out.stderr) {
        return Err(AppError::Git(DIRTY_TREE_MSG.into()));
    }
    let track_out = run_at_target(
        target,
        ["checkout", "-b", local, &format!("origin/{local}")],
    )?;
    if track_out.ok() {
        Ok(())
    } else if dirty_tree_error(&track_out.stderr) {
        Err(AppError::Git(DIRTY_TREE_MSG.into()))
    } else {
        Err(AppError::Git(format!(
            "checkout failed: {}",
            track_out.stderr.trim()
        )))
    }
}

const DIRTY_TREE_MSG: &str =
    "작업 트리에 커밋되지 않은 변경사항이 있어 브랜치를 전환할 수 없습니다. 변경사항을 커밋하거나 스태시한 뒤 다시 시도하세요.";

fn dirty_tree_error(stderr: &str) -> bool {
    let e = stderr.to_lowercase();
    [
        "would be overwritten",
        "local changes",
        "stash them",
        "untracked working tree files",
    ]
    .iter()
    .any(|m| e.contains(m))
}

// ── Helpers ────────────────────────────────────────────────────────────────────

/// git 의 영어 오류를 사람 말로 바꾼다.
///
/// 한국어 화면에 `fatal: 'origin' does not appear to be a git repository` 같은
/// 영어 5줄이 그대로 뜨면, 처음 git 을 쓰는 사람은 무엇이 잘못됐는지도, 무엇을
/// 해야 하는지도 알 수 없다. 구체적인 원인을 먼저 검사한다 — 원격이 없다는
/// 오류에는 "access rights" 같은 단어가 함께 들어 있어서, 일반적인 인증 실패로
/// 잘못 분류되기 쉽다.
pub fn friendly_git_error(stderr: &str) -> String {
    let s = stderr.trim();

    // ── 원격이 아예 없음 (혼자 만든 로컬 저장소에서 가장 흔하다) ──
    if s.contains("does not appear to be a git repository")
        || s.contains("No such remote")
        || s.contains("'origin' does not appear")
    {
        return "이 저장소에는 원격(origin)이 없어서 푸시할 곳이 없습니다.\n             터미널에서 원격을 한 번 등록하세요:\n  git remote add origin <저장소 주소>"
            .into();
    }
    // ── 아직 커밋이 없음 / 브랜치가 없음 ──
    if s.contains("src refspec") && s.contains("does not match any") {
        return "푸시할 커밋이 없습니다. 먼저 커밋한 뒤 다시 시도하세요.".into();
    }
    if s.contains("has no upstream branch") || s.contains("no upstream configured") {
        return "이 브랜치는 아직 원격에 없습니다. 앱이 자동으로 만들어 주니 다시 시도하세요."
            .into();
    }
    // ── 원격은 있지만 없는 저장소 / 권한 없음 ──
    if s.contains("Repository not found") || s.contains("repository does not exist") {
        return "원격에서 저장소를 찾을 수 없습니다. 저장소 주소와 접근 권한을 확인하세요.".into();
    }
    if s.contains("non-fast-forward") || s.contains("updates were rejected") {
        return "푸시 거부됨: 원격 브랜치가 로컬보다 앞서 있습니다. 먼저 ‘동기화’로 최신 내용을 받은 뒤 다시 푸시하세요.".into();
    }
    if s.contains("failed to push some refs") {
        return "푸시 실패: 원격에 새 변경이 있습니다. 먼저 ‘동기화’로 받은 뒤 다시 푸시하세요."
            .into();
    }
    // ── 네트워크 (인증보다 먼저 — 호스트를 못 찾은 것은 권한 문제가 아니다) ──
    if s.contains("Could not resolve host")
        || s.contains("Connection refused")
        || s.contains("Connection timed out")
        || s.contains("network is unreachable")
        || s.contains("network")
    {
        return "네트워크에 연결할 수 없습니다. 인터넷 연결과 저장소 주소를 확인하세요.".into();
    }
    // ── 인증 ──
    if s.contains("Permission denied")
        || s.contains("permission denied")
        || s.contains("Authentication failed")
        || s.contains("authentication")
        || s.contains("auth")
    {
        return "접근 권한이 없습니다. SSH 키가 등록되어 있는지, 또는 아이디/비밀번호가 맞는지 확인하세요.".into();
    }
    if s.contains("Host key verification failed") {
        return "서버의 SSH 호스트 키를 확인할 수 없습니다. 터미널에서 한 번 접속해 호스트를 신뢰 목록에 추가하세요.".into();
    }
    if s.is_empty() {
        return "알 수 없는 이유로 실패했습니다.".into();
    }
    s.to_string()
}
