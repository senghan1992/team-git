//! Persistent app config — repositories, projects, SSH profiles, external tools.
//!
//! Stored at the OS-conventional config dir under `com.gitcompanion.app/`.
use std::fs;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::peer::PeerConfig;

pub const APP_DIR: &str = "com.gitcompanion.app";
pub const CONFIG_FILE: &str = "config.json";
pub const CURRENT_SCHEMA: u32 = 8;
// v8: Locks the app behind login and adds username+password login with two
// seeded demo accounts (`test`/`test`, `test2`/`test2`) for trying team features.
// v7: Added `accounts`, `active_account_id`, `push_credentials` (existing
// fields use `#[serde(default)]`, so no explicit migration step is required).

// ── SSH ────────────────────────────────────────────────────────────────────────

fn default_string() -> String {
    String::new()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshProfile {
    #[serde(default = "default_string")]
    pub default_user: String,
    #[serde(default = "default_string")]
    pub default_key_path: String,
    /// Optional password for user/password auth; empty = key-based.
    #[serde(default = "default_string")]
    pub default_password: String,
    #[serde(default = "default_string")]
    pub default_host: String,
    #[serde(default = "default_string")]
    pub connect_timeout: String,
    /// SSH port (default 22).
    #[serde(default = "default_profile_ssh_port")]
    pub default_port: u16,
}

fn default_profile_ssh_port() -> u16 {
    22
}

impl Default for SshProfile {
    fn default() -> Self {
        Self {
            default_user: String::new(),
            default_key_path: String::new(),
            default_password: String::new(),
            default_host: String::new(),
            connect_timeout: "5".into(),
            default_port: 22,
        }
    }
}

// ── Repository ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repository {
    pub id: Uuid,
    pub path: String,
    pub display_name: String,
    /// Seeded from `git symbolic-ref --short HEAD` at registration.
    #[serde(default)]
    pub default_branch: String,
    /// The branch the user has selected for this project.
    #[serde(default)]
    pub working_branch: String,
    /// SSH host from which the project was discovered; empty for local-only repos.
    #[serde(default)]
    pub ssh_host: String,
    /// SSH user used to connect to the remote host.
    #[serde(default)]
    pub ssh_user: String,
    /// Absolute path to the identity file; empty = use ~/.ssh/config.
    #[serde(default)]
    pub ssh_key_path: String,
    /// Optional password for user/password auth. Empty = key-based.
    /// Stored in the local config file; prefer SSH keys when possible.
    #[serde(default)]
    pub ssh_password: String,
    /// Captured at registration for verification ping.
    #[serde(default)]
    pub ed25519_fingerprint: String,
    /// SSH port (default 22).
    #[serde(default = "default_repo_ssh_port")]
    pub ssh_port: u16,
    /// Remote origin URL (e.g. `git@host:path/to/repo.git`).
    #[serde(default)]
    pub remote_url: String,
    pub created_at: DateTime<Utc>,
}

fn default_repo_ssh_port() -> u16 {
    22
}

// ── External tools ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExternalTool {
    pub id: String,
    pub label: String,
    /// Command template; `{path}` is replaced with the repo path.
    pub command_template: String,
    /// Args template; `{path}` is replaced with the repo path.
    pub args_template: String,
    #[serde(default)]
    pub enabled: bool,
}

impl Default for ExternalTool {
    fn default() -> Self {
        Self {
            id: String::new(),
            label: String::new(),
            command_template: String::new(),
            args_template: String::new(),
            enabled: true,
        }
    }
}

// ── AI ────────────────────────────────────────────────────────────────────────

/// OpenAI-compatible `/chat/completions` settings for the optional conflict
/// suggester. Default is **disabled** — the user must opt in from Settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub model: String,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: String::new(),
            api_key: String::new(),
            model: String::new(),
        }
    }
}

// ── Project ────────────────────────────────────────────────────────────────────

/// A team project owned by this device. Stored locally and mirrored on the backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub display_name: String,
    /// Join code for other devices to link in.
    pub join_code: String,
    /// "owner" | "member"
    pub role: String,
}

impl Default for Project {
    fn default() -> Self {
        Self {
            id: String::new(),
            display_name: String::new(),
            join_code: String::new(),
            role: "owner".into(),
        }
    }
}

// ── Accounts & push credentials ───────────────────────────────────────────

/// A registered human identity (login). Match with `.gpconfig` members by email.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Account {
    pub id: Uuid,
    pub name: String,
    pub email: String,
    /// Login id (lowercase). None for legacy accounts registered before v8.
    #[serde(default)]
    pub username: Option<String>,
    /// SHA-256(password) hash: `sha256("git-companion::" + username + ":" + password)`.
    #[serde(default)]
    pub password_hash: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Password hashing scheme shared with the dev bridge (`dev/git-bridge.ts`).
