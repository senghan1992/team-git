//! Tauri commands for peer networking.
use crate::config_store::config_dir;
use crate::error::{AppError, AppResult};
use crate::peer::{self, PeerConfig, PeerDeviceInfo, PeerProjectInfo};
use uuid::Uuid;

#[tauri::command]
pub async fn peer_register_device(backend_url: String, name: String) -> AppResult<PeerDeviceInfo> {
    let token = peer::load_or_create_token()?;
    let info = peer::register_device(&backend_url, &token, &name).await?;

    // Persist peer config into AppSettings.peer (single source of truth)
    let mut cfg = crate::config_store::load()?;
    cfg.peer.backend_url = backend_url;
    cfg.peer.device_token = token.clone();
    cfg.peer.device_id = info.id.clone();
    cfg.peer.device_name = name;
    crate::config_store::save(&cfg)?;

    Ok(info)
}

/// 서버가 이 기기를 알기 전에는 팀 만들기·합류·폴링이 전부 401로 죽는다.
/// 예전에는 `peer_register_device`를 아무도 호출하지 않아 새 설치에서
/// "팀 만들기"가 곧바로 실패했다 — 서버를 쓰는 커맨드가 시작하기 전에
/// 여기서 조용히 한 번 등록한다 (서버 쪽은 같은 토큰 재등록을 멱등 처리).
async fn ensure_device_registered() -> AppResult<(String, String)> {
    let cfg = crate::config_store::load()?;
    let token = peer::load_or_create_token()?;
    let backend = if cfg.peer.backend_url.is_empty() {
        "http://127.0.0.1:8000".to_string()
    } else {
        cfg.peer.backend_url.clone()
    };
    if cfg.peer.device_id.is_empty() {
        let name = cfg
            .session
            .as_ref()
            .map(|s| s.user.name.clone())
            .filter(|n| !n.trim().is_empty())
            .or_else(|| std::env::var("HOSTNAME").ok().filter(|h| !h.is_empty()))
            .unwrap_or_else(|| "Git Companion".to_string());
        let info = peer::register_device(&backend, &token, &name).await?;
        let mut cfg = crate::config_store::load()?;
        cfg.peer.backend_url = backend.clone();
        cfg.peer.device_token = token.clone();
        cfg.peer.device_id = info.id;
        cfg.peer.device_name = name;
        crate::config_store::save(&cfg)?;
    }
    Ok((backend, token))
}

#[tauri::command]
pub async fn peer_create_project(
    name: String,
    repo_id: Option<Uuid>,
) -> AppResult<PeerProjectInfo> {
    let (backend, token) = ensure_device_registered().await?;
    let cfg = crate::config_store::load()?;
    let info = peer::create_project(&backend, &token, &name).await?;

    if let Some(r_id) = repo_id {
        if let Some(repo) = cfg.repositories.iter().find(|r| r.id == r_id) {
            let repo_path = std::fs::canonicalize(&repo.path)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| repo.path.clone());
            let mut repos = peer::RepoProjects::load()?;
            repos.link(&repo_path, &info.id);
            repos.save()?;
        }
    }

    Ok(info)
}

#[tauri::command]
pub async fn peer_join_project(code: String, repo_id: Option<Uuid>) -> AppResult<PeerProjectInfo> {
    let (backend, token) = ensure_device_registered().await?;
    let cfg = crate::config_store::load()?;
    let info = peer::join_project(&backend, &token, &code).await?;

    if let Some(r_id) = repo_id {
        if let Some(repo) = cfg.repositories.iter().find(|r| r.id == r_id) {
            let repo_path = std::fs::canonicalize(&repo.path)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| repo.path.clone());
            let mut repos = peer::RepoProjects::load()?;
            repos.link(&repo_path, &info.id);
            repos.save()?;
        }
    }

    Ok(info)
}

#[tauri::command]
pub async fn peer_list_projects() -> AppResult<Vec<PeerProjectInfo>> {
    let (backend, token) = ensure_device_registered().await?;
    peer::list_projects(&backend, &token).await
}

