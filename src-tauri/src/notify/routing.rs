//! 팀 이벤트 라우팅 — 수신함과 배지에 *내가 받을 알림*만 보이게 한다.
//!
//! 팀 서버는 프로젝트 전원에게 모든 push 이벤트를 배달한다(역할을 모른다).
//! 역할 판정은 각 기기의 `.gpconfig`(저장소 → 설정 탭)가 한다:
//!
//! - `branch_push` (팀원이 작업 브랜치에 푸시)
//!   → 그 베이스 브랜치의 **병합 관리자**에게만 push 알림.
//! - `merge_request` (팀원이 작업 탭에서 병합을 요청)
//!   → push 알림과 달리 **할 일이 있는** 알림 — 마찬가지로 그 베이스의
//!   병합 관리자에게만 보인다. 승인 대기열(병합 탭)로 이어진다.
//! - `main_push` (병합 대상 브랜치에 병합 반영 — 관리자가 merge 후 push)
//!   → **구성원 전체**에게 "동기화하세요" 안내.
//! - `release` → 역할 구분 없이 전원에게.
//!
//! `.gpconfig`가 없거나 저장소를 못 찾으면 이전 동작(전원 공개) 그대로 둔다 —
//! 설정 전 상태에서 알림이 조용해지는 회귀를 막는다.
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::config_store;
use crate::git::{self, Target};
use crate::gpconfig::{self, ProjectConfig};
use crate::notify::store::TeamEventRow;

/// `.gpconfig` 읽기는 작업 트리에 없을 때 `git show origin/<base>:.gpconfig`로
/// 커밋된 사본을 찾느라 git 프로세스를 띄울 수 있다 — 5초 폴링마다 반복하지
/// 않도록 짧게 캐시한다. 설정 저장 후 반영은 30초 안에 따라온다.
const CFG_TTL: Duration = Duration::from_secs(30);

static CFG_CACHE: Mutex<Option<(PathBuf, Instant, Option<(ProjectConfig, bool)>)>> =
    Mutex::new(None);

fn cached_config(repo_path: &str, base: &str) -> Option<(ProjectConfig, bool)> {
    let path = PathBuf::from(repo_path);
    if let Ok(mut guard) = CFG_CACHE.lock() {
        if let Some((p, at, val)) = guard.as_ref() {
            if *p == path && at.elapsed() < CFG_TTL {
                return val.clone();
            }
        }
        let val = gpconfig::read_config_effective(&Target::Local(path.clone()), base, "origin").ok();
        *guard = Some((path, Instant::now(), val.clone()));
        val
    } else {
        gpconfig::read_config_effective(&Target::Local(path), base, "origin").ok()
    }
}

/// 이 이벤트가 **내** 수신함/배지에 보여야 하는가.
pub fn event_visible_for_me(row: &TeamEventRow) -> bool {
    let kind = row.event_kind.trim();
    // 릴리스 태그 푸시는 역할 구분 없이 전원에게.
    if kind == "release" || kind.ends_with("release") {
        return true;
    }
    let Some((repo, cfg, exists)) = repo_context(row) else {
        return true; // 어떤 저장소인지 못 찾으면 숨기지 않는다 (기존 동작)
    };
    // .gpconfig 없음 = 아무도 역할을 지정하지 않음 → 전원 공개(기존 동작).
    if !exists {
        return true;
    }
    if kind == "branch_push" || kind.ends_with("branch_push") || kind == "merge_request" {
        if kind != "merge_request" && !cfg.notify.on_branch_ready {
            return false;
        }
        let base = base_branch_of(&cfg, &repo);
        let managers = merge_manager_emails(&cfg, &base);
        if managers.is_empty() {
            // 병합 관리자 미지정 — "누구나 병합할 수 있다"는 팀이므로 그대로 공개.
            return true;
        }
        let Some(me) = config_store::active_account().ok().flatten() else {
            // 로그아웃 상태는 관리자 판정이 불가능하다 — 남의 브랜치 푸시를
            // 내 할 일처럼 보여주지 않는다.
            return false;
        };
        let email = me.email.to_lowercase();
        if managers.iter().any(|m| *m == email) {
            return true;
        }
        // admin 은 병합 센터와 같은 규칙으로 모든 브랜치를 병합할 수 있다.
        return cfg
            .members
            .iter()
            .any(|m| m.email.trim().to_lowercase() == email && m.role == "admin");
    }
    if kind == "main_push" || kind.ends_with("main_push") {
        // 병합 반영 = 구성원 전원의 동기화 할 일. 알림 자체가 꺼져 있으면 숨긴다.
        return cfg.notify.on_merge_complete;
    }
    // 미지의 종류(구버전 서버)는 숨기지 않는다.
    true
}

/// 이벤트가 가리키는 등록 저장소 + 그 팀 설정. 매칭 열쇠는 payload의
/// remote URL (`normalize_remote_url` — `ui/lib/repoMatch.ts`와 같은 규칙).
fn repo_context(
    row: &TeamEventRow,
) -> Option<(config_store::Repository, ProjectConfig, bool)> {
    let cfg = config_store::load().ok()?;
    let url_key = payload_remote_key(&row.payload)?;
    let repo = cfg
        .repositories
        .iter()
        .find(|r| {
            let rk = git::normalize_remote_url(&r.remote_url);
            !rk.is_empty() && rk == url_key
        })?
        .clone();
    let base = if repo.default_branch.trim().is_empty() {
        "main"
    } else {
        repo.default_branch.trim()
    };
    let (pcfg, exists) = cached_config(&repo.path, base)?;
    Some((repo, pcfg, exists))
}

fn payload_remote_key(payload: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(payload).ok()?;
    let url = v.get("data")?.get("url")?.as_str()?;
    let key = git::normalize_remote_url(url);
    if key.is_empty() {
        None
    } else {
        Some(key)
    }
}

/// 병합 관리자를 찾을 베이스 브랜치 — `.gpconfig`의 기본 베이스가 우선이고,
/// 없으면 등록된 기본 브랜치, 그것도 없으면 main.
fn base_branch_of(cfg: &ProjectConfig, repo: &config_store::Repository) -> String {
    if !cfg.default_base_branch.trim().is_empty() {
        cfg.default_base_branch.trim().to_string()
    } else if !repo.default_branch.trim().is_empty() {
        repo.default_branch.trim().to_string()
    } else {
        "main".to_string()
    }
}

/// `merge_managers[branch]`는 쉼표로 구분된 이메일 목록 한 줄로 저장된다
/// (이메일에는 쉼표가 없다). 소문자로 정규화해 돌려준다.
fn merge_manager_emails(cfg: &ProjectConfig, branch: &str) -> Vec<String> {
    cfg.merge_managers
        .get(branch)
        .map(|v| {
            v.split(',')
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}