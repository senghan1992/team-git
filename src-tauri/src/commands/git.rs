//! Tauri commands for git operations: add, commit, push, pull, status, diff, log, stash, merge center.
use serde::Serialize;
use uuid::Uuid;

use crate::config_store;
use crate::error::{AppError, AppResult};
use crate::git;
use crate::git::ops::{self, DiffOpts, StashAction};
use crate::git::Target;

pub(crate) fn resolve_target(id: Uuid) -> AppResult<(Target, config_store::Repository)> {
    let cfg = config_store::load()?;
    let repo = cfg
        .repositories
        .iter()
        .find(|r| r.id == id)
        .ok_or_else(|| AppError::RepoNotFound(id.to_string()))?
        .clone();
    let target = Target::from_repo(
        &repo.path,
        &repo.ssh_host,
        &repo.ssh_user,
        &repo.ssh_key_path,
        &repo.ssh_password,
        repo.ssh_port,
    );
    Ok((target, repo))
}

#[tauri::command]
pub async fn list_branches(repo_id: Uuid) -> AppResult<Vec<git::Branch>> {
    let (target, _) = resolve_target(repo_id)?;
    git::list_branches_at(&target)
}

#[tauri::command]
pub async fn list_commits(repo_id: Uuid, branch: String, count: u32) -> AppResult<Vec<git::Commit>> {
    let (target, _) = resolve_target(repo_id)?;
    ops::list_commits(&target, &branch, count)
}

#[tauri::command]
pub async fn status(repo_id: Uuid) -> AppResult<git::WorkingTreeStatus> {
    let (target, repo) = resolve_target(repo_id)?;
    ops::list_status_with_base(&target, &repo.default_branch)
}

#[tauri::command]
pub async fn add_files(repo_id: Uuid, paths: Vec<String>) -> AppResult<git::WorkingTreeStatus> {
    let (target, _) = resolve_target(repo_id)?;
    ops::add(&target, &paths)?;
    ops::list_status(&target)
}

#[tauri::command]
pub async fn commit(repo_id: Uuid, message: String, stage_all: bool) -> AppResult<ops::CommitResult> {
    let (target, _) = resolve_target(repo_id)?;
    ops::commit(&target, &message, stage_all)
}

#[tauri::command]
pub async fn push(
    repo_id: Uuid,
    branch: Option<String>,
    credentials: Option<config_store::PushCredential>,
    save_credential: bool,
) -> AppResult<ops::PushOutcome> {
    let (target, _) = resolve_target(repo_id)?;
    let outcome = ops::push(&target, branch.as_deref(), credentials.as_ref())?;
    if outcome.ok && save_credential {
        if let Some(cred) = credentials {
            config_store::set_push_credential(&repo_id, &cred)?;
        }
    }
    Ok(outcome)
}

#[tauri::command]
pub async fn pull(repo_id: Uuid) -> AppResult<ops::PullOutcome> {
    let (target, _) = resolve_target(repo_id)?;
    ops::pull(&target)
}

#[tauri::command]
pub async fn diff(
    repo_id: Uuid,
    pathspec: Option<String>,
    staged: bool,
    stat: bool,
) -> AppResult<String> {
    let (target, _) = resolve_target(repo_id)?;
    ops::diff(
        &target,
        DiffOpts {
            pathspec,
            staged,
            stat,
        },
    )
}

#[tauri::command]
pub async fn stash(repo_id: Uuid, action: String) -> AppResult<()> {
    let (target, _) = resolve_target(repo_id)?;
    let action = parse_stash_action(&action)?;
    ops::stash(&target, action)
}

#[tauri::command]
pub async fn stash_list(repo_id: Uuid) -> AppResult<Vec<ops::StashEntry>> {
    let (target, _) = resolve_target(repo_id)?;
    ops::list_stashes(&target)
}

fn parse_stash_action(s: &str) -> AppResult<StashAction> {
    if s == "pop" {
        Ok(StashAction::Pop)
    } else if s == "list" {
        Ok(StashAction::List)
    } else if s == "drop" {
        Ok(StashAction::Drop)
    } else if s == "clear" {
        Ok(StashAction::Clear)
    } else if let Some(msg) = s.strip_prefix("save:") {
        Ok(StashAction::Save {
            message: Some(msg.to_string()),
        })
    } else if let Some(idx) = s.strip_prefix("pop:") {
        Ok(StashAction::PopIndex(idx.to_string()))
    } else if let Some(idx) = s.strip_prefix("drop:") {
        Ok(StashAction::DropIndex(idx.to_string()))
    } else if s == "save" {
        Ok(StashAction::Save { message: None })
    } else {
        Err(AppError::Git(format!("unknown stash action: {s}")))
    }
}

