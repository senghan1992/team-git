//! Tauri commands for the per-project collaboration config (`.gpconfig`).
use serde::Serialize;
use uuid::Uuid;

use crate::commands::git::{resolve_target, MERGE_REMOTE};
use crate::error::AppResult;
use crate::gpconfig::{self, CommitOutcome, ProjectConfig};
use crate::{config_store, gpconfig::member_from_account};

#[derive(Debug, Clone, Serialize)]
pub struct ProjectConfigResult {
    pub exists: bool,
    pub config: ProjectConfig,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectConfigSaveResult {
    pub config: ProjectConfig,
    /// Result of the auto `git add` + `commit` of `.gpconfig` when requested.
    pub commit: Option<CommitOutcome>,
}

#[tauri::command]
pub async fn project_config_get(repo_id: Uuid) -> AppResult<ProjectConfigResult> {
    let (target, repo) = resolve_target(repo_id)?;
    // 작업 브랜치에 .gpconfig 사본이 없어도 팀 규칙(병합 관리자 등)은 보여야
    // 한다 — 없으면 병합 관리자 미지정으로 읽혀 팀원에게 관리자 화면이 뜬다.
    let (config, exists) =
        gpconfig::read_config_effective(&target, &repo.default_branch, MERGE_REMOTE)?;
    Ok(ProjectConfigResult { exists, config })
}

#[tauri::command]
pub async fn project_config_set(
    repo_id: Uuid,
    config: ProjectConfig,
    auto_commit: bool,
) -> AppResult<ProjectConfigSaveResult> {
    let (target, repo) = resolve_target(repo_id)?;
    // 프로젝트 설정을 저장하는 사람도 구성원으로 자동 포함한다 (로그인 상태일 때).
    let mut config = config;
    if let Some(me) = config_store::active_account()? {
        let email = me.email.trim().to_lowercase();
        if !config
            .members
            .iter()
            .any(|m| m.email.trim().to_lowercase() == email)
        {
            config.members.push(member_from_account(
                &me.id.to_string(),
                &me.name,
                &me.email,
                "member",
            ));
        }
    }
    if config.default_base_branch.is_empty() {
        config.default_base_branch = repo.default_branch.clone();
    }
    let config = gpconfig::save_config(&target, &config)?;
    let commit = if auto_commit {
        Some(gpconfig::commit_config(&target)?)
    } else {
        None
    };
    Ok(ProjectConfigSaveResult { config, commit })
}

#[tauri::command]
pub async fn project_config_commit(repo_id: Uuid) -> AppResult<CommitOutcome> {
    let (target, _) = resolve_target(repo_id)?;
    gpconfig::commit_config(&target)
}

#[tauri::command]
pub fn push_credentials_list(
) -> AppResult<std::collections::HashMap<String, config_store::PushCredential>> {
    config_store::list_push_credentials()
}

#[tauri::command]
pub fn push_credential_set(repo_id: Uuid, username: String, password: String) -> AppResult<()> {
    config_store::set_push_credential(
        &repo_id,
        &config_store::PushCredential { username, password },
    )
}

#[tauri::command]
pub fn push_credential_delete(repo_id: Uuid) -> AppResult<()> {
    config_store::delete_push_credential(&repo_id)
}
