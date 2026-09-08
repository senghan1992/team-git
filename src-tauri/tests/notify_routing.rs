//! 알림 라우팅 (notify/routing.rs) — "누가 어떤 알림을 받는가".
//!
//! 팀 서버는 프로젝트 전원에게 모든 push 이벤트를 배달하므로, 역할 판정은
//! 각 기기의 `.gpconfig`(저장소 → 설정 탭)가 한다:
//!   - `branch_push` → 기본 베이스 브랜치의 병합 관리자(+admin)에게만
//!   - `main_push`   → 구성원 전체 (동기화 안내)
//!   - `release`     → 전원
//! `.gpconfig`가 없거나(설정 전) 저장소를 못 찾으면 전원 공개 — 라우팅 도입
//! 전 동작을 그대로 유지한다.
//!
//! 실제 `~/.config` 를 건드리지 않도록 `XDG_CONFIG_HOME` 으로 격리한다.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

use chrono::Utc;
use serde_json::json;

use git_companion::config_store::{self, Account, AppSettings, Repository};
use git_companion::notify::routing::event_visible_for_me;
use git_companion::notify::store::TeamEventRow;

const REMOTE_URL: &str = "git@github.com:team/app.git";

static LOCK: Mutex<()> = Mutex::new(());
static HOME: OnceLock<tempfile::TempDir> = OnceLock::new();

fn test_setup() -> (MutexGuard<'static, ()>, PathBuf) {
    // 이전 테스트가 패닉해도 계속 진행할 수 있게 poisoned lock 을 복구한다.
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let td = HOME.get_or_init(|| {
        let d = tempfile::tempdir().unwrap();
        let cfg_root = d.path().join("config");
        fs::create_dir_all(&cfg_root).unwrap();
        env::set_var("XDG_CONFIG_HOME", &cfg_root);
        d
    });
    (guard, td.path().to_path_buf())
}

fn account(name: &str, email: &str) -> Account {
    Account {
        id: email.into(),
        name: name.into(),
        email: email.into(),
        username: name.into(),
        created_at: "2026-09-02T00:00:00Z".into(),
    }
}

/// 시나리오별로 **다른** 저장소 경로를 쓴다 — routing.rs 의 30초 설정 캐시는
/// 경로를 키로 쓰므로, 경로가 같으면 두 번째 시나리오가 첫 번째 .gpconfig 를
/// 읽는다.

/// `gpconfig_json`: Some(문자열)이면 저장소 루트에 `.gpconfig` 로 기록한다.
/// `me`: 로그인 계정 (None = 로그아웃).
/// 락은 호출한 테스트가 이미 잡고 있다 (test_setup 의 가드가 본문 전체를 덮는다).
fn world(home: &std::path::Path, scenario: &str, me: Option<&Account>, gpconfig_json: Option<&str>) {
    let repo_path = home.join(format!("repos/{scenario}"));
    fs::create_dir_all(&repo_path).unwrap();
    if let Some(body) = gpconfig_json {
        fs::write(repo_path.join(".gpconfig"), body).unwrap();
    }

    let mut s = AppSettings::default();
    s.repositories.push(Repository {
        id: uuid::Uuid::new_v4(),
        path: repo_path.to_string_lossy().to_string(),
        display_name: "app".into(),
        default_branch: "main".into(),
        working_branch: String::new(),
        ssh_host: String::new(),
        ssh_user: String::new(),
        ssh_key_path: String::new(),
        ssh_password: String::new(),
        ed25519_fingerprint: String::new(),
        ssh_port: 22,
        remote_url: REMOTE_URL.into(),
        created_at: Utc::now(),
    });
    if let Some(me) = me {
        s.session = Some(config_store::SessionState {
            user: me.clone(),
            token: "t".into(),
        });
    }
    config_store::save(&s).unwrap();
}

/// 팀 설정 JSON — 관리자는 lead@x.com, admin 은 tae@x.com, 구성원은 minji@x.com.
fn team_config(notify: &str) -> String {
    format!(
        r#"{{
  "gpconfig_version": 3,
  "default_base_branch": "main",
  "members": [
    {{"id": "1", "name": "Minji", "email": "minji@x.com", "role": "member"}},
    {{"id": "2", "name": "Tae", "email": "tae@x.com", "role": "admin"}},
    {{"id": "3", "name": "Lead", "email": "lead@x.com", "role": "member"}}
  ],
  "merge_managers": {{"main": "lead@x.com"}},
  "merge_targets": ["main"],
  "notify_recipients": [],
  "notify": {notify}
}}"#
    )
}

fn row(kind: &str, branch: &str, url: &str) -> TeamEventRow {
    TeamEventRow {
        id: format!("evt-{kind}-{branch}"),
        project_id: "p1".into(),
        sender_device_name: "Minji".into(),
        event_kind: kind.into(),
        repo_name: "app".into(),
        payload: json!({
            "kind": kind,
            "data": { "branch": branch, "url": url, "author": "Minji", "message": "wip" }
        })
        .to_string(),
        received_at: Utc::now(),
        read: false,
    }
}

