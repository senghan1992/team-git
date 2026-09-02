//! Peer networking: token management, device/project registration, event fan-out.
//!
//! The desktop app acts as a client to the peer backend. It registers itself
//! on first launch, creates or joins projects, and calls POST /events when
//! the pre-push hook fires.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

use crate::config_store::config_dir;
use crate::error::{AppError, AppResult};

pub const PEER_TOKEN_FILE: &str = "peer_token";
pub const PEER_DEVICE_ID_FILE: &str = "peer_device_id";
pub const REPO_PROJECTS_FILE: &str = "repo_projects.json";

/// Backend API response types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerDeviceInfo {
    pub id: String,
    pub name: String,
    pub user_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerProjectInfo {
    pub id: String,
    pub display_name: String,
    pub join_code: String,
    pub role: String,
}

/// Persisted peer configuration embedded in AppSettings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PeerConfig {
    pub backend_url: String,
    #[serde(default)]
    pub device_token: String,
    #[serde(default)]
    pub device_id: String,
    #[serde(default)]
    pub device_name: String,
    #[serde(default)]
    pub last_poll_port: Option<u16>,
}

// ── Token helpers ─────────────────────────────────────────────────────────────

fn peer_token_path() -> AppResult<PathBuf> {
    Ok(config_dir()?.join(PEER_TOKEN_FILE))
}

fn peer_device_id_path() -> AppResult<PathBuf> {
    Ok(config_dir()?.join(PEER_DEVICE_ID_FILE))
}

/// Load the persisted bearer token, or generate and store a new one.
pub fn load_or_create_token() -> AppResult<String> {
    let path = peer_token_path()?;
    if path.exists() {
        Ok(std::fs::read_to_string(&path).map(|s| s.trim().to_string())?)
    } else {
        let token = uuid::Uuid::new_v4().to_string() + &uuid::Uuid::new_v4().to_string();
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, &token)?;
        Ok(token)
    }
}

/// Load the persisted device ID, or generate and store a new one.
pub fn load_or_create_device_id() -> AppResult<String> {
    let path = peer_device_id_path()?;
    if path.exists() {
        Ok(std::fs::read_to_string(&path).map(|s| s.trim().to_string())?)
    } else {
        let id = Uuid::new_v4().to_string();
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, &id)?;
        Ok(id)
    }
}

// ── HTTP client helpers (async) ────────────────────────────────────────────────

fn auth_header(token: &str) -> String {
    format!("Bearer {}", token)
}

/// Register this device with the peer backend.
pub async fn register_device(
    backend_url: &str,
    token: &str,
    name: &str,
) -> AppResult<PeerDeviceInfo> {
    let url = format!("{}/devices/register", backend_url.trim_end_matches('/'));
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("Authorization", auth_header(token))
        .json(&serde_json::json!({ "name": name }))
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("request failed: {}", e)))?;
    if !resp.status().is_success() {
        return Err(AppError::Internal(format!(
            "device registration failed: {}",
            resp.status()
        )));
    }
    let info: PeerDeviceInfo = resp
        .json()
        .await
        .map_err(|e| AppError::Internal(format!("bad JSON: {}", e)))?;
    Ok(info)
}

/// Create a new peer project.
pub async fn create_project(
    backend_url: &str,
    token: &str,
    name: &str,
) -> AppResult<PeerProjectInfo> {
    let url = format!("{}/projects", backend_url.trim_end_matches('/'));
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("Authorization", auth_header(token))
        .json(&serde_json::json!({ "display_name": name }))
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("request failed: {}", e)))?;
    if !resp.status().is_success() {
        return Err(AppError::Internal(format!(
            "create project failed: {}",
            resp.status()
        )));
    }
    let info: PeerProjectInfo = resp
        .json()
        .await
        .map_err(|e| AppError::Internal(format!("bad JSON: {}", e)))?;
    Ok(info)
}

/// Join an existing peer project by join code.
pub async fn join_project(
    backend_url: &str,
    token: &str,
    join_code: &str,
) -> AppResult<PeerProjectInfo> {
    let url = format!("{}/projects/join", backend_url.trim_end_matches('/'));
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("Authorization", auth_header(token))
        .json(&serde_json::json!({ "join_code": join_code }))
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("request failed: {}", e)))?;
    if !resp.status().is_success() {
        return Err(AppError::Internal(format!(
            "join project failed: {}",
            resp.status()
        )));
    }
    let info: PeerProjectInfo = resp
        .json()
        .await
        .map_err(|e| AppError::Internal(format!("bad JSON: {}", e)))?;
    Ok(info)
}

/// List all projects this device is a member of.
pub async fn list_projects(backend_url: &str, token: &str) -> AppResult<Vec<PeerProjectInfo>> {
    let url = format!("{}/projects", backend_url.trim_end_matches('/'));
    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .header("Authorization", auth_header(token))
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("request failed: {}", e)))?;
    if !resp.status().is_success() {
        return Err(AppError::Internal(format!(
            "list projects failed: {}",
            resp.status()
        )));
    }
    #[derive(Deserialize)]
    struct Resp {
        projects: Vec<PeerProjectInfo>,
    }
    let body: Resp = resp
        .json()
        .await
        .map_err(|e| AppError::Internal(format!("bad JSON: {}", e)))?;
    Ok(body.projects)
}

