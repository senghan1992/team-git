//! Tauri commands for the one-click auto-merge flow and branch syncing.
use std::str::FromStr;
use std::time::Duration;
use uuid::Uuid;

use crate::ai::ConflictContext;
use crate::commands::git::resolve_target;
use crate::error::{AppError, AppResult};
use crate::git::{self, auto, sync};

/// One-click "AI 자동 병합": resolve every conflicted file (AI with a
/// deterministic fallback) and, when all are clean, commit the merge.
///
/// Async because the per-file AI call blocks on the HTTP client; the engine
/// itself is sync and runs on a blocking thread.
#[tauri::command]
pub async fn merge_auto_resolve(
    repo_id: Uuid,
    binary_strategy: Option<String>,
) -> AppResult<auto::AutoResolveReport> {
    let (target, _) = resolve_target(repo_id)?;
    let strategy = match binary_strategy {
        Some(s) => auto::SideChoice::from_str(&s)?,
        None => auto::SideChoice::Theirs,
    };
    let opts = auto::AutoResolveOptions {
        binary_strategy: strategy,
    };
    // Capture the runtime handle before moving into the blocking task.
    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        auto::auto_resolve_merge(&target, &opts, |ctx| {
            // The ai module is async (reqwest); bridge with block_on — it is
            // safe here because we are on a dedicated blocking thread and
            // share the caller's runtime with multi-threaded workers.
            let c = ConflictContext {
                file_path: ctx.path.clone(),
                base: ctx.base.clone(),
                ours: ctx.ours.clone(),
                theirs: ctx.theirs.clone(),
            };
            handle
                .block_on(async {
                    tokio::time::timeout(Duration::from_secs(90), crate::ai::suggest(&c)).await
                })
                .map_err(|_| AppError::Git("AI 호출 시간 초과".into()))?
        })
    })
    .await
    .map_err(|e| AppError::Internal(format!("auto resolve task: {e}")))?
}

#[tauri::command]
pub fn merge_backup_list(repo_id: Uuid) -> AppResult<Vec<auto::BackupEntry>> {
    let (target, _) = resolve_target(repo_id)?;
    auto::list_backups(&target)
}

#[tauri::command]
pub fn merge_backup_restore(repo_id: Uuid, backup_id: String) -> AppResult<usize> {
    let (target, _) = resolve_target(repo_id)?;
    auto::restore_backup(&target, &backup_id)
}

/// Sync the current branch with the team base branch (fetch + merge
/// origin/<base>). Conflicts leave MERGE_HEAD set for the Merge Center.
#[tauri::command]
pub fn sync_branch(repo_id: Uuid, base: String) -> AppResult<sync::SyncResult> {
    let (target, _) = resolve_target(repo_id)?;
    git::sync_to_base(&target, &base, "origin")
}
