//! Tauri commands for login accounts (identity) — the app's people registry.
//!
//! Login is local-first: identities live in the app config, and per-project
//! membership is carried by the repo's own `.gpconfig` file (matched by email),
//! so it works offline and travels with the repo.
use uuid::Uuid;

use crate::config_store::{self, Account};
use crate::error::AppResult;

#[tauri::command]
pub fn account_register(
    name: String,
    email: String,
    username: Option<String>,
    password: Option<String>,
) -> AppResult<Account> {
    config_store::register_account(&name, &email, username.as_deref(), password.as_deref())
}

#[tauri::command]
pub fn account_login_by_password(username: String, password: String) -> AppResult<Account> {
    config_store::login_by_password(&username, &password)
}

#[tauri::command]
pub fn account_list() -> AppResult<Vec<Account>> {
    config_store::list_accounts()
}

#[tauri::command]
pub fn account_delete(id: Uuid) -> AppResult<()> {
    config_store::delete_account(&id)
}

#[tauri::command]
pub fn account_login(id: Uuid) -> AppResult<Account> {
    config_store::login_account(&id)
}

#[tauri::command]
pub fn account_logout() -> AppResult<()> {
    config_store::logout_account()
}

#[tauri::command]
pub fn account_current() -> AppResult<Option<Account>> {
    config_store::active_account()
}
