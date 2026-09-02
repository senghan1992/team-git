//! 로그인 세션 캐시 (config schema v9).
//!
//! 계정 자체는 팀 서버의 `users` 테이블(SQLite)이 소유한다. 이 앱의 설정
//! 파일에는 **지금 로그인한 사람과 토큰**만 남기므로, 여기서는 그 캐시의
//! 동작과 옛 설정 파일의 이행만 검증한다. 서버와 주고받는 부분은
//! `backend/tests/test_accounts.py` 가 검증한다.
//!
//! 실제 `~/.config` 를 건드리지 않도록 `XDG_CONFIG_HOME` 으로 격리한다.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

use git_companion::config_store::{
    active_account, clear_session, load, save, save_session, session_token, Account, AppSettings,
};

/// 한 바이너리 안의 테스트는 병렬로 돌면서 프로세스 env 를 공유하므로, 격리된
/// 설정 디렉터리 하나를 쓰고 테스트 본문 전체를 락으로 직렬화한다.
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
    let cfg_file = td
        .path()
        .join("config")
        .join(git_companion::config_store::APP_DIR)
        .join(git_companion::config_store::CONFIG_FILE);
    let _ = fs::remove_file(&cfg_file); // 시작 상태로 리셋
    (guard, cfg_file)
}

fn sample() -> Account {
    Account {
        id: "6f1d2c3b4a5e6f7a8b9c0d1e2f3a4b5c".into(),
        name: "홍길동".into(),
        email: "hong@example.com".into(),
        username: "hong".into(),
        created_at: "2026-09-02T00:00:00Z".into(),
    }
}

#[test]
fn fresh_config_is_signed_out_and_seeds_no_accounts() {
    let (_g, _p) = test_setup();
    let cfg = load().unwrap();
    assert_eq!(cfg.schema_version, 9);
    assert!(cfg.session.is_none(), "새 설정은 로그아웃 상태여야 한다");
    assert!(active_account().unwrap().is_none());
    assert!(session_token().unwrap().is_none());
}

#[test]
fn save_session_then_read_back() {
    let (_g, _p) = test_setup();
    let me = sample();
    save_session(&me, "tok-abc").unwrap();

    let back = active_account().unwrap().expect("로그인 상태여야 한다");
    assert_eq!(back, me, "저장한 사용자가 그대로 돌아와야 한다");
    assert_eq!(session_token().unwrap().as_deref(), Some("tok-abc"));

    // 파일을 다시 읽어도 유지된다 — 앱 재시작 후 로그인 유지.
    let cfg = load().unwrap();
    assert_eq!(cfg.session.as_ref().unwrap().user.email, "hong@example.com");
}

#[test]
fn save_session_replaces_the_previous_one() {
    let (_g, _p) = test_setup();
    save_session(&sample(), "tok-1").unwrap();
    let other = Account {
        id: "aaaa1111bbbb2222cccc3333dddd4444".into(),
        name: "김민지".into(),
        email: "minji@example.com".into(),
        username: "minji".into(),
        created_at: "2026-09-02T01:00:00Z".into(),
    };
    save_session(&other, "tok-2").unwrap();

    // 계정 목록이 쌓이지 않는다 — "지금 로그인한 사람" 하나만 남는다.
    assert_eq!(active_account().unwrap().unwrap().username, "minji");
    assert_eq!(session_token().unwrap().as_deref(), Some("tok-2"));
}

#[test]
fn clear_session_signs_out_but_keeps_other_settings() {
    let (_g, _p) = test_setup();
    let mut cfg = load().unwrap();
    cfg.ssh_profile.default_user = "ec2-user".into();
    save(&cfg).unwrap();

    save_session(&sample(), "tok-abc").unwrap();
    clear_session().unwrap();

    assert!(active_account().unwrap().is_none());
    assert!(session_token().unwrap().is_none());
    assert_eq!(
        load().unwrap().ssh_profile.default_user,
        "ec2-user",
        "로그아웃이 다른 설정을 지우면 안 된다"
    );
}

/// v8 이전 설정 파일에는 로컬 `accounts` 배열과 `active_account_id` 가 있었다.
/// serde 가 모르는 키를 버리므로, 업그레이드하면 **한 번 로그아웃**되고 그
/// 뒤로는 서버로 로그인한다. 파일이 깨지거나 앱이 열리지 않아서는 안 된다.
#[test]
fn old_config_with_local_accounts_opens_signed_out() {
    let (_g, cfg_file) = test_setup();
    fs::create_dir_all(cfg_file.parent().unwrap()).unwrap();
    fs::write(
        &cfg_file,
        r#"{
          "schema_version": 8,
          "repositories": [],
          "accounts": [
            {"id":"1a2b3c4d-0000-0000-0000-000000000000","name":"테스트 1",
             "email":"test@example.com","username":"test",
             "password_hash":"deadbeef","created_at":"2026-01-01T00:00:00Z"}
          ],
          "active_account_id": "1a2b3c4d-0000-0000-0000-000000000000",
          "ssh_profile": {"default_user":"keep-me"}
        }"#
        .as_bytes(),
    )
    .unwrap();

    let cfg = load().unwrap();
    assert_eq!(cfg.schema_version, 9, "스키마가 올라가야 한다");
    assert!(
        cfg.session.is_none(),
        "옛 로컬 계정으로 로그인된 상태가 되면 안 된다"
    );
    assert_eq!(
        cfg.ssh_profile.default_user, "keep-me",
        "다른 설정은 그대로 살아남아야 한다"
    );
}

/// 토큰이 설정 파일에 들어가므로, 무엇이 저장되는지 분명히 해 둔다 —
/// 비밀번호는 어떤 형태로도 이 파일에 남지 않는다.
#[test]
fn session_stores_a_token_and_never_a_password() {
    let (_g, cfg_file) = test_setup();
    save_session(&sample(), "tok-secret").unwrap();
    let raw = fs::read_to_string(&cfg_file).unwrap();
    assert!(
        raw.contains("tok-secret"),
        "토큰은 저장된다 (로그인 유지용)"
    );
    assert!(
        !raw.contains("password_hash"),
        "비밀번호 해시도 앱에 저장하지 않는다 — 서버만 갖는다"
    );
    // 세션 블록 자체에는 user + token 만 들어간다 — 로그인 비밀번호는 어떤
    // 키로도 남지 않는다. (ssh_profile.default_password 는 SSH 접속용으로
    // 문서화된 별개 설정이므로 파일 전체가 아니라 세션만 본다.)
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let session = parsed
        .get("session")
        .and_then(|s| s.as_object())
        .expect("session 블록이 있어야 한다");
    let mut keys: Vec<&str> = session.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, vec!["token", "user"], "세션에는 user와 token만 남는다");
    let user_json = serde_json::to_string(session.get("user").unwrap()).unwrap();
    assert!(
        !user_json.contains("password"),
        "세션 사용자에 비밀번호 필드가 있어서는 안 된다"
    );
}

#[test]
fn settings_default_is_signed_out() {
    let s = AppSettings::default();
    assert!(s.session.is_none());
}
