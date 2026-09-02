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