#[tauri::command]
pub async fn create_branch(repo_id: Uuid, branch: String) -> AppResult<()> {
    let (target, _) = resolve_target(repo_id)?;
    ops::create_branch(&target, &branch)
}

#[tauri::command]
pub async fn checkout_branch(repo_id: Uuid, branch: String) -> AppResult<()> {
    let (target, _) = resolve_target(repo_id)?;
    ops::checkout_branch(&target, &branch)
}

// ── Merge center commands ───────────────────────────────────────────────────────

/// Snapshot of an in-progress merge — drives the conflict panel banner.
#[derive(Debug, Clone, Serialize)]
pub struct MergeState {
    pub in_progress: bool,
    pub conflicted_files: Vec<String>,
}

pub const MERGE_REMOTE: &str = "origin";

#[tauri::command]
pub async fn fetch_repo(repo_id: Uuid) -> AppResult<String> {
    let (target, _) = resolve_target(repo_id)?;
    git::fetch::fetch_target(&target, MERGE_REMOTE)
}

#[tauri::command]
pub async fn list_pending_branches(
    repo_id: Uuid,
    base: String,
) -> AppResult<Vec<git::merge::PendingBranch>> {
    let (target, _) = resolve_target(repo_id)?;
    let mut pending = git::merge::list_pending_branches(&target, MERGE_REMOTE, &base)?;
    // 다른 병합 대상 브랜치(develop 기준일 때의 release/1.0 등)는 팀원의
    // 작업 브랜치가 아니다 — "병합 대기" 카드로 세우면 관리자가 release 를
    // develop 에 실수로 병합하도록 유도한다.
    let targets = merge_target_branches(&target, &base);
    pending.retain(|b| !targets.contains(&b.short_name));
    Ok(pending)
}

/// `expected_sha`: 관리자가 화면에서 검토한 tip. fetch 후 tip 이 달라졌으면
/// (그 사이 새 push / force-push) 병합하지 않고 새로고침을 요구한다.
#[tauri::command]
pub async fn start_merge(
    repo_id: Uuid,
    branch_ref: String,
    base: String,
    expected_sha: Option<String>,
) -> AppResult<git::merge::MergeOutcome> {
    let (target, _) = resolve_target(repo_id)?;
    git::merge::start_merge(
        &target,
        &branch_ref,
        &base,
        MERGE_REMOTE,
        expected_sha.as_deref(),
    )
}

/// 병합 대기 브랜치의 한 파일이 base와 얼마나 다른지 —
/// `git diff <remote>/<base>...<branch> -- <path>`. 병합 관리자가 파일 이름만
/// 보고 병합을 결정하지 않도록, 카드의 파일 칩에서 실제 변경을 보여 준다.
#[tauri::command]
pub async fn branch_file_diff(
    repo_id: Uuid,
    base: String,
    branch_ref: String,
    path: String,
) -> AppResult<String> {
    let (target, _) = resolve_target(repo_id)?;
    let range = format!("{MERGE_REMOTE}/{base}...{branch_ref}");
    let out = git::run_at_target(&target, ["diff", &range, "--", &path])?;
    if !out.ok() {
        return Err(AppError::Git(format!("diff 실패: {}", out.stderr.trim())));
    }
    Ok(out.stdout)
}

/// 병합이 끝나 base에 완전히 포함된 원격 브랜치 목록 — 정리(삭제) 후보.
///
/// `mine`(본인 작성 브랜치)을 채워 UI 가 삭제 버튼을 그 브랜치에만
/// 활성화하게 한다 — **본인이 만든 브랜치만 삭제할 수 있다.** 신원은
/// 저장소 git 설정(user.name/user.email)을 우선하고, 없으면 로그인 계정을
/// 쓴다 (git 이 기록한 author 와 같은 출처끼리 비교해야 오탐이 없다).
#[tauri::command]
pub async fn list_merged_remote_branches(
    repo_id: Uuid,
    base: String,
) -> AppResult<Vec<git::merge::MergedRemoteBranch>> {
    let (target, _) = resolve_target(repo_id)?;
    let mut merged = git::merge::list_merged_remote_branches(&target, MERGE_REMOTE, &base)?;
    // 병합 대상 브랜치(develop, release/1.0 …)는 다른 base의 조상이어도
    // 정리 후보가 아니다 — 팀의 합류 지점이지 작업 브랜치가 아니다.
    let targets = merge_target_branches(&target, &base);
    merged.retain(|b| !targets.contains(&b.short_name));
    let (name, email) = current_user_identity(&target);
    for b in &mut merged {
        b.mine = git::merge::identity_matches(
            &b.author,
            &b.author_email,
            name.as_deref(),
            email.as_deref(),
        );
    }
    Ok(merged)
}

