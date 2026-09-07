//! Google OAuth 로그인 — 브라우저(앱 안의 작은 창) 흐름을 조율한다.
//!
//! 계정은 여전히 팀 서버의 `users` 테이블이 소유한다. Google은 "이 이메일이
//! 진짜다"를 증명하는 도구일 뿐이고, 이 모듈은 그 증명을 토큰으로 바꾸는
//! 손잡이다:
//!
//!   1. 로컬 콜백 서버를 127.0.0.1 의 빈 포트에 연다 (외부에 노출 안 됨).
//!   2. 팀 서버에 `GET /auth/google/url?redirect_uri=<콜백 주소>` 를 물어
//!      Google 동의 화면 주소와 state 를 받는다.
//!   3. 앱 안의 작은 웹뷰 창으로 그 주소를 연다 — 사용자가 Google에서 로그인.
//!   4. Google → 팀 서버 `/auth/google/callback` → 로컬 콜백 서버로
//!      `?token=…` (또는 `?error=…`) 가 돌아오면 창을 닫고 세션을 저장한다.
//!
//! 콜백 서버는 127.0.0.1에만 묶이므로 다른 컴퓨터는 이 주소를 모른다;
//! 한 번의 흐름에 한 번만 작동하는 1회용 채널이라 state 재사용도 없다.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};
use tokio::sync::oneshot;

use crate::config_store::{self, Account};

/// 앱의 로컬 콜백 서버에 등록하는 경로 (팀 서버는 이 주소로 302 를 보낸다).
const CALLBACK_PATH: &str = "/auth/google/complete";
/// 동의 화면에서 머뭇거려도 5분이면 흐름을 끊는다.
const FLOW_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Deserialize)]
struct GoogleUrlResponse {
    url: String,
}

#[derive(Debug, Deserialize)]
struct UserPublic {
    id: String,
    username: String,
    email: String,
    name: String,
    created_at: String,
}

impl UserPublic {
    fn into_account(self) -> Account {
        Account {
            id: self.id,
            name: self.name,
            email: self.email,
            username: self.username,
            created_at: self.created_at,
        }
    }
}

#[tauri::command]
pub async fn google_login_start(app: tauri::AppHandle) -> Result<Account, String> {
    let base = crate::accounts::backend_url().map_err(|e| e.to_string())?;

    // 1 ── 로컬 콜백 서버 ─────────────────────────────────────────────
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| format!("로컬 콜백 서버를 시작하지 못했습니다: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("로컬 콜백 서버 주소를 얻지 못했습니다: {e}"))?
        .port();
    let callback_url = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");

    // 결과(성공/실패)와 사용자 취소(창 닫기)를 나누어 받는다. 결과를 받아
    // 처리한 뒤에는 로컬 서버가 스스로 내려가도록 종료 신호 채널도 건다.
    let (tx_result, rx_result) = oneshot::channel::<Result<String, String>>();
    let (tx_cancel, rx_cancel) = oneshot::channel::<()>();
    let (tx_shutdown, rx_shutdown) = oneshot::channel::<()>();

    {
        let tx = tx_result;
        std::thread::spawn(move || {
            // 이 스레드만의 런타임 — tauri 명령의 런타임과 섞이지 않는다.
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(_) => return,
            };
            rt.block_on(async move {
                let state = Arc::new(Mutex::new(Some(tx)));
                let router = axum::Router::new()
                    .route(CALLBACK_PATH, axum::routing::get(callback_handler))
                    .with_state(state);
                let listener = match tokio::net::TcpListener::from_std(listener) {
                    Ok(l) => l,
                    Err(_) => return,
                };
                // 콜백을 받아 처리한 뒤(또는 취소·시간초과) 로컬 서버는 내려간다.
                let _ = axum::serve(listener, router)
                    .with_graceful_shutdown(async move {
                        let _ = rx_shutdown.await;
                    })
                    .await;
            });
        });
    }

    // 2 ── 팀 서버에서 Google 동의 화면 주소 받기 ─────────────────────
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| format!("HTTP 클라이언트 생성 실패: {e}"))?;
    let resp = client
        .get(format!("{base}/auth/google/url"))
        .query(&[("redirect_uri", callback_url.as_str())])
        .send()
        .await
        .map_err(|e| {
            format!(
                "팀 서버에 연결할 수 없습니다 ({base}). 서버가 실행 중인지 확인하세요.\n원인: {e}"
            )
        })?;
    if !resp.status().is_success() {
        // 서버가 400 "Google 로그인이 설정되지 않았습니다" 같은 사유를 준다.
        return Err(crate::accounts::read_error(resp, "Google 로그인을 시작하지 못했습니다.")
            .await
            .to_string());
    }
    let start: GoogleUrlResponse = resp
        .json()
        .await
        .map_err(|e| format!("서버 응답을 읽지 못했습니다: {e}"))?;

    // 3 ── 로그인 창 열기 ──────────────────────────────────────────────
    if let Some(old) = app.get_webview_window("google-login") {
        let _ = old.close();
    }
    let url = start
        .url
        .parse::<url::Url>()
        .map_err(|e| format!("서버가 준 로그인 주소가 올바르지 않습니다: {e}"))?;
    let window = WebviewWindowBuilder::new(&app, "google-login", WebviewUrl::External(url))
        .title("Google 로그인 — Git Companion")
        .inner_size(460.0, 700.0)
        .min_inner_size(460.0, 640.0)
        .resizable(false)
        .build()
        .map_err(|e| format!("로그인 창을 열지 못했습니다: {e}"))?;

    // 사용자가 창을 닫으면 = 취소로 간주한다 (결과가 먼저 오면 무시됨).
    let cancel_tx = Arc::new(Mutex::new(Some(tx_cancel)));
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::Destroyed = event {
            if let Some(sender) = cancel_tx.lock().unwrap().take() {
                let _ = sender.send(());
            }
        }
    });

    // 4 ── 결과 대기 ──────────────────────────────────────────────────
    // 셋 중 먼저 오는 하나로 끝낸다: 콜백 서버의 결과 / 창 닫기(취소) / 시간초과.
    // rx_result 는 oneshot<Result<String,String>> 이다: 바깥 Result 는 채널
    // 자체의 오류(송신자 소실), 안쪽 Result 는 로그인 성공/실패. 받은 뒤
    // 안쪽을 꺼내야 취소·시간초과와 같은 한 줄로 비교할 수 있다.
    let token = tokio::select! {
        outcome = rx_result => match outcome {
            Ok(result) => result,
            Err(_) => Err("로그인 결과를 받지 못했습니다. 다시 시도해 주세요.".to_string()),
        },
        _ = rx_cancel => Err("로그인 창이 닫혀 취소되었습니다.".into()),
        _ = tokio::time::sleep(FLOW_TIMEOUT) => {
            Err("로그인 시간이 초과되었습니다. 다시 시도해 주세요.".into())
        }
    };
    // 어느 쪽이든 끝났으니 로컬 콜백 서버를 내린다 — 다음 로그인 때 새 포트로
    // 다시 연다.
    let _ = tx_shutdown.send(());
    let token = match token {
        Ok(t) => t,
        Err(e) => return Err(e),
    };
    let _ = window.close();

    // 5 ── 내 정보로 세션 저장 ────────────────────────────────────────
    let me = client
        .get(format!("{base}/auth/me"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| format!("팀 서버에 연결할 수 없습니다 ({base}).\n원인: {e}"))?;
    if !me.status().is_success() {
        return Err(
            crate::accounts::read_error(me, "Google 로그인은 됐지만 계정 정보를 가져오지 못했습니다.")
                .await
                .to_string(),
        );
    }
    let user: UserPublic = me
        .json()
        .await
        .map_err(|e| format!("서버 응답을 읽지 못했습니다: {e}"))?;
    let account = user.into_account();
    config_store::save_session(&account, &token)
        .map_err(|e| format!("세션을 저장하지 못했습니다: {e}"))?;
    Ok(account)
}