pub fn hash_password(username: &str, password: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(format!("git-companion::{username}:{password}").as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Built-in demo accounts so anyone can try the app without registering.
/// Inserted once per config; dedup is by username, so ids only need to be local.
pub const SEED_ACCOUNTS: &[(&str, &str, &str, &str)] = &[
    ("test", "test", "테스트 1", "test@example.com"),
    ("test2", "test2", "테스트 2", "test2@example.com"),
];

/// Insert missing seed accounts (username `test` / `test2`) into the settings.
pub fn ensure_seed_accounts(cfg: &mut AppSettings) {
    for (username, password, name, email) in SEED_ACCOUNTS {
        let exists = cfg
            .accounts
            .iter()
            .any(|a| a.username.as_deref().map(|u| u.to_lowercase()) == Some(username.to_string()));
        if exists {
            continue;
        }
        let id = Uuid::new_v4();
        cfg.accounts.push(Account {
            id,
            name: name.to_string(),
            email: email.to_string(),
            username: Some(username.to_string()),
            password_hash: Some(hash_password(username, password)),
            created_at: Utc::now(),
        });
    }
}

/// Git-host credentials saved per repo — auto-filled in the push modal.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PushCredential {
    pub username: String,
    pub password: String,
}

// ── AppSettings ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub schema_version: u32,
    #[serde(default)]
    pub repositories: Vec<Repository>,
    /// Team projects owned / joined by this device.
    #[serde(default)]
    pub projects: Vec<Project>,
    /// External tool launchers.
    #[serde(default)]
    pub external_tools: Vec<ExternalTool>,
    #[serde(default)]
    pub ssh_profile: SshProfile,
    #[serde(default)]
    pub peer: PeerConfig,
    /// Optional AI conflict-resolver settings (disabled by default).
    #[serde(default)]
    pub ai: AiConfig,
    /// Registered people (login identities).
    #[serde(default)]
    pub accounts: Vec<Account>,
    /// Currently logged-in account.
    #[serde(default)]
    pub active_account_id: Option<String>,
    /// Saved git-host push credentials keyed by repository id.
    #[serde(default)]
    pub push_credentials: std::collections::HashMap<String, PushCredential>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA,
            repositories: vec![],
            projects: vec![],
            external_tools: vec![
                ExternalTool {
                    id: "code".into(),
                    label: "VS Code".into(),
                    command_template: "code".into(),
                    args_template: "{path}".into(),
                    enabled: true,
                },
                ExternalTool {
                    id: "cursor".into(),
                    label: "Cursor".into(),
                    command_template: "cursor".into(),
                    args_template: "{path}".into(),
                    enabled: true,
                },
                ExternalTool {
                    id: "sublime".into(),
                    label: "Sublime Text".into(),
                    command_template: "subl".into(),
                    args_template: "{path}".into(),
                    enabled: true,
                },
                ExternalTool {
                    id: "gnome-terminal".into(),
                    label: "GNOME Terminal".into(),
                    command_template: "gnome-terminal".into(),
                    args_template: "--working-directory={path}".into(),
                    enabled: true,
                },
                ExternalTool {
                    id: "xterm".into(),
                    label: "XTerm".into(),
                    command_template: "xterm".into(),
                    args_template: r#"-e "cd {path} && bash""#.into(),
                    enabled: true,
                },
                ExternalTool {
                    id: "tmux".into(),
                    label: "Tmux".into(),
                    command_template: "tmux".into(),
                    args_template: "new-session -c {path}".into(),
                    enabled: true,
                },
            ],
            ssh_profile: SshProfile::default(),
            peer: PeerConfig::default(),
            ai: AiConfig::default(),
            accounts: vec![],
            active_account_id: None,
            push_credentials: std::collections::HashMap::new(),
        }
    }
}

// ── Accounts ───────────────────────────────────────────────────────────────────

pub fn list_accounts() -> AppResult<Vec<Account>> {
    Ok(load()?.accounts)
}