/// 현재 사용자의 (이름, 이메일) — 저장소 git 설정 우선, 없으면 로그인 계정.
/// 둘 다 없으면 (None, None) — 그 경우 어떤 브랜치도 "내 것"이 아니다.
fn current_user_identity(target: &git::Target) -> (Option<String>, Option<String>) {
    let session = config_store::load()
        .ok()
        .and_then(|c| c.session)
        .map(|s| (s.user.name, s.user.email));
    git::merge::current_git_identity(
        target,
        session.as_ref().map(|(n, e)| (n.as_str(), e.as_str())),
    )
}

/// 병합이 끝난 원격 브랜치를 origin에서 삭제한다.
///
/// **본인이 만든 브랜치만 삭제할 수 있다** — 작성자가 현재 사용자(저장소
/// git 설정 user.name/user.email 또는 로그인 계정)와 일치하지 않으면 거부하고,
/// 신원을 확인할 수 없어도 거부한다(모르면 삭제를 허락하지 않는다).
///
/// `credentials` 는 HTTPS 원격(Git 호스트) 삭제에 쓸 로그인 정보다 — 없으면
/// 결과의 `auth_required` 가 true 로 돌아와 UI 가 아이디/비밀번호 모달을
/// 띄운다(푸시와 같은 계약). 성공하면서 `save_credential` 이면 다음부터
/// 자동 입력되도록 설정에 보관한다.
#[tauri::command]
pub async fn delete_remote_branch(
    repo_id: Uuid,
    base: String,
    branch: String,
    credentials: Option<config_store::PushCredential>,
    save_credential: bool,
) -> AppResult<git::merge::DeleteBranchOutcome> {
    let (target, _) = resolve_target(repo_id)?;
    // .gpconfig의 병합 대상 브랜치는 어떤 경우에도 지우지 않는다.
    if merge_target_branches(&target, &base).contains(&branch) {
        return Err(AppError::Git(format!(
            "{branch}은(는) 병합 대상 브랜치라 삭제할 수 없습니다."
        )));
    }
    // ── 본인 브랜치만 삭제 가능 ──
    let (name, email) = current_user_identity(&target);
    if name.is_none() && email.is_none() {
        return Err(AppError::Git(
            "본인 브랜치인지 확인할 수 없습니다 — 저장소의 git 사용자 설정(user.name)을 확인하거나 로그인 상태를 유지하세요."
                .into(),
        ));
    }
    let author_out = git::run_at_target(
        &target,
        [
            "for-each-ref",
            &format!("refs/remotes/{MERGE_REMOTE}/{branch}"),
            "--format=%(authorname)%09%(authoremail)",
        ],
    )?;
    if !author_out.ok() || author_out.stdout.trim().is_empty() {
        return Err(AppError::Git(format!(
            "{branch} 브랜치를 찾을 수 없습니다 — 목록을 새로고침하세요."
        )));
    }
    let mut parts = author_out.stdout.trim().splitn(2, '\t');
    let author = parts.next().unwrap_or("").to_string();
    let author_email = parts.next().unwrap_or("").to_string();
    if !git::merge::identity_matches(&author, &author_email, name.as_deref(), email.as_deref()) {
        let who = if author.trim().is_empty() {
            "다른 팀원".to_string()
        } else {
            format!("{author}님")
        };
        return Err(AppError::Git(format!(
            "{branch} 브랜치는 {who}이(가) 만든 브랜치라 삭제할 수 없습니다 — 본인이 만든 브랜치만 삭제할 수 있습니다."
        )));
    }

    let outcome = git::merge::delete_remote_branch(
        &target,
        MERGE_REMOTE,
        &base,
        &branch,
        credentials.as_ref(),
    )?;
    if outcome.ok && save_credential {
        if let Some(cred) = credentials {
            config_store::set_push_credential(&repo_id, &cred)?;
        }
    }
    Ok(outcome)
}

