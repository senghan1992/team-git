//! `gc-peer-listener` — tiny axum server that receives push events from the backend.

use axum::{extract::State, routing::post, Json, Router};
use rusqlite::Connection;
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::PathBuf;
use tokio::net::TcpListener;

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