/// Register a new human identity. Email must be unique.
pub fn register_account(
    name: &str,
    email: &str,
    username: Option<&str>,
    password: Option<&str>,
) -> AppResult<Account> {
    let name = name.trim();
    let email = email.trim().to_lowercase();
    if name.is_empty() || email.is_empty() {
        return Err(AppError::Config("이름과 이메일을 입력하세요.".into()));
    }
    if !email.contains('@') {
        return Err(AppError::Config("올바른 이메일 주소를 입력하세요.".into()));
    }
    let username = match username.map(|u| u.trim().to_lowercase()) {
        Some(u) if u.is_empty() => None,
        other => other,
    };
    let password_hash = match (username.as_deref(), password.map(str::trim)) {
        (Some(uname), _) if uname.is_empty() => None,
        (Some(uname), Some(pw)) if !pw.is_empty() => {
            let valid = uname.len() <= 32
                && uname.chars().all(|c| {
                    c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_' || c == '-'
                });
            if !valid {
                return Err(AppError::Config(
                    "아이디는 영문/숫자/._- 만 사용하고 1~32자로 입력하세요.".into(),
                ));
            }
            if pw.len() < 4 {
                return Err(AppError::Config("비밀번호는 4자 이상 입력하세요.".into()));
            }
            Some(hash_password(uname, pw))
        }
        _ => None,
    };
    let mut cfg = load()?;
    if cfg.accounts.iter().any(|a| a.email == email) {
        return Err(AppError::Config(format!(
            "{email}은(는) 이미 등록된 이메일입니다."
        )));
    }
    if let Some(uname) = username.as_deref() {
        if !uname.is_empty()
            && cfg
                .accounts
                .iter()
                .any(|a| a.username.as_deref().map(|u| u.to_lowercase()).as_deref() == Some(uname))
        {
            return Err(AppError::Config(format!(
                "{uname}은(는) 이미 사용 중인 아이디입니다."
            )));
        }
    }
    let account = Account {
        id: Uuid::new_v4(),
        name: name.to_string(),
        email: email.clone(),
        username,
        password_hash,
        created_at: Utc::now(),
    };
    cfg.accounts.push(account.clone());
    cfg.active_account_id = Some(account.id.to_string());
    save(&cfg)?;
    Ok(account)
}

/// Login with username (or email) + password. Errors are intentionally generic
/// so the login form cannot be used to enumerate accounts.
pub fn login_by_password(username: &str, password: &str) -> AppResult<Account> {
    let id = username.trim().to_lowercase();
    let mut cfg = load()?;
    let err = || AppError::Config("아이디 또는 비밀번호가 올바르지 않습니다.".into());
    let candidate = cfg.accounts.iter().find(|a| {
        a.username.as_deref().map(|u| u.to_lowercase()).as_deref() == Some(id.as_str())
            || (id.contains('@') && a.email == id)
    });
    let Some(account) = candidate else {
        return Err(err());
    };
    let Some(hash) = account.password_hash.as_deref() else {
        return Err(err());
    };
    let uname = account.username.as_deref().unwrap_or(id.as_str());
    if hash != hash_password(uname, password) {
        return Err(err());
    }
    cfg.active_account_id = Some(account.id.to_string());
    save(&cfg)?;
    Ok(account.clone())
}

pub fn login_account(id: &Uuid) -> AppResult<Account> {
    let mut cfg = load()?;
    let account = cfg
        .accounts
        .iter()
        .find(|a| &a.id == id)
        .cloned()
        .ok_or_else(|| AppError::Config("계정을 찾을 수 없습니다.".into()))?;
    cfg.active_account_id = Some(account.id.to_string());
    save(&cfg)?;
    Ok(account)
}

pub fn logout_account() -> AppResult<()> {
    let mut cfg = load()?;
    cfg.active_account_id = None;
    save(&cfg)
}

pub fn delete_account(id: &Uuid) -> AppResult<()> {
    let mut cfg = load()?;
    cfg.accounts.retain(|a| &a.id != id);
    if cfg.active_account_id.as_deref() == Some(&id.to_string()) {
        cfg.active_account_id = None;
    }
    save(&cfg)
}

pub fn active_account() -> AppResult<Option<Account>> {
    let cfg = load()?;
    let Some(id) = cfg.active_account_id.as_deref() else {
        return Ok(None);
    };
    Ok(cfg
        .accounts
        .iter()
        .find(|a| a.id.to_string() == id)
        .cloned())
}

// ── Push credentials ───────────────────────────────────────────────────────────

pub fn list_push_credentials() -> AppResult<std::collections::HashMap<String, PushCredential>> {
    Ok(load()?.push_credentials)
}

pub fn set_push_credential(repo_id: &Uuid, credential: &PushCredential) -> AppResult<()> {
    let mut cfg = load()?;
    cfg.push_credentials
        .insert(repo_id.to_string(), credential.clone());
    save(&cfg)
}

pub fn delete_push_credential(repo_id: &Uuid) -> AppResult<()> {
    let mut cfg = load()?;
    cfg.push_credentials.remove(&repo_id.to_string());
    save(&cfg)
}

