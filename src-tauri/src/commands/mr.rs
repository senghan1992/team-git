//! 병합 요청 commands — 푸시와 승인 사이의 명시적 승인 단계.
//!
//! 팀원(작업 탭)이 `request_merge`로 요청을 올리면, 병합 관리자(병합 탭)의
//! 대기열에 나타난다. 요청은 git ref(`refs/gc-mr/<base>/<branch>`)로 원격에
//! 공유되므로 서버 설정 없이 모든 팀원이 같은 대기열을 본다.
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::config_store;
use crate::commands::git::{resolve_target, MERGE_REMOTE};
use crate::error::AppResult;
use crate::git::mr::{self, MergeRequest, RequestedMerge};

/// 병합 요청 전송 결과 — 푸시와 같은 자격증명 계약을 쓴다.
/// `auth_required` 는 HTTPS 원격(mod.lge.com 같은 Git 호스트)에 자격증명이
/// 없거나 거부된 경우 — UI 가 로그인 모달을 띄우고 같은 요청으로 재시도한다.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeRequestOutcome {
    /// 성공 시 만들어진(또는 갱신된) 요청. `auth_required` 면 None.
    pub request: Option<MergeRequest>,
    #[serde(default)]
    pub auth_required: bool,
}

#[tauri::command]
pub async fn request_merge(
    repo_id: Uuid,
    base: String,
    branch: String,
    title: Option<String>,
    credentials: Option<config_store::PushCredential>,
    save_credential: bool,
) -> AppResult<MergeRequestOutcome> {
    let (target, repo) = resolve_target(repo_id)?;
    // HTTPS 원격 + 자격증명 없음 → 요청 ref 푸시는 반드시 실패한다.
    // (푸시·브랜치 삭제와 같은 정책 — 시도조차 하지 않는다.)
    let https = crate::git::ops::remote_is_https(&target, MERGE_REMOTE);
    if https && credentials.is_none() {
        return Ok(MergeRequestOutcome {
            request: None,
            auth_required: true,
        });
    }
    // 요청자 신원 — 로그인해 있으면 계정, 아니면 git 전역 설정을 믿는다.
    let (author, email) = match config_store::active_account().ok().flatten() {
        Some(acc) => (acc.name, acc.email),
        None => {
            let name = crate::git::run_at_target(&target, ["config", "user.name"])
                .ok()
                .map(|o| o.stdout.trim().to_string())
                .unwrap_or_default();
            let mail = crate::git::run_at_target(&target, ["config", "user.email"])
                .ok()
                .map(|o| o.stdout.trim().to_string())
                .unwrap_or_default();
            (name, mail)
        }
    };
    let req = mr::request_merge(
        &target,
        MERGE_REMOTE,
        &base,
        &branch,
        title.as_deref(),
        &author,
        &email,
        credentials.as_ref(),
    )?;
    // 자격증명은 있었는데 거부됐을 수 있다(401) — 이 경우에도 로그인 모달로
    // 이어준다 (prefill 은 방금 거부된 값).
    let auth_required = https && req.local_only;
    // 원격에 실제로 공유됐을 때만 관리자에게 알린다 — local_only(오프라인)는
    // 상대방이 볼 요청이 없는데 알림만 가면 혼란스럽다. 대기열은 나중에
    // fetch 나 자동 재시도로 따라온다.
    if !req.local_only {
        notify_request(&repo, &req);
        if save_credential {
            if let Some(cred) = credentials {
                config_store::set_push_credential(&repo_id, &cred)?;
            }
        }
    }
    Ok(MergeRequestOutcome {
        request: Some(req),
        auth_required,
    })
}

#[tauri::command]
pub async fn list_requested_merges(
    repo_id: Uuid,
    base: String,
) -> AppResult<Vec<RequestedMerge>> {
    let (target, _) = resolve_target(repo_id)?;
    mr::list_requested_merges(&target, MERGE_REMOTE, &base)
}

/// 병합(승인)이 끝났거나 거절된 요청을 닫는다.
///
/// 요청 ref 를 원격에서 지우는 것도 쓰기 동작이라 HTTPS 원격이면 자격증명이
/// 필요하다 — `credentials` 는 저장된 값(푸시 자격증명)을 그대로 재사용한다.
#[tauri::command]
pub async fn close_merge_request(
    repo_id: Uuid,
    base: String,
    branch: String,
    reason: Option<String>,
    credentials: Option<config_store::PushCredential>,
) -> AppResult<()> {
    let (target, _) = resolve_target(repo_id)?;
    let _ = reason; // 요약 기록용 — 현재는 ref 삭제가 곧 닫힘이다.
    mr::close_request(
        &target,
        MERGE_REMOTE,
        &base,
        &branch,
        credentials.as_ref(),
    )
}

/// 병합 요청을 팀 알림망으로 팬아웃한다 — pre-push hook의 branch_push와 같은
/// 경로(같은 서버, 같은 스풀)를 쓴다. 서버가 죽어 있으면 스풀에 보관해 앱이
/// 살아나면 재전송한다. 실패해도 요청 자체는 이미 원격 ref로 공유됐으므로
/// 조용히 넘어간다 (fail-open).
fn notify_request(repo: &config_store::Repository, req: &MergeRequest) {
    let Ok(cfg) = config_store::load() else {
        return;
    };
    if cfg.peer.backend_url.is_empty() || cfg.peer.device_token.is_empty() {
        return;
    }
    let url = git_companion_git_normalize(&repo.remote_url);
    let payload = json!({
        "kind": "merge_request",
        "data": {
            "author": req.author,
            "message": req.title,
            "sha": req.sha,
            "repo_name": repo.display_name,
            "url": url,
            "branch": req.branch,
            "base": req.base,
        },
    })
    .to_string();
    let Ok(projects) = crate::peer::RepoProjects::load() else {
        return;
    };
    let project_ids = projects.projects_for(&repo.path);
    let backend_url = cfg.peer.backend_url.clone();
    let token = cfg.peer.device_token.clone();
    let repo_name = repo.display_name.clone();
    for project_id in project_ids {
        let sent = tokio::runtime::Runtime::new().ok().and_then(|rt| {
            rt.block_on(crate::peer::fanout_event(
                &backend_url,
                &token,
                &project_id,
                "merge_request",
                &repo_name,
                &payload,
            ))
            .ok()
        });
        if sent.is_none() {
            let _ = crate::peer::spool_event(&crate::peer::SpooledEvent {
                project_id,
                event_kind: "merge_request".into(),
                repo_name: repo_name.clone(),
                payload: payload.clone(),
            });
        }
    }
}

/// 원격 URL을 팀 알림 매칭 열쇠로 정규화한다 (hook emit과 같은 규칙).
fn git_companion_git_normalize(url: &str) -> String {
    crate::git::normalize_remote_url(url)
}