/// Fan out a push event to all project subscribers.
pub async fn fanout_event(
    backend_url: &str,
    token: &str,
    project_id: &str,
    event_kind: &str,
    repo_name: &str,
    payload: &str,
) -> AppResult<String> {
    let url = format!("{}/events", backend_url.trim_end_matches('/'));
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("Authorization", auth_header(token))
        .json(&serde_json::json!({
            "project_id": project_id,
            "event_kind": event_kind,
            "repo_name": repo_name,
            "payload": payload,
        }))
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("request failed: {}", e)))?;
    if !resp.status().is_success() {
        return Err(AppError::Internal(format!(
            "fanout failed: {}",
            resp.status()
        )));
    }
    #[derive(Deserialize)]
    struct Resp {
        id: String,
    }
    let body: Resp = resp
        .json()
        .await
        .map_err(|e| AppError::Internal(format!("bad JSON: {}", e)))?;
    Ok(body.id)
}

// ── Email invite helpers ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InviteByEmailResponse {
    pub device_id: Option<String>,
    pub email: String,
    pub role: String,
    pub pending: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberEmailEntry {
    pub device_id: Option<String>,
    pub email: Option<String>,
    pub name: Option<String>,
    pub role: String,
    pub joined_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ListMembersEmailResp {
    members: Vec<MemberEmailEntry>,
}

/// Invite someone to a project by email.
pub async fn invite_by_email(
    backend_url: &str,
    token: &str,
    project_id: &str,
    email: &str,
    name: Option<&str>,
    role: &str,
) -> AppResult<InviteByEmailResponse> {
    let url = format!(
        "{}/projects/{}/members/email",
        backend_url.trim_end_matches('/'),
        project_id
    );
    let client = reqwest::Client::new();
    let mut body_map = serde_json::Map::new();
    body_map.insert("email".to_string(), serde_json::json!(email));
    body_map.insert("role".to_string(), serde_json::json!(role));
    if let Some(n) = name {
        body_map.insert("name".to_string(), serde_json::json!(n));
    }
    let resp = client
        .post(&url)
        .header("Authorization", auth_header(token))
        .json(&body_map)
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("request failed: {}", e)))?;
    if !resp.status().is_success() {
        return Err(AppError::Internal(format!(
            "invite by email failed: {}",
            resp.status()
        )));
    }
    let info: InviteByEmailResponse = resp
        .json()
        .await
        .map_err(|e| AppError::Internal(format!("bad JSON: {}", e)))?;
    Ok(info)
}

/// List all members and pending email invites for a project.
pub async fn list_members(
    backend_url: &str,
    token: &str,
    project_id: &str,
) -> AppResult<Vec<MemberEmailEntry>> {
    let url = format!(
        "{}/projects/{}/members/email",
        backend_url.trim_end_matches('/'),
        project_id
    );
    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .header("Authorization", auth_header(token))
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("request failed: {}", e)))?;
    if !resp.status().is_success() {
        return Err(AppError::Internal(format!(
            "list members failed: {}",
            resp.status()
        )));
    }
    let body: ListMembersEmailResp = resp
        .json()
        .await
        .map_err(|e| AppError::Internal(format!("bad JSON: {}", e)))?;
    Ok(body.members)
}

/// Remove a pending email invite from a project.
pub async fn remove_email_invite(
    backend_url: &str,
    token: &str,
    project_id: &str,
    email: &str,
) -> AppResult<()> {
    let url = format!(
        "{}/projects/{}/members/email/{}",
        backend_url.trim_end_matches('/'),
        project_id,
        email
    );
    let client = reqwest::Client::new();
    let resp = client
        .delete(&url)
        .header("Authorization", auth_header(token))
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("request failed: {}", e)))?;
    if !resp.status().is_success() {
        return Err(AppError::Internal(format!(
            "remove email invite failed: {}",
            resp.status()
        )));
    }
    Ok(())
}

// ── Repo ↔ Project linkage ────────────────────────────────────────────────────

/// Mapping from repo path → list of project IDs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RepoProjects(pub std::collections::HashMap<String, Vec<String>>);

impl RepoProjects {
    pub fn load() -> AppResult<Self> {
        let path = config_dir()?.join(REPO_PROJECTS_FILE);
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)?;
        serde_json::from_str(&text).map_err(|e| AppError::Config(e.to_string()))
    }

    pub fn save(&self) -> AppResult<()> {
        let path = config_dir()?.join(REPO_PROJECTS_FILE);
        std::fs::create_dir_all(path.parent().unwrap())?;
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, text)?;
        Ok(())
    }

    pub fn link(&mut self, repo_path: &str, project_id: &str) {
        let canon = std::fs::canonicalize(repo_path)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| repo_path.to_string());
        let entry = self.0.entry(canon).or_insert_with(Vec::new);
        if !entry.contains(&project_id.to_string()) {
            entry.push(project_id.to_string());
        }
    }

    pub fn unlink(&mut self, repo_path: &str, project_id: &str) {
        let canon = std::fs::canonicalize(repo_path)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| repo_path.to_string());
        if let Some(entry) = self.0.get_mut(&canon) {
            entry.retain(|id| id != project_id);
            if entry.is_empty() {
                self.0.remove(&canon);
            }
        }
    }

    pub fn projects_for(&self, repo_path: &str) -> Vec<String> {
        let canon = std::fs::canonicalize(repo_path)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| repo_path.to_string());
        self.0.get(&canon).cloned().unwrap_or_default()
    }
}
