//! Tauri commands for the optional AI conflict suggester.
//!
//! The credentials live in `AiConfig` (loaded from the global config store by
//! `ai::suggest`), so the command takes no auth-related arguments — only the
//! conflict body itself.
use crate::ai::{self, ConflictContext};
use crate::error::AppResult;

#[tauri::command]
pub async fn ai_suggest_resolution(
    file_path: String,
    base: Option<String>,
    ours: String,
    theirs: String,
) -> AppResult<String> {
    let ctx = ConflictContext {
        file_path,
        base,
        ours,
        theirs,
    };
    ai::suggest(&ctx).await
}