pub fn get_push_credential(repo_id: &Uuid) -> AppResult<Option<PushCredential>> {
    Ok(load()?.push_credentials.get(&repo_id.to_string()).cloned())
}

// ── Path helpers ───────────────────────────────────────────────────────────────

pub fn config_dir() -> AppResult<PathBuf> {
    let base = dirs::config_dir()
        .ok_or_else(|| AppError::Config("could not resolve config dir".into()))?;
    Ok(base.join(APP_DIR))
}

pub fn config_path() -> AppResult<PathBuf> {
    Ok(config_dir()?.join(CONFIG_FILE))
}

pub fn inbox_db_path() -> AppResult<PathBuf> {
    Ok(config_dir()?.join("inbox.db"))
}

pub fn hooks_dir() -> AppResult<PathBuf> {
    Ok(config_dir()?.join("hooks"))
}

pub fn ensure_dirs() -> AppResult<()> {
    let dir = config_dir()?;
    if !dir.exists() {
        fs::create_dir_all(&dir)?;
    }
    let hd = hooks_dir()?;
    if !hd.exists() {
        fs::create_dir_all(&hd)?;
    }
    Ok(())
}

// ── Load / save ────────────────────────────────────────────────────────────────

pub fn load() -> AppResult<AppSettings> {
    let path = config_path()?;
    if !path.exists() {
        let mut cfg = AppSettings::default();
        // Demo login accounts (`test`/`test`, `test2`/`test2`) — the app requires login.
        ensure_seed_accounts(&mut cfg);
        save(&cfg)?;
        return Ok(cfg);
    }
    let bytes = fs::read(&path)?;
    let mut cfg: AppSettings =
        serde_json::from_slice(&bytes).map_err(|e| AppError::Config(e.to_string()))?;
    if cfg.schema_version < CURRENT_SCHEMA {
        migrate(&mut cfg)?;
    }
    // Seed default external tools for configs that already had schema_version == CURRENT_SCHEMA
    // (e.g. upgraded installs that ran migrate() before this seed was added, or fresh installs
    // that loaded before the default() seeding was in place)
    if cfg.external_tools.is_empty() {
        cfg.external_tools = AppSettings::default().external_tools;
    }
    // Demo login accounts (`test`/`test`, `test2`/`test2`) — the app requires login.
    ensure_seed_accounts(&mut cfg);
    Ok(cfg)
}

/// Atomic write: tmp file + rename — mirrors aos-git's `_atomic_dump` pattern.
pub fn save(cfg: &AppSettings) -> AppResult<()> {
    ensure_dirs()?;
    let path = config_path()?;
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(cfg)?;
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}
// ── Migration ──────────────────────────────────────────────────────────────────

/// Migrate settings from older schemas to CURRENT_SCHEMA.
///
/// Unknown JSON keys (channels, sync, base_branch, maintainers, …) are silently
/// dropped by serde during deserialization. New fields use their `#[serde(default)]`
/// values. So migration is just a version bump.
pub fn migrate(cfg: &mut AppSettings) -> AppResult<()> {
    cfg.schema_version = CURRENT_SCHEMA;
    Ok(())
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_roundtrip() {
        let s = AppSettings::default();
        let json = serde_json::to_string(&s).unwrap();
        let back: AppSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.schema_version, CURRENT_SCHEMA);
        assert!(back.repositories.is_empty());
        assert!(back.projects.is_empty());
        assert_eq!(back.external_tools.len(), 6);
        assert!(back.ssh_profile.default_key_path.is_empty());
        assert_eq!(back.ssh_profile.default_port, 22);
    }

    #[test]
    fn atomic_save_then_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CONFIG_FILE);
        let mut s = AppSettings::default();
        s.repositories.push(Repository {
            id: Uuid::new_v4(),
            path: "/tmp/x".into(),
            display_name: "x".into(),
            default_branch: "main".into(),
            working_branch: String::new(),
            ssh_host: String::new(),
            ssh_user: String::new(),
            ssh_key_path: String::new(),
            ssh_password: String::new(),
            ed25519_fingerprint: String::new(),
            ssh_port: 22,
            remote_url: String::new(),
            created_at: Utc::now(),
        });
        let bytes = serde_json::to_vec_pretty(&s).unwrap();
        std::fs::write(&path, bytes).unwrap();
        let raw = std::fs::read(&path).unwrap();
        let back: AppSettings = serde_json::from_slice(&raw).unwrap();
        assert_eq!(back.repositories.len(), 1);
        assert_eq!(back.repositories[0].display_name, "x");
    }
}
