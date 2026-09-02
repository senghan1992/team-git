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
pub fn list_branches(repo_id: Uuid) -> AppResult<Vec<git::Branch>> {
    let (target, _) = resolve_target(repo_id)?;
    git::list_branches_at(&target)
}

#[tauri::command]
pub fn list_commits(repo_id: Uuid, branch: String, count: u32) -> AppResult<Vec<git::Commit>> {
    let (target, _) = resolve_target(repo_id)?;
    ops::list_commits(&target, &branch, count)
}

#[tauri::command]
pub fn status(repo_id: Uuid) -> AppResult<git::WorkingTreeStatus> {
    let (target, _) = resolve_target(repo_id)?;
    ops::list_status(&target)
}

#[tauri::command]
pub fn add_files(repo_id: Uuid, paths: Vec<String>) -> AppResult<git::WorkingTreeStatus> {
    let (target, _) = resolve_target(repo_id)?;
    ops::add(&target, &paths)?;
    ops::list_status(&target)
}

#[tauri::command]
pub fn commit(repo_id: Uuid, message: String, stage_all: bool) -> AppResult<ops::CommitResult> {
    let (target, _) = resolve_target(repo_id)?;
    ops::commit(&target, &message, stage_all)
}

#[tauri::command]
pub fn push(
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
pub fn pull(repo_id: Uuid) -> AppResult<ops::PullOutcome> {
    let (target, _) = resolve_target(repo_id)?;
    ops::pull(&target)
}

#[tauri::command]
pub fn diff(
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
pub fn stash(repo_id: Uuid, action: String) -> AppResult<()> {
    let (target, _) = resolve_target(repo_id)?;
    let action = parse_stash_action(&action)?;
    ops::stash(&target, action)
}

#[tauri::command]
pub fn stash_list(repo_id: Uuid) -> AppResult<Vec<ops::StashEntry>> {
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
pub fn create_branch(repo_id: Uuid, branch: String) -> AppResult<()> {
    let (target, _) = resolve_target(repo_id)?;
    ops::create_branch(&target, &branch)
}

#[tauri::command]
pub fn checkout_branch(repo_id: Uuid, branch: String) -> AppResult<()> {
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
pub fn fetch_repo(repo_id: Uuid) -> AppResult<String> {
    let (target, _) = resolve_target(repo_id)?;
    git::fetch::fetch_target(&target, MERGE_REMOTE)
}

#[tauri::command]
pub fn list_pending_branches(
    repo_id: Uuid,
    base: String,
) -> AppResult<Vec<git::merge::PendingBranch>> {
    let (target, _) = resolve_target(repo_id)?;
    git::merge::list_pending_branches(&target, MERGE_REMOTE, &base)
}

#[tauri::command]
pub fn start_merge(
    repo_id: Uuid,
    branch_ref: String,
    base: String,
) -> AppResult<git::merge::MergeOutcome> {
    let (target, _) = resolve_target(repo_id)?;
    git::merge::start_merge(&target, &branch_ref, &base, MERGE_REMOTE)
}

#[tauri::command]
pub fn merge_state(repo_id: Uuid) -> AppResult<MergeState> {
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
pub fn conflict_detail(repo_id: Uuid, path: String) -> AppResult<git::merge::ConflictDetail> {
    let (target, _) = resolve_target(repo_id)?;
    git::merge::conflict_detail(&target, &path)
}

#[tauri::command]
pub fn resolve_conflict(
    repo_id: Uuid,
    path: String,
    resolution: git::merge::Resolution,
) -> AppResult<Vec<String>> {
    let (target, _) = resolve_target(repo_id)?;
    git::merge::resolve_conflict(&target, &path, &resolution)
}

#[tauri::command]
pub fn abort_merge(repo_id: Uuid) -> AppResult<()> {
    let (target, _) = resolve_target(repo_id)?;
    git::merge::abort_merge(&target)
}

#[tauri::command]
pub fn complete_merge(
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
            let mut remote = String::new();
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
