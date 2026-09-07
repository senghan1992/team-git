//! Login accounts, backed by the team server.
//!
//! Identities used to live in this app's own `config.json`, which meant every
//! machine had a different user list: a teammate's account simply did not exist
//! until someone re-typed it, and the "계정 전환" list in the UI was really a
//! list of whoever had ever signed in on that one computer.
//!
//! The server's `users` table (SQLite) is the source of truth now. This module
//! talks to `/auth/*` and caches **only the signed-in user plus their token**
//! locally, so the app stays signed in across restarts and while offline.
//!
//! Offline rules, deliberately conservative:
//!   - `current()` reads the cache — no network, so startup never blocks.
//!   - register / login / profile / password need the server. Failing loudly
//!     is better than signing someone in against a stale local copy.
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::config_store::{self, Account};
use crate::error::{AppError, AppResult};

/// Requests are short — a hung server should not freeze the login button.
const TIMEOUT: Duration = Duration::from_secs(15);

fn client() -> AppResult<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(TIMEOUT)
        .build()
        .map_err(|e| AppError::Internal(format!("HTTP 클라이언트 생성 실패: {e}")))
}

/// The configured server base URL, or a message telling the user what to do.
pub(crate) fn backend_url() -> AppResult<String> {
    let url = config_store::load()?.peer.backend_url.trim().to_string();
    if url.is_empty() {
        return Err(AppError::Config(
            "팀 서버 주소가 설정되지 않았습니다. 로그인 화면의 ‘서버 주소’에 입력하세요 (예: http://127.0.0.1:8000)."
                .into(),
        ));
    }
    Ok(url.trim_end_matches('/').to_string())
}

#[derive(Debug, Deserialize)]
struct UserPublic {
    id: String,
    username: String,
    email: String,
    name: String,
    created_at: String,
}

#[derive(Debug, Deserialize)]
struct AuthResponse {
    user: UserPublic,
    token: String,
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

/// Turn a non-2xx response into the server's own Korean `detail` message.
///
/// FastAPI puts the human-readable reason in `{"detail": "..."}`; surfacing the
/// bare status code instead ("register failed: 409") tells the user nothing
/// about *which* field collided.
pub(crate) async fn read_error(resp: reqwest::Response, fallback: &str) -> AppError {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    #[derive(Deserialize)]
    struct Detail {
        detail: serde_json::Value,
    }
    let message = serde_json::from_str::<Detail>(&body)
        .ok()
        .map(|d| match d.detail {
            serde_json::Value::String(s) => s,
            // 422 (스키마 검증)는 detail 이 배열이다 — 첫 항목의 msg 를 쓴다.
            serde_json::Value::Array(items) => items
                .first()
                .and_then(|i| i.get("msg"))
                .and_then(|m| m.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default(),
            other => other.to_string(),
        })
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| format!("{fallback} ({status})"));
    AppError::Config(message)
}

/// A transport failure is almost always "server not running" — say so.
fn transport_error(e: reqwest::Error, url: &str) -> AppError {
    AppError::Config(format!(
        "팀 서버에 연결할 수 없습니다 ({url}). 서버가 실행 중인지 확인하세요: cd backend && uvicorn app.main:app\n원인: {e}"
    ))
}

// ── Public API ──────────────────────────────────────────────────────────────

pub async fn register(
    name: &str,
    email: &str,
    username: &str,
    password: &str,
) -> AppResult<Account> {
    let base = backend_url()?;
    let url = format!("{base}/auth/register");
    #[derive(Serialize)]
    struct Body<'a> {
        name: &'a str,
        email: &'a str,
        username: &'a str,
        password: &'a str,
    }
    let resp = client()?
        .post(&url)
        .json(&Body {
            name,
            email,
            username,
            password,
        })
        .send()
        .await
        .map_err(|e| transport_error(e, &base))?;
    if !resp.status().is_success() {
        return Err(read_error(resp, "회원가입에 실패했습니다").await);
    }
    let auth: AuthResponse = resp
        .json()
        .await
        .map_err(|e| AppError::Internal(format!("응답 파싱 실패: {e}")))?;
    let account = auth.user.into_account();
    config_store::save_session(&account, &auth.token)?;
    Ok(account)
}

pub async fn login(username: &str, password: &str) -> AppResult<Account> {
    let base = backend_url()?;
    let url = format!("{base}/auth/login");
    #[derive(Serialize)]
    struct Body<'a> {
        username: &'a str,
        password: &'a str,
    }
    let resp = client()?
        .post(&url)
        .json(&Body { username, password })
        .send()
        .await
        .map_err(|e| transport_error(e, &base))?;
    if !resp.status().is_success() {
        return Err(read_error(resp, "로그인에 실패했습니다").await);
    }
    let auth: AuthResponse = resp
        .json()
        .await
        .map_err(|e| AppError::Internal(format!("응답 파싱 실패: {e}")))?;
    let account = auth.user.into_account();
    config_store::save_session(&account, &auth.token)?;
    Ok(account)
}

