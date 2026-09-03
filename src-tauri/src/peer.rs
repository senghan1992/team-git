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

// ── 오프라인 스풀 ────────────────────────────────────────────────────────────
//
// 팀 서버가 죽어 있는 동안 push 가 일어나면 알림 이벤트가 영영 사라졌다.
// hook emit 이 전송에 실패한 이벤트를 한 줄(JSON)씩 여기 붙여 두고,
// 앱의 주기 폴링(peer_poll_now)이 서버가 살아났을 때 재전송한다.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpooledEvent {
    pub project_id: String,
    pub event_kind: String,
    pub repo_name: String,
    pub payload: String,
}

fn spool_path() -> AppResult<std::path::PathBuf> {
    Ok(crate::config_store::config_dir()?.join("pending_events.jsonl"))
}

/// 전송 실패한 이벤트를 스풀에 덧붙인다 (append-only — hook 프로세스와
/// 앱이 동시에 붙여도 안전하다).
pub fn spool_event(ev: &SpooledEvent) -> AppResult<()> {
    use std::io::Write;
    let path = spool_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut line = serde_json::to_string(ev)
        .map_err(|e| AppError::Internal(format!("스풀 직렬화 실패: {e}")))?;
    line.push('\n');
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
    f.write_all(line.as_bytes())?;
    Ok(())
}

/// 스풀에 쌓인 이벤트를 재전송한다. 성공한 것만 지우고, 실패분과
/// (읽는 사이 hook 이 새로 붙인) 꼬리 부분은 보존한다. 보낸 건수를 돌려준다.
pub async fn flush_spooled_events(backend_url: &str, token: &str) -> AppResult<usize> {
    let path = spool_path()?;
    let data = match std::fs::read(&path) {
        Ok(d) if !d.is_empty() => d,
        _ => return Ok(0),
    };
    let read_len = data.len() as u64;
    let text = String::from_utf8_lossy(&data);
    let mut kept: Vec<String> = Vec::new();
    let mut sent = 0usize;
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(ev) = serde_json::from_str::<SpooledEvent>(line) else {
            continue; // 깨진 줄은 버린다 — 재전송할 방법이 없다.
        };
        match fanout_event(
            backend_url,
            token,
            &ev.project_id,
            &ev.event_kind,
            &ev.repo_name,
            &ev.payload,
        )
        .await
        {
            Ok(_) => sent += 1,
            Err(e) => {
                // 서버가 4xx 로 거부한 줄(사라진 프로젝트, 권한 없음 등)은
                // 영원히 성공할 수 없다 — 보존하면 매 폴링마다 재시도되는
                // 독약이 된다. 네트워크 오류와 5xx(일시 장애)만 보존한다.
                let msg = e.to_string();
                let permanent = msg.contains("fanout failed: 4");
                if !permanent {
                    kept.push(line.to_string());
                }
            }
        }
    }
    // 처리하는 사이 hook 이 새 줄을 붙였을 수 있다 — 읽은 길이 이후의
    // 꼬리를 그대로 보존한다.
    let mut out = kept.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    if let Ok(now) = std::fs::read(&path) {
        if now.len() as u64 > read_len {
            out.push_str(&String::from_utf8_lossy(&now[read_len as usize..]));
        }
    }
    if out.is_empty() {
        let _ = std::fs::remove_file(&path);
    } else {
        std::fs::write(&path, out)?;
    }
    Ok(sent)
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
    // pre-push hook 이 이 함수를 동기 대기한다 — 서버가 응답 없이 매달리면
    // `git push` 자체가 hook 안에서 영원히 멈추므로, 알림은 push 를 5초
    // 이상 잡아 두지 않는다 (연결 거부는 즉시 실패해 push 는 정상 진행).
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| AppError::Internal(format!("HTTP 클라이언트 생성 실패: {e}")))?;
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