#[tauri::command]
pub fn peer_link_repo_to_project(repo_id: Uuid, project_id: String) -> AppResult<()> {
    let cfg = crate::config_store::load()?;
    let repo = cfg
        .repositories
        .iter()
        .find(|r| r.id == repo_id)
        .ok_or_else(|| AppError::RepoNotFound(repo_id.to_string()))?;
    let repo_path = std::fs::canonicalize(&repo.path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| repo.path.clone());
    let mut repos = peer::RepoProjects::load()?;
    repos.link(&repo_path, &project_id);
    repos.save()
}

#[tauri::command]
pub fn peer_unlink_repo(repo_id: Uuid, project_id: String) -> AppResult<()> {
    let cfg = crate::config_store::load()?;
    let repo = cfg
        .repositories
        .iter()
        .find(|r| r.id == repo_id)
        .ok_or_else(|| AppError::RepoNotFound(repo_id.to_string()))?;
    let repo_path = std::fs::canonicalize(&repo.path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| repo.path.clone());
    let mut repos = peer::RepoProjects::load()?;
    repos.unlink(&repo_path, &project_id);
    repos.save()
}

/// Linked-repo summary for the project card — id + display name + path.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoLinkSummary {
    pub repo_id: Uuid,
    pub display_name: String,
    pub path: String,
}

/// List repositories registered on this device that are linked to `project_id`.
/// (The reverse of `peer_link_repo_to_project` — used so a project card can
/// show which local repos belong to the team project.)
#[tauri::command]
pub fn peer_repos_for_project(project_id: String) -> AppResult<Vec<RepoLinkSummary>> {
    let cfg = crate::config_store::load()?;
    let repos_by_path = peer::RepoProjects::load()?;
    let mut out = Vec::new();
    for repo in &cfg.repositories {
        let canon = std::fs::canonicalize(&repo.path)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| repo.path.clone());
        if repos_by_path
            .projects_for(&canon)
            .iter()
            .any(|id| id == &project_id)
        {
            out.push(RepoLinkSummary {
                repo_id: repo.id,
                display_name: repo.display_name.clone(),
                path: repo.path.clone(),
            });
        }
    }
    Ok(out)
}

/// Read the peer_port file and register the URL with the backend via PUT /devices/me/poll_url.
#[tauri::command]
pub async fn peer_local_url() -> AppResult<String> {
    let cfg = crate::config_store::load()?;
    let path = config_dir()?.join("peer_port");
    if !path.exists() {
        return Err(AppError::Config("peer listener not started".into()));
    }
    let port: u16 = std::fs::read_to_string(&path)?
        .trim()
        .parse()
        .map_err(|e| AppError::Config(format!("invalid port: {}", e)))?;
    let local_url = format!("http://127.0.0.1:{}", port);

    let client = reqwest::Client::new();
    let resp = client
        .put(format!(
            "{}/devices/me/poll_url",
            cfg.peer.backend_url.trim_end_matches('/')
        ))
        .header("Authorization", format!("Bearer {}", cfg.peer.device_token))
        .json(&serde_json::json!({ "poll_url": local_url }))
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("poll_url registration failed: {}", e)))?;

    if !resp.status().is_success() {
        return Err(AppError::Internal(format!(
            "poll_url PUT failed: {}",
            resp.status()
        )));
    }

    Ok(local_url)
}

/// Count unread peer push events in the local inbox DB.
#[tauri::command]
pub fn peer_unread_count() -> AppResult<u32> {
    let store = crate::notify::store::Store::open()?;
    store.count_unread_team_events()
}
/// Get current peer config.
#[tauri::command]
pub fn peer_get_config() -> AppResult<PeerConfig> {
    let cfg = crate::config_store::load()?;
    Ok(cfg.peer.clone())
}