const DEFAULT_NOTIFY: &str = r#"{"on_branch_ready": true, "on_merge_complete": true}"#;
const OFF_BRANCH: &str = r#"{"on_branch_ready": false, "on_merge_complete": true}"#;
const OFF_MERGE: &str = r#"{"on_branch_ready": true, "on_merge_complete": false}"#;
const OFF_BOTH: &str = r#"{"on_branch_ready": false, "on_merge_complete": false}"#;

#[test]
fn no_gpconfig_keeps_legacy_broadcast() {
    let (_g, home) = test_setup();
    let _w = world(&home, "no-cfg", Some(&account("Lead", "lead@x.com")), None);
    // 설정 전 상태: 역할 판정 없이 모두에게 보인다.
    assert!(event_visible_for_me(&row("branch_push", "feat/x", REMOTE_URL)));
    assert!(event_visible_for_me(&row("main_push", "main", REMOTE_URL)));
    assert!(event_visible_for_me(&row("release", "", REMOTE_URL)));
}

#[test]
fn unknown_repo_falls_back_to_visible() {
    let (_g, home) = test_setup();
    let _w = world(&home, "unknown-repo", Some(&account("Lead", "lead@x.com")), None);
    // 내가 등록한 저장소가 아닌 이벤트는 숨길 근거가 없다 — 기존처럼 보인다.
    assert!(event_visible_for_me(&row(
        "branch_push",
        "feat/x",
        "git@github.com:other/team.git"
    )));
}

#[test]
fn branch_push_goes_to_the_base_manager_only() {
    // lead = main 브랜치의 병합 관리자 → 브랜치 푸시가 보인다.
    let (_g, home) = test_setup();
    let _w = world(&home, "manager", Some(&account("Lead", "lead@x.com")), Some(&team_config(DEFAULT_NOTIFY)));
    assert!(event_visible_for_me(&row("branch_push", "feat/x", REMOTE_URL)));
    assert!(event_visible_for_me(&row("main_push", "main", REMOTE_URL)));
}

#[test]
fn branch_push_hidden_from_plain_member_when_manager_assigned() {
    // minji = 일반 구성원. 병합 관리자가 따로 있으면 남의 브랜치 푸시는
    // 내 할 일이 아니다 — 수신함에 보여주지 않는다.
    let (_g, home) = test_setup();
    let _w = world(
        &home,
        "member",
        Some(&account("Minji", "minji@x.com")),
        Some(&team_config(DEFAULT_NOTIFY)),
    );
    assert!(!event_visible_for_me(&row("branch_push", "feat/x", REMOTE_URL)));
    // 병합 반영(동기화 안내)은 구성원 전원의 몫이다.
    assert!(event_visible_for_me(&row("main_push", "main", REMOTE_URL)));
}

#[test]
fn admin_sees_branch_push_like_a_manager() {
    // tae = admin. 병합 센터와 같은 규칙으로 모든 브랜치를 병합할 수 있다.
    let (_g, home) = test_setup();
    let _w = world(
        &home,
        "admin",
        Some(&account("Tae", "tae@x.com")),
        Some(&team_config(DEFAULT_NOTIFY)),
    );
    assert!(event_visible_for_me(&row("branch_push", "feat/x", REMOTE_URL)));
}

#[test]
fn signed_out_user_sees_no_body_elses_branch_push() {
    // 로그아웃 상태는 관리자 판정이 불가능하다 — 남의 브랜치 푸시는 숨기고,
    // 동기화 안내는 그대로 받는다.
    let (_g, home) = test_setup();
    let _w = world(&home, "anon", None, Some(&team_config(DEFAULT_NOTIFY)));
    assert!(!event_visible_for_me(&row("branch_push", "feat/x", REMOTE_URL)));
    assert!(event_visible_for_me(&row("main_push", "main", REMOTE_URL)));
}

#[test]
fn notify_flags_gate_each_channel() {
    // on_branch_ready=false → 관리자라도 브랜치 푸시 알림이 꺼진다.
    let (_g, home) = test_setup();
    let _w = world(
        &home,
        "flag-branch",
        Some(&account("Lead", "lead@x.com")),
        Some(&team_config(OFF_BRANCH)),
    );
    assert!(!event_visible_for_me(&row("branch_push", "feat/x", REMOTE_URL)));
    assert!(event_visible_for_me(&row("main_push", "main", REMOTE_URL)));

    // on_merge_complete=false → 동기화 안내가 꺼진다.
    let _w2 = world(
        &home,
        "flag-merge",
        Some(&account("Minji", "minji@x.com")),
        Some(&team_config(OFF_MERGE)),
    );
    assert!(!event_visible_for_me(&row("main_push", "main", REMOTE_URL)));

    // 둘 다 꺼도 release 알림은 남는다.
    let _w3 = world(
        &home,
        "flag-both",
        Some(&account("Minji", "minji@x.com")),
        Some(&team_config(OFF_BOTH)),
    );
    assert!(event_visible_for_me(&row("release", "", REMOTE_URL)));
}