/// Sign out. The local session is cleared even if the server call fails —
/// otherwise a user could be stuck signed in whenever the network is down.
pub async fn logout() -> AppResult<()> {
    let token = config_store::session_token()?;
    if let (Ok(base), Some(token)) = (backend_url(), token) {
        if let Ok(c) = client() {
            let _ = c
                .post(format!("{base}/auth/logout"))
                .header("Authorization", format!("Bearer {token}"))
                .send()
                .await;
        }
    }
    config_store::clear_session()
}

pub async fn update_profile(name: Option<&str>, email: Option<&str>) -> AppResult<Account> {
    let base = backend_url()?;
    let token = require_token()?;
    #[derive(Serialize)]
    struct Body<'a> {
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        email: Option<&'a str>,
    }
    let resp = client()?
        .patch(format!("{base}/auth/me"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&Body { name, email })
        .send()
        .await
        .map_err(|e| transport_error(e, &base))?;
    if !resp.status().is_success() {
        return Err(read_error(resp, "내 정보 저장에 실패했습니다").await);
    }
    let user: UserPublic = resp
        .json()
        .await
        .map_err(|e| AppError::Internal(format!("응답 파싱 실패: {e}")))?;
    let account = user.into_account();
    config_store::save_session(&account, &token)?;
    Ok(account)
}

pub async fn change_password(current_password: &str, new_password: &str) -> AppResult<()> {
    let base = backend_url()?;
    let token = require_token()?;
    #[derive(Serialize)]
    struct Body<'a> {
        current_password: &'a str,
        new_password: &'a str,
    }
    let resp = client()?
        .post(format!("{base}/auth/me/password"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&Body {
            current_password,
            new_password,
        })
        .send()
        .await
        .map_err(|e| transport_error(e, &base))?;
    if !resp.status().is_success() {
        return Err(read_error(resp, "비밀번호 변경에 실패했습니다").await);
    }
    Ok(())
}

/// Delete the account on the server, then sign out locally.
pub async fn delete_self() -> AppResult<()> {
    let base = backend_url()?;
    let token = require_token()?;
    let resp = client()?
        .delete(format!("{base}/auth/me"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| transport_error(e, &base))?;
    if !resp.status().is_success() {
        return Err(read_error(resp, "회원 탈퇴에 실패했습니다").await);
    }
    config_store::clear_session()
}

/// Re-read the signed-in user from the server and refresh the cache.
///
/// Used by the my-page so a change made on another machine shows up. A network
/// failure is not an error here — the cached copy is returned instead, because
/// being offline should not look like being signed out. A 401 *is* acted on:
/// the session is gone, so the local cache must go too.
pub async fn refresh() -> AppResult<Option<Account>> {
    let Some(cached) = config_store::active_account()? else {
        return Ok(None);
    };
    let Ok(base) = backend_url() else {
        return Ok(Some(cached));
    };
    let Some(token) = config_store::session_token()? else {
        return Ok(Some(cached));
    };
    let Ok(c) = client() else {
        return Ok(Some(cached));
    };
    let resp = match c
        .get(format!("{base}/auth/me"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => return Ok(Some(cached)),
    };
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        config_store::clear_session()?;
        return Ok(None);
    }
    if !resp.status().is_success() {
        return Ok(Some(cached));
    }
    match resp.json::<UserPublic>().await {
        Ok(user) => {
            let account = user.into_account();
            config_store::save_session(&account, &token)?;
            Ok(Some(account))
        }
        Err(_) => Ok(Some(cached)),
    }
}

/// Find teammates by name / id / email. Empty result when signed out or when
/// the server cannot be reached — member search must never break the settings
/// screen it lives in.
pub async fn search(query: &str) -> AppResult<Vec<Account>> {
    let q = query.trim();
    if q.len() < 2 {
        return Ok(vec![]);
    }
    let base = backend_url()?;
    let token = require_token()?;
    let resp = client()?
        .get(format!("{base}/auth/users"))
        .query(&[("q", q)])
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| transport_error(e, &base))?;
    if !resp.status().is_success() {
        return Err(read_error(resp, "구성원 검색에 실패했습니다").await);
    }
    let users: Vec<UserPublic> = resp
        .json()
        .await
        .map_err(|e| AppError::Internal(format!("응답 파싱 실패: {e}")))?;
    Ok(users.into_iter().map(UserPublic::into_account).collect())
}

fn require_token() -> AppResult<String> {
    config_store::session_token()?.ok_or_else(|| AppError::Config("로그인이 필요합니다.".into()))
}
