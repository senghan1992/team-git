//! Tauri commands for external tool launching.
use std::process::Command;
use uuid::Uuid;

use crate::config_store;
use crate::error::{AppError, AppResult};

#[tauri::command]
pub fn open_external_tool(repo_id: Uuid, tool_id: String) -> AppResult<()> {
    let cfg = config_store::load()?;
    let repo = cfg
        .repositories
        .iter()
        .find(|r| r.id == repo_id)
        .ok_or_else(|| AppError::RepoNotFound(repo_id.to_string()))?;
    let tool = cfg
        .external_tools
        .iter()
        .find(|t| t.id == tool_id)
        .ok_or_else(|| AppError::Config(format!("tool not found: {}", tool_id)))?;
    if !tool.enabled {
        return Err(AppError::Config(format!("tool {} is disabled", tool_id)));
    }
    // SSH 저장소는 작업 트리가 원격 서버에 있다. `repo.path` 를 그대로 로컬
    // 명령에 넘기면 존재하지 않는 경로를 여는 셈이라, 조용히 엉뚱한 창이 뜨거나
    // 알 수 없는 오류로 끝난다. 왜 안 되는지 분명히 말하고 멈춘다.
    if !repo.ssh_host.is_empty() {
        return Err(AppError::Config(format!(
            "‘{}’은(는) SSH 저장소({}:{})입니다. 작업 트리가 원격 서버에 있어 이 컴퓨터의 도구로 열 수 없습니다.",
            repo.display_name, repo.ssh_host, repo.path
        )));
    }
    let path = &repo.path;
    let cmd_str = tool.command_template.replace("{path}", path);
    let args_str = tool.args_template.replace("{path}", path);

    let mut cmd = Command::new(&cmd_str);
    if !args_str.is_empty() {
        for arg in args_str.split_whitespace() {
            cmd.arg(arg);
        }
    }
    cmd.spawn()
        .map_err(|e| AppError::Internal(format!("failed to spawn {}: {}", tool.label, e)))?;
    Ok(())
}
