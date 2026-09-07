//! `gc-peer-listener` — tiny axum server that receives push events from the backend.
//!
//! 앱(부모)이 `GC_PARENT_PID` 를 주면, 부모가 죽는 순간 1초 안에 스스로
//! 종료한다. 앱이 끝났는데 리스너만 남으면 exe 파일이 잠겨서 새 버전 설치가
//! "Error opening file for writing gc-peer-listener.exe" 로 실패하는 원인이
//! 된다 — 이 감시는 그 잠금을 남기지 않게 한다.

use axum::{extract::State, routing::post, Json, Router};
use rusqlite::Connection;
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::PathBuf;
use tokio::net::TcpListener;

/// 부모 프로세스 생존 감시.
///
/// Windows: 프로세스 핸들을 열어 종료 코드가 STILL_ACTIVE 인지 본다.
/// Unix: `kill(pid, 0)` 시그널 0 으로 존재 여부만 확인한다.
#[cfg(windows)]
mod parent_watch {
    use std::time::Duration;
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    pub fn spawn(pid: u32) {
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(1));
            if !alive(pid) {
                std::process::exit(0);
            }
        });
    }

    fn alive(pid: u32) -> bool {
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return false; // 열 수 없다 = 이미 사라졌다.
            }
            let mut code: u32 = 0;
            let ok = GetExitCodeProcess(handle, &mut code);
            CloseHandle(handle);
            ok != 0 && code == STILL_ACTIVE as u32
        }
    }
}

#[cfg(unix)]
mod parent_watch {
    use std::time::Duration;

    pub fn spawn(pid: u32) {
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(1));
            // kill(2) 시그널 0 — 실제 신호를 보내지 않고 존재만 확인한다.
            if unsafe { libc::kill(pid as i32, 0) != 0 } {
                std::process::exit(0);
            }
        });
    }
}

#[derive(Debug, Deserialize)]
struct PushPayload {
    event_id: String,
    project_id: Option<String>,
    sender_device_name: Option<String>,
    event_kind: Option<String>,
    repo_name: Option<String>,
    payload: Option<String>,
}

#[derive(Clone)]
struct AppState {
    db_path: PathBuf,
}

async fn handle_push(
    State(state): State<AppState>,
    Json(payload): Json<PushPayload>,
) -> Json<serde_json::Value> {
    let conn = match Connection::open(&state.db_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("gc-peer-listener: failed to open DB: {}", e);
            return Json(serde_json::json!({"error": e.to_string()}));
        }
    };

    let event_id = &payload.event_id;
    let project_id = payload.project_id.as_deref().unwrap_or("");
    let sender = payload.sender_device_name.as_deref().unwrap_or("peer");
    let event_kind = payload.event_kind.as_deref().unwrap_or("main_push");
    let repo_name = payload.repo_name.as_deref().unwrap_or("");
    let event_payload = payload.payload.as_deref().unwrap_or("{}");
    // ISO 8601 so Rust's DateTime::parse_from_rfc3339 can read it
    let received_at = chrono::Utc::now().to_rfc3339();
    let team_event_id = format!("team_{}", &event_id[..event_id.len().min(24)]);

    // NOTE: event_kind is stored as-is (branch_push / main_push / release).
    // The UI matches on these exact values for kind-specific actions.
    if let Err(e) = conn.execute(
        "INSERT OR IGNORE INTO team_events (id, project_id, sender_device_name, event_kind, repo_name, payload, received_at, read)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0)",
        rusqlite::params![
            &team_event_id,
            project_id,
            sender,
            event_kind,
            repo_name,
            event_payload,
            &received_at,
        ],
    ) {
        eprintln!("gc-peer-listener: failed to insert event: {}", e);
    }

    Json(serde_json::json!({"ok": true}))
}

async fn handle_health() -> &'static str {
    "ok"
}

#[tokio::main]
async fn main() {
    // 앱이 준 부모 PID 가 있으면 "부모가 죽으면 같이 죽는다" 감시를 건다.
    // (설치기·수동 실행처럼 부모가 없을 땐 감시 없이 평소처럼 돈다.)
    if let Some(pid) = std::env::var("GC_PARENT_PID")
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
    {
        parent_watch::spawn(pid);
    }

    let port: u16 = std::env::var("GC_PEER_PORT")
        .unwrap_or_else(|_| "0".into())
        .parse()
        .unwrap_or(0);

    let db_path = std::env::var("GC_PEER_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("com.gitcompanion.app")
                .join("inbox.db")
        });

    if let Some(parent) = db_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let conn = Connection::open(&db_path).expect("cannot open inbox DB");
    conn.execute(
        "CREATE TABLE IF NOT EXISTS team_events (
            id TEXT PRIMARY KEY,
            project_id TEXT NOT NULL,
            sender_device_name TEXT NOT NULL DEFAULT '',
            event_kind TEXT NOT NULL,
            repo_name TEXT NOT NULL,
            payload TEXT NOT NULL,
            received_at TEXT NOT NULL,
            read INTEGER NOT NULL DEFAULT 0
        )",
        [],
    )
    .expect("cannot create team_events table");

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = TcpListener::bind(&addr).await.expect("cannot bind port");
    let actual_port = listener.local_addr().expect("cannot get local addr").port();

    println!("{}", actual_port);

    let state = AppState { db_path };

    let app = Router::new()
        .route("/events", post(handle_push))
        .route("/healthz", post(handle_health))
        .with_state(state);

    axum::serve(listener, app).await.expect("server error");
}
