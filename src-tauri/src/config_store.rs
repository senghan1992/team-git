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
pub const CURRENT_SCHEMA: u32 = 9;
// v9: Accounts moved to the team server's `users` table (SQLite). This file now
// keeps only `session` — the signed-in user plus their token — so the app stays
// signed in offline. The old local `accounts` array, `active_account_id` and the
// seeded demo logins are gone: serde drops those unknown keys, so an upgraded
// install is simply signed out once and signs in against the server.
// v8: Locked the app behind login and added username+password login.
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
///
/// `system_prompt` is the merge-manager's **pre-configured instruction** to the
/// resolver: it is written once in Settings and reused for every conflicted
/// file, so a merge that crashes can be repaired without anyone typing a
/// prompt in the moment. Empty means "use `ai::DEFAULT_SYSTEM_PROMPT`".
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
    /// Pre-configured system prompt. Empty → built-in default.
    #[serde(default)]
    pub system_prompt: String,
    /// When true, a merge/sync that ends in conflicts immediately runs the
    /// auto-resolver instead of waiting for the user to press the button.
    #[serde(default)]
    pub auto_resolve: bool,
    /// Side used for binary / oversized files during auto-resolve:
    /// "theirs" (default) or "ours".
    #[serde(default = "default_binary_strategy")]
    pub binary_strategy: String,
}

fn default_binary_strategy() -> String {
    "theirs".into()
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: String::new(),
            api_key: String::new(),
            model: String::new(),
            system_prompt: String::new(),
            auto_resolve: false,
            binary_strategy: default_binary_strategy(),
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

/// The signed-in person, as the team server describes them.
///
/// This is a **cache**, not a registry: the server's `users` table owns
/// identities (see `accounts.rs`). Only the currently signed-in user is stored,
/// so the app can stay signed in across restarts and while offline.
///
/// `id`/`created_at` are kept as the server's own strings — the app never
/// parses or generates them, so re-encoding would only invite mismatches.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Account {
    pub id: String,
    pub name: String,
    /// Lowercased. Members and merge managers are matched by email
    /// (see `.gpconfig`), so this is the identity that matters to a team.
    pub email: String,
    /// Login id, lowercased.
    pub username: String,
    /// ISO-8601 timestamp from the server.
    pub created_at: String,
}

/// A signed-in session: who, plus the bearer token for `/auth/*` calls.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub user: Account,
    pub token: String,
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
    /// Cached sign-in. The server owns the user list; this is only "who is
    /// signed in on this machine" so the app works offline and across restarts.
    /// Older configs carried an `accounts` array and `active_account_id` —
    /// serde drops those unknown keys, which signs the user out once.
    #[serde(default)]
    pub session: Option<SessionState>,
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
            session: None,
            push_credentials: std::collections::HashMap::new(),
        }
    }
}

// ── Signed-in session ──────────────────────────────────────────────────────────
//
// Registering, signing in, and editing a profile all live in `accounts.rs`,
// which talks to the team server. This file only remembers the result.

/// Store (or refresh) the signed-in user and their token.
pub fn save_session(user: &Account, token: &str) -> AppResult<()> {
    let mut cfg = load()?;
    cfg.session = Some(SessionState {
        user: user.clone(),
        token: token.to_string(),
    });
    save(&cfg)
}

pub fn clear_session() -> AppResult<()> {
    let mut cfg = load()?;
    cfg.session = None;
    save(&cfg)
}

/// The bearer token for `/auth/*` calls, or `None` when signed out.
pub fn session_token() -> AppResult<Option<String>> {
    Ok(load()?.session.map(|s| s.token))
}

/// The signed-in user from the local cache. No network, so callers can use it
/// during startup and while offline.
pub fn active_account() -> AppResult<Option<Account>> {
    Ok(load()?.session.map(|s| s.user))
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
        let cfg = AppSettings::default();
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

    /// 시나리오 5: 병합 관리자가 미리 저장해 둔 지침/자동 해결 스위치는
    /// 재시작 후에도 그대로 남아야 하고, 이 필드들이 없던 옛 config.json 도
    /// 안전한 기본값으로 열려야 한다.
    #[test]
    fn ai_config_persists_prompt_and_defaults_for_old_files() {
        let mut s = AppSettings::default();
        assert_eq!(s.ai.binary_strategy, "theirs");
        assert!(!s.ai.auto_resolve);
        assert!(s.ai.system_prompt.is_empty());

        s.ai.enabled = true;
        s.ai.auto_resolve = true;
        s.ai.system_prompt = "마이그레이션 파일은 합치지 말고 양쪽을 모두 남긴다.".into();
        s.ai.binary_strategy = "ours".into();
        let json = serde_json::to_string(&s).unwrap();
        let back: AppSettings = serde_json::from_str(&json).unwrap();
        assert!(back.ai.auto_resolve);
        assert_eq!(
            back.ai.system_prompt,
            "마이그레이션 파일은 합치지 말고 양쪽을 모두 남긴다."
        );
        assert_eq!(back.ai.binary_strategy, "ours");

        // 새 필드가 전혀 없는 예전 파일.
        let legacy = r#"{"schema_version":7,"ai":{"enabled":true,"base_url":"u","api_key":"k","model":"m"}}"#;
        let old: AppSettings = serde_json::from_str(legacy).unwrap();
        assert!(old.ai.enabled);
        assert!(!old.ai.auto_resolve, "옛 파일은 자동 해결이 꺼진 채 열린다");
        assert!(
            old.ai.system_prompt.is_empty(),
            "지침은 비어 있어 기본값을 쓴다"
        );
        assert_eq!(
            old.ai.binary_strategy, "theirs",
            "바이너리 전략은 안전한 기본값으로 채워진다"
        );
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