/// Update backend URL and persist to AppSettings.peer.
#[tauri::command]
pub async fn peer_set_backend_url(url: String) -> AppResult<()> {
    let mut cfg = crate::config_store::load()?;
    cfg.peer.backend_url = url;
    crate::config_store::save(&cfg)?;
    Ok(())
}

/// Poll once, drain all pending events, persist each to the local team inbox DB.
/// The backend marks each delivery as consumed, so we must deserialize and store
/// before the next poll call — otherwise events are permanently lost.
/// Uses ?wait=0 on every call so empty polls return immediately instead of blocking 25s.
#[tauri::command]
pub async fn peer_poll_now() -> AppResult<()> {
    use crate::notify::store::{new_id, Store, TeamEventRow};

    let store = Store::open()?;
    let cfg = crate::config_store::load()?;
    // 팀 서버를 설정한 적이 없으면 조용히 넘어간다 (알림은 선택 기능).
    if cfg.peer.backend_url.is_empty() {
        return Ok(());
    }
    // 서버가 이 기기를 모르면 폴링은 영원히 401이다 — 필요하면 등록부터.
    let (backend, token) = ensure_device_registered().await?;
    // 서버가 죽어 있는 동안 hook 이 보관해 둔 알림부터 재전송한다.
    let _ = peer::flush_spooled_events(&backend, &token).await;
    let client = reqwest::Client::new();
    loop {
        let url = format!("{}/events/poll?wait=0", backend.trim_end_matches('/'));
        let resp = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .timeout(std::time::Duration::from_secs(35))
            .send()
            .await
            .map_err(|e| AppError::Internal(format!("poll failed: {}", e)))?;
        if !resp.status().is_success() {
            return Err(AppError::Internal(format!(
                "poll returned {}",
                resp.status()
            )));
        }
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| AppError::Internal(format!("poll JSON parse failed: {}", e)))?;
        let event_val = match body.get("event") {
            Some(serde_json::Value::Null) | None => break,
            Some(v) => v.clone(),
        };
        // 서버는 poll 응답 시점에 이 이벤트를 '배달됨'으로 소비한다 — 여기서
        // 파싱 실패로 버리면 재배달 없이 영영 사라진다. 필드가 빠져도 있는
        // 것만으로 저장한다 (엄격한 구조체 파싱 금지).
        let get = |k: &str| -> String {
            event_val
                .get(k)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        // 보낸 사람은 이름이 우선 — 없으면(구버전 서버) 기기 id 라도 남긴다.
        let sender = {
            let name = get("sender_device_name");
            if name.is_empty() { get("sender_device_id") } else { name }
        };
        let row = TeamEventRow {
            id: new_id(),
            project_id: get("project_id"),
            sender_device_name: sender,
            event_kind: get("event_kind"),
            repo_name: get("repo_name"),
            payload: get("payload"),
            received_at: chrono::Utc::now(),
            read: false,
        };
        store.insert_team_event(&row)?;
    }
    Ok(())
}

/// Leave a peer project (remove self from member list) and unlink all repos.
#[tauri::command]
pub async fn peer_leave_project(project_id: String) -> AppResult<()> {
    let cfg = crate::config_store::load()?;
    let token = peer::load_or_create_token()?;
    let device_id = &cfg.peer.device_id;

    let client = reqwest::Client::new();
    let resp = client
        .delete(format!(
            "{}/projects/{}/members/{}",
            cfg.peer.backend_url.trim_end_matches('/'),
            project_id,
            device_id
        ))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("leave failed: {}", e)))?;
    if !resp.status().is_success() {
        return Err(AppError::Internal(format!(
            "leave returned {}",
            resp.status()
        )));
    }

    // Unlink all repos from this project
    let mut repos = peer::RepoProjects::load()?;
    let to_remove: Vec<String> = repos
        .0
        .iter()
        .filter(|(_, pids)| pids.contains(&project_id))
        .map(|(rp, _)| rp.clone())
        .collect();
    for rp in to_remove {
        repos.unlink(&rp, &project_id);
    }
    repos.save()?;

    Ok(())
}

