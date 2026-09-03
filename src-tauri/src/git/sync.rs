use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::error::{AppError, AppResult};
use crate::git::fetch::fetch_target;
use crate::git::merge::remaining_conflicts;
use crate::git::{fetch_origin, run, run_at_target, Target};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncResult {
    pub conflicted: bool,
    pub files: Vec<String>,
    pub message: String,
}

/// Sync the *current branch* of a repo (local or SSH) with the team's base
/// branch: fetch, advance the local base ref, then merge `origin/<base>` in.
/// This is the "내 브랜치에 main 병합" step of the team workflow.
///
/// Refuses to run while a merge is already in progress — the user must
/// finish that in the Merge Center first (data-safety rule).
pub fn sync_to_base(target: &Target, base: &str, remote: &str) -> AppResult<SyncResult> {
    if crate::git::merge::merge_in_progress(target)? {
        return Err(AppError::Git(
            "이미 진행 중인 병합이 있습니다. 병합 탭에서 먼저 마무리하세요.".into(),
        ));
    }
    // MERGE_HEAD 없이도 unmerged 항목은 남을 수 있다 (스태시 복원 충돌 등).
    // 그대로 진행하면 남의 충돌이 "동기화 충돌"로 둔갑하고, 완료 시 병합
    // 커밋이 아닌 일반 커밋이 되며 실제 동기화는 조용히 누락된다.
    if !remaining_conflicts(target)?.is_empty() {
        return Err(AppError::Git(
            "해결되지 않은 충돌 파일이 있습니다(스태시 복원 충돌 등). 파일을 정리해 스테이징한 뒤 다시 시도하세요."
                .into(),
        ));
    }

    // 1. Fetch from remote (best-effort — may fail if no network).
    let _ = fetch_target(target, remote);

    // 2. Advance the local base ref to match origin/<base> without checkout.
    //    Best-effort; the merge below still targets origin/<base> directly.
    let _ = run_at_target(target, ["fetch", remote, &format!("{base}:{base}")]);

    // 3. Merge origin/<base> into the current branch. On the base branch
    //    itself a plain fast-forward merge is fine (avoids throwaway merge
    //    commits); on feature branches force a merge commit so the sync is
    //    visible in history.
    // symbolic-ref: detached 에서만 실패하고, 커밋 없는 새 저장소(unborn)
    // 에서도 브랜치 이름을 준다 — rev-parse 는 unborn 을 detached 로 오판했다.
    let head = run_at_target(target, ["symbolic-ref", "-q", "--short", "HEAD"])?;
    let current = head.stdout.trim().to_string();
    // detached HEAD에서 merge 커밋을 만들면 어느 브랜치에도 속하지 않아
    // 다음 checkout과 함께 미아가 된다 (reflog 없이는 유실).
    if !head.ok() || current.is_empty() {
        return Err(AppError::Git(
            "지금 브랜치 위에 있지 않습니다(detached HEAD). 브랜치로 전환한 뒤 동기화하세요."
                .into(),
        ));
    }
    let merge_args: Vec<String> = if current == base {
        vec![format!("merge"), format!("{remote}/{base}")]
    } else {
        vec![
            "merge".to_string(),
            "--no-ff".to_string(),
            "--no-edit".to_string(),
            format!("{remote}/{base}"),
        ]
    };
    let out = run_at_target(target, merge_args.iter().map(|s| s.as_str()))?;
    if out.ok() {
        return Ok(SyncResult {
            conflicted: false,
            files: vec![],
            message: out.stdout.trim().to_string(),
        });
    }
    // Conflicts → hand off to the Merge Center; MERGE_HEAD remains set.
    let files = remaining_conflicts(target)?;
    if !files.is_empty() {
        return Ok(SyncResult {
            conflicted: true,
            files,
            message: out.stderr.trim().to_string(),
        });
    }
    // 동기화 버튼은 작업 중에 누르는 일이 가장 흔하다 — git 의 영어 오류
    // ("Your local changes would be overwritten…")를 그대로 보여 주지 않고
    // 다음 행동(커밋/스태시)을 알려 준다.
    if crate::git::ops::dirty_tree_error(&out.stderr) {
        return Err(AppError::Git(
            "커밋하지 않은 변경이 있어 동기화할 수 없습니다. 작업 탭에서 먼저 커밋하거나 스태시한 뒤 다시 시도하세요."
                .into(),
        ));
    }
    Err(AppError::Git(format!(
        "병합 실패: {}",
        crate::git::ops::friendly_git_error(&out.stderr)
    )))
}

/// Full sync: fetch from remote, checkout current branch if needed, then merge
/// origin/<base_branch> into it. Returns SyncResult.
pub fn run_pull_and_merge(
    repo_path: &Path,
    remote: &str,
    base_branch: &str,
    current_branch: Option<&str>,
) -> AppResult<SyncResult> {
    // 1. Fetch from remote (best-effort — may fail if no network).
    let _ = fetch_origin(repo_path);

    // 2. Advance local base_branch ref to match origin/base_branch without checkout.
    //    This is best-effort; if it fails we still proceed to merge.
    let _ = run(
        Some(repo_path),
        ["fetch", remote, &format!("{base_branch}:{base_branch}")],
    );

    // 3. Checkout current branch if specified and not already there.
    if let Some(cb) = current_branch {
        let status_out = run(Some(repo_path), ["rev-parse", "--abbrev-ref", "HEAD"])?;
        let head = status_out.stdout.trim();
        if head != cb {
            let _ = run(Some(repo_path), ["checkout", cb]);
        }
    }

    // 4. Merge origin/<base_branch> into current branch.
    run_merge(repo_path, base_branch)
}

/// Merge `origin/<base>` into the current branch. The caller must ensure the
/// working tree is clean. If conflicts arise, returns the file list and
/// `conflicted: true`.
pub fn run_merge(repo_path: &Path, base: &str) -> AppResult<SyncResult> {
    let _ = fetch_origin(repo_path)?;
    let target = format!("origin/{base}");
    let out = run(Some(repo_path), ["merge", "--no-ff", "--no-edit", &target])?;
    if out.ok() {
        return Ok(SyncResult {
            conflicted: false,
            files: vec![],
            message: out.stdout,
        });
    }
    // Detect conflicts from stderr or unmerged file list.
    if out.stderr.contains("CONFLICT") || !conflicted_files(repo_path)?.is_empty() {
        let files = conflicted_files(repo_path)?;
        return Ok(SyncResult {
            conflicted: true,
            files,
            message: out.stderr,
        });
    }
    // Other failure — surface.
    Err(crate::error::AppError::Git(format!(
        "merge failed: {}",
        out.stderr.trim()
    )))
}

pub fn conflicted_files(repo_path: &Path) -> AppResult<Vec<String>> {
    let out = run(Some(repo_path), ["diff", "--name-only", "--diff-filter=U"])?;
    Ok(out
        .stdout
        .lines()
        .filter(|s| !s.is_empty())
        .map(crate::git::unquote_git_path)
        .collect())
}
