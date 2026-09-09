//! 서버 운영자용 /admin/* 호출 통로.
//!
//! 관리 화면의 조회(사용자·프로젝트·이벤트)와 제어(정지·강제 로그아웃·멤버
//! 제거·프로젝트 삭제)는 전부 "세션 토큰을 붙여 서버의 /admin 에 JSON 을
//! 보낸다"는 같은 뼈대다. 엔드포인트마다 명령을 만들면 브릿지·Rust·UI 세
//! 곳이 늘어나므로, 메서드·경로·본문만 받아 전달하는 단일 통로로 묶는다.
//! 권한 판정은 서버가 한다 — 이 통로는 그저 토큰을 옮길 뿐이다.

use crate::error::{AppError, AppResult};

#[tauri::command]
pub async fn admin_request(
    method: String,
    path: String,
    body: Option<serde_json::Value>,
) -> AppResult<serde_json::Value> {
    let cfg = crate::config_store::load()?;
    let backend = if cfg.peer.backend_url.is_empty() {
        "http://127.0.0.1:8000".to_string()
    } else {
        cfg.peer.backend_url.clone()
    };
    let token = crate::config_store::session_token()?
        .ok_or_else(|| AppError::Internal("로그인이 필요합니다. 다시 로그인하세요.".into()))?;

    let parsed = reqwest::Method::from_bytes(method.as_bytes())
        .map_err(|e| AppError::Internal(format!("잘못된 메서드: {}", e)))?;
    // 경로 조작 방지 — /admin 아래만 허용한다.
    let path = path.trim_start_matches('/').to_string();
    if !path.starts_with("admin/") || path.contains("..") {
        return Err(AppError::Internal("허용되지 않은 경로입니다.".into()));
    }

    let client = reqwest::Client::new();
    let mut req = client
        .request(parsed, format!("{}/{}", backend.trim_end_matches('/'), path))
        .header("Authorization", format!("Bearer {}", token));
    if let Some(b) = &body {
        req = req.json(b);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("팀 서버에 연결할 수 없습니다: {}", e)))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        // 서버의 detail 메시지를 그대로 노출한다 — "관리자 계정이 아닙니다"
        // 같은 판정 문구가 UI 에 읽혀야 원인을 알 수 있다.
        let detail = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("detail").and_then(|d| d.as_str()).map(String::from))
            .unwrap_or_else(|| format!("요청 실패 ({})", status.as_u16()));
        return Err(AppError::Internal(detail));
    }
    if text.is_empty() {
        return Ok(serde_json::Value::Null);
    }
    serde_json::from_str(&text)
        .map_err(|e| AppError::Internal(format!("응답 해석 실패: {}", e)))
}