/// 로컬 콜백 서버의 단일 핸들러. 팀 서버가 `/auth/google/complete` 로
/// `?token=…` 또는 `?error=…` 를 보내면 그 내용을 명령 쪽 채널로 넘기고,
/// 창에 보일 작은 안내 페이지를 돌려준다.
async fn callback_handler(
    axum::extract::State(tx): axum::extract::State<
        Arc<Mutex<Option<oneshot::Sender<Result<String, String>>>>>,
    >,
    axum::extract::Query(query): axum::extract::Query<HashMap<String, String>>,
) -> axum::response::Html<&'static str> {
    let outcome = if let Some(error) = query.get("error") {
        Err(error.clone())
    } else if let Some(token) = query.get("token") {
        Ok(token.clone())
    } else {
        Err("로그인 응답이 올바르지 않습니다.".into())
    };
    // 첫 요청만 채널로 흘려보낸다 — 두 번째 방문은 페이지 구경만 한다.
    if let Some(tx) = tx.lock().unwrap().take() {
        let _ = tx.send(outcome);
    }
    axum::response::Html(
        r#"<!doctype html>
<meta charset="utf-8">
<title>로그인 완료</title>
<body style="font-family:sans-serif;display:flex;align-items:center;justify-content:center;height:100vh;margin:0;background:#faf7f0;color:#3d3a31">
  <div style="text-align:center">
    <p style="font-size:16px">Git Companion 로그인이 처리되었습니다.</p>
    <p style="color:#8f8a7d;font-size:13px">이 창은 곧 닫힙니다. 앱으로 돌아가세요.</p>
  </div>
</body>"#,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_path_is_absolute() {
        assert!(CALLBACK_PATH.starts_with('/'));
    }

    /// select 에서 oneshot<Result<..>> 를 꺼낼 때 안쪽까지 풀어야 한다 —
    /// 이 모듈의 핵심 흐름(결과/취소/시간초과 비교)이 여기서 나온다.
    #[tokio::test]
    async fn question_mark_unwraps_nested_result() {
        async fn inner() -> Result<String, String> {
            let (tx, rx) = tokio::sync::oneshot::channel::<Result<String, String>>();
            tx.send(Ok("tok".into())).unwrap();
            let token = tokio::select! {
                outcome = rx => match outcome {
                    Ok(r) => r,
                    Err(_) => Err("no".to_string()),
                },
                _ = tokio::time::sleep(Duration::from_secs(5)) => {
                    Err("timeout".to_string())
                }
            };
            let token = token?;
            format!("Bearer {token}");
            Ok(token)
        }
        assert_eq!(inner().await.unwrap(), "tok");
    }
}