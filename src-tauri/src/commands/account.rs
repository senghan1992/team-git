//! Tauri commands for login accounts.
//!
//! Identities live in the team server's `users` table; this layer is a thin
//! pass-through to `crate::accounts`. Only `account_current` is local — it
//! reads the cached session so startup never waits on the network.
use crate::accounts;
use crate::config_store::Account;
use crate::error::AppResult;

#[tauri::command]
pub async fn account_register(
    name: String,
    email: String,
    username: String,
    password: String,
) -> AppResult<Account> {
    accounts::register(&name, &email, &username, &password).await
}

#[tauri::command]
pub async fn account_login_by_password(username: String, password: String) -> AppResult<Account> {
    accounts::login(&username, &password).await
}

#[tauri::command]
pub async fn account_logout() -> AppResult<()> {
    accounts::logout().await
}

/// The signed-in user from the local cache — no network call.
#[tauri::command]
pub fn account_current() -> AppResult<Option<Account>> {
    crate::config_store::active_account()
}

/// Re-read the signed-in user from the server. Being offline returns the cached
/// copy; only a rejected token signs the user out.
#[tauri::command]
pub async fn account_refresh() -> AppResult<Option<Account>> {
    accounts::refresh().await
}

#[tauri::command]
pub async fn account_update_profile(
    name: Option<String>,
    email: Option<String>,
) -> AppResult<Account> {
    accounts::update_profile(name.as_deref(), email.as_deref()).await
}

#[tauri::command]
pub async fn account_change_password(
    current_password: String,
    new_password: String,
) -> AppResult<()> {
    accounts::change_password(&current_password, &new_password).await
}

/// Delete my own account on the server, then sign out locally.
#[tauri::command]
pub async fn account_delete_self() -> AppResult<()> {
    accounts::delete_self().await
}

/// Search the team's user directory (name / id / email). Needs 2+ characters.
#[tauri::command]
pub async fn account_search(query: String) -> AppResult<Vec<Account>> {
    accounts::search(&query).await
}