/// `.gpconfig`의 병합 대상 브랜치 + 기본 base. 설정을 못 읽어도 base는 지킨다.
fn merge_target_branches(target: &git::Target, base: &str) -> Vec<String> {
    let mut out = vec![base.to_string()];
    if let Ok((cfg, exists)) = crate::gpconfig::read_config_effective(target, base, MERGE_REMOTE) {
        if exists {
            for t in &cfg.merge_targets {
                if !out.contains(t) {
                    out.push(t.clone());
                }
            }
            if !cfg.default_base_branch.is_empty() && !out.contains(&cfg.default_base_branch) {
                out.push(cfg.default_base_branch.clone());
            }
        }
    }
    out
}

/// 로컬 base가 origin/<base>보다 앞선 커밋 수 — 병합 커밋은 만들어졌는데
/// push가 실패/취소된 상태를 UI가 재시작 후에도 알아볼 수 있게 한다.
#[tauri::command]
pub async fn base_unpushed_count(repo_id: Uuid, base: String) -> AppResult<u32> {
    let (target, _) = resolve_target(repo_id)?;
    git::merge::base_unpushed_count(&target, MERGE_REMOTE, &base)
}

#[tauri::command]
pub async fn merge_state(repo_id: Uuid) -> AppResult<MergeState> {
    let (target, _) = resolve_target(repo_id)?;
    let in_progress = git::merge::merge_in_progress(&target)?;
    let files = if in_progress {
        git::merge::remaining_conflicts(&target)?
    } else {
        Vec::new()
    };
    Ok(MergeState {
        in_progress,
        conflicted_files: files,
    })
}

#[tauri::command]
pub async fn conflict_detail(repo_id: Uuid, path: String) -> AppResult<git::merge::ConflictDetail> {
    let (target, _) = resolve_target(repo_id)?;
    git::merge::conflict_detail(&target, &path)
}

#[tauri::command]
pub async fn resolve_conflict(
    repo_id: Uuid,
    path: String,
    resolution: git::merge::Resolution,
) -> AppResult<Vec<String>> {
    let (target, _) = resolve_target(repo_id)?;
    git::merge::resolve_conflict(&target, &path, &resolution)
}

#[tauri::command]
pub async fn abort_merge(repo_id: Uuid) -> AppResult<()> {
    let (target, _) = resolve_target(repo_id)?;
    git::merge::abort_merge(&target)
}

#[tauri::command]
pub async fn complete_merge(
    repo_id: Uuid,
    message: Option<String>,
) -> AppResult<git::merge::MergeOutcome> {
    let (target, _) = resolve_target(repo_id)?;
    // 메시지가 없으면 팀 컨벤션 "<브랜치> 브렌치 병합"으로 채운다
    // (병합 센터 수동 해결 후 완료할 때도 같은 문구가 남도록).
    let message = match message {
        Some(m) if !m.trim().is_empty() => Some(m),
        _ => Some(merge_head_branch_name(&target).map(|b| format!("{b} 브렌치 병합"))?),
    };
    git::merge::complete_merge(&target, message.as_deref())
}

/// MERGE_HEAD가 가리키는 브랜치 이름(remote 우선). 못 찾으면 병합 대상으로 표기.
fn merge_head_branch_name(target: &git::Target) -> AppResult<String> {
    let sha = git::run_at_target(target, ["rev-parse", "MERGE_HEAD"])?;
    let mut name = String::new();
    if sha.ok() {
        let out = git::run_at_target(
            target,
            [
                "for-each-ref",
                "--points-at",
                sha.stdout.trim(),
                "--format=%(refname:short)",
            ],
        )?;
        if out.ok() {
            for line in out.stdout.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if line.starts_with("origin/") {
                    name = line.strip_prefix("origin/").unwrap_or(&line).to_string();
                    break;
                }
                if name.is_empty() {
                    name = line.to_string();
                }
            }
        }
    }
    if name.is_empty() {
        name = "(병합 대상)".into();
    }
    Ok(name)
}

/// 병합 탭 상단의 "최근 N일 병합 흐름" — base 로 무엇이 언제 합류했고,
/// 어떤 브랜치가 아직 열려 있는지.
#[tauri::command]
pub async fn merge_timeline(
    repo_id: Uuid,
    base: String,
    days: u32,
) -> AppResult<git::timeline::MergeTimeline> {
    let (target, _) = resolve_target(repo_id)?;
    let mut tl = git::timeline::merge_timeline(&target, MERGE_REMOTE, &base, days)?;
    // 다른 병합 대상(develop 기준일 때의 release/1.0 등)은 팀원의 작업
    // 브랜치가 아니다 — "병합 대기" 레인으로 세우지 않는다.
    let targets = merge_target_branches(&target, &base);
    tl.open.retain(|b| !targets.contains(&b.name));
    Ok(tl)
}