/// List team push events from local inbox DB.
#[tauri::command]
pub fn peer_list_team_events(
    limit: u32,
    unread_only: bool,
) -> AppResult<Vec<crate::notify::store::TeamEventRow>> {
    let store = crate::notify::store::Store::open()?;
    store.list_team_events(limit, unread_only)
}

/// Mark a team event as read.
#[tauri::command]
pub fn peer_mark_team_read(id: String) -> AppResult<()> {
    let store = crate::notify::store::Store::open()?;
    store.mark_team_read(&id)
}

/// Mark every team event as read — the inbox's "모두 읽음".
#[tauri::command]
pub fn peer_mark_all_team_read() -> AppResult<u32> {
    let store = crate::notify::store::Store::open()?;
    store.mark_all_team_read()
}

// ── Email invite commands ────────────────────────────────────────────────────────

/// Invite someone to a project by email.
#[tauri::command]
pub async fn peer_invite_by_email(
    project_id: String,
    email: String,
    name: Option<String>,
    role: Option<String>,
) -> AppResult<peer::InviteByEmailResponse> {
    let cfg = crate::config_store::load()?;
    let token = peer::load_or_create_token()?;
    let role_str = role.unwrap_or_else(|| "member".to_string());
    peer::invite_by_email(
        &cfg.peer.backend_url,
        &token,
        &project_id,
        &email,
        name.as_deref(),
        &role_str,
    )
    .await
}

/// List all members and pending email invites for a project.
#[tauri::command]
pub async fn peer_list_members(project_id: String) -> AppResult<Vec<peer::MemberEmailEntry>> {
    let cfg = crate::config_store::load()?;
    let token = peer::load_or_create_token()?;
    peer::list_members(&cfg.peer.backend_url, &token, &project_id).await
}

/// 팀 서버에 실제로 연결되는지 확인한다.
///
/// 로그인 화면의 "서버 주소"는 예전에 저장만 하고 "저장했습니다"라고 말했다.
/// 오타 하나 때문에 로그인이 계속 실패하는데 화면은 성공했다고 하니, 원인을
/// 찾을 방법이 없었다. 저장 직후 여기서 `/healthz` 를 확인해 준다.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BackendCheck {
    pub ok: bool,
    pub message: String,
}

#[tauri::command]
pub async fn peer_check_backend(url: Option<String>) -> AppResult<BackendCheck> {
    let candidate = match url.map(|u| u.trim().to_string()).filter(|u| !u.is_empty()) {
        Some(u) => u,
        None => crate::config_store::load()?
            .peer
            .backend_url
            .trim()
            .to_string(),
    };
    let base = candidate.trim_end_matches('/').to_string();
    if base.is_empty() {
        return Ok(BackendCheck {
            ok: false,
            message: "서버 주소가 비어 있습니다.".into(),
        });
    }
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return Ok(BackendCheck {
                ok: false,
                message: format!("확인할 수 없습니다: {e}"),
            })
        }
    };
    match client.get(format!("{base}/healthz")).send().await {
        Ok(r) if r.status().is_success() => Ok(BackendCheck {
            ok: true,
            message: "서버에 연결됩니다.".into(),
        }),
        Ok(r) => Ok(BackendCheck {
            ok: false,
            message: format!("서버가 응답했지만 상태가 정상이 아닙니다 ({}).", r.status()),
        }),
        Err(_) => Ok(BackendCheck {
            ok: false,
            message:
                "연결할 수 없습니다. 주소가 맞는지, 서버가 실행 중인지 확인하세요:\n  cd backend && uvicorn app.main:app"
                    .into(),
        }),
    }
}

/// Remove a pending email invite from a project.
#[tauri::command]
pub async fn peer_remove_email_invite(project_id: String, email: String) -> AppResult<()> {
    let cfg = crate::config_store::load()?;
    let token = peer::load_or_create_token()?;
    peer::remove_email_invite(&cfg.peer.backend_url, &token, &project_id, &email).await
}
