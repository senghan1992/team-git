//! Password login + seed demo accounts (config schema v8).
//!
//! These tests isolate the app config via `XDG_CONFIG_HOME` so the real
//! `~/.config/com.gitcompanion.app/config.json` is never touched.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

use git_companion::config_store::{
    active_account, delete_account, ensure_seed_accounts, hash_password, list_accounts, load,
    login_account, login_by_password, logout_account, register_account, AppSettings, SEED_ACCOUNTS,
};

/// Tests in one binary run in parallel and share the process env, so they use
/// ONE isolated config dir, serialized via a lock that is held for the whole
/// test body (returned guard), with the config file reset per test.
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
    (guard, td.path().join("config"))
}

#[test]
fn load_seeds_demo_accounts() {
    let (_g, _cfg_dir) = test_setup();
    let cfg = load().unwrap();
    assert_eq!(cfg.schema_version, 8);
    for (username, password, name, email) in SEED_ACCOUNTS {
        let acc = cfg
            .accounts
            .iter()
            .find(|a| a.username.as_deref() == Some(*username))
            .unwrap_or_else(|| panic!("seed account {username} missing"));
        assert_eq!(acc.name, *name);
        assert_eq!(acc.email, *email);
        assert_eq!(
            acc.password_hash.as_deref(),
            Some(hash_password(username, password).as_str())
        );
    }
    // Seeding is idempotent — a second load must not duplicate them.
    let again = load().unwrap();
    assert_eq!(again.accounts.len(), SEED_ACCOUNTS.len());
}

#[test]
fn login_by_password_succeeds_for_seeds() {
    let (_g, _cfg_dir) = test_setup();
    let acc = login_by_password("test", "test").unwrap();
    assert_eq!(acc.username.as_deref(), Some("test"));
    assert_eq!(active_account().unwrap().unwrap().id, acc.id);
    // username matching is case-insensitive
    let acc2 = login_by_password("TEST2", "test2").unwrap();
    assert_eq!(acc2.username.as_deref(), Some("test2"));
}

#[test]
fn login_by_password_rejects_bad_credentials() {
    let (_g, _cfg_dir) = test_setup();
    let err = login_by_password("test", "wrong").unwrap_err().to_string();
    assert!(err.contains("올바르지 않습니다"), "{err}");
    let err = login_by_password("nobody", "test").unwrap_err().to_string();
    assert!(err.contains("올바르지 않습니다"), "{err}");
    // generic message also for accounts without password support
    register_account("레거시", "legacy@example.com", None, None).unwrap();
    logout_account().unwrap(); // 등록은 자동 로그인 되므로 로그아웃 후 검증
    let err = login_by_password("legacy@example.com", "x")
        .unwrap_err()
        .to_string();
    assert!(err.contains("올바르지 않습니다"), "{err}");
    // no active account was set by any failed attempt
    assert!(active_account().unwrap().is_none());
}

#[test]
fn register_with_credentials_then_login() {
    let (_g, _cfg_dir) = test_setup();
    let acc =
        register_account("홍길동", "hong@example.com", Some("hong2"), Some("pw1234")).unwrap();
    assert_eq!(acc.username.as_deref(), Some("hong2"));
    assert!(acc.password_hash.is_some());
    assert_eq!(active_account().unwrap().unwrap().id, acc.id);

    logout_account().unwrap();
    assert!(active_account().unwrap().is_none());
    let back = login_by_password("hong2", "pw1234").unwrap();
    assert_eq!(back.id, acc.id);
    // email also works when it contains '@'
    logout_account().unwrap();
    let by_email = login_by_password("hong@example.com", "pw1234").unwrap();
    assert_eq!(by_email.id, acc.id);

    // wrong password
    assert!(login_by_password("hong2", "nope").is_err());
}

#[test]
fn register_validation_rules() {
    let (_g, _cfg_dir) = test_setup();
    // duplicate username
    let err = register_account("중복", "dup@example.com", Some("test"), Some("pw1234"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("이미 사용 중인 아이디"), "{err}");
    // duplicate email colliding with seed
    let err = register_account("중복2", "test@example.com", Some("x1"), Some("pw1234"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("이미 등록된 이메일"), "{err}");
    // too-short password
    let err = register_account("짧은", "short@example.com", Some("short1"), Some("pw"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("4자 이상"), "{err}");
    // invalid username characters
    let err = register_account("이상", "bad@example.com", Some("뭐 1"), Some("pw1234"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("영문/숫자"), "{err}");
    // empty username → legacy account (no password login)
    let acc = register_account("무아이디", "none@example.com", Some(""), Some("pw1234")).unwrap();
    assert!(acc.username.is_none());
    assert!(acc.password_hash.is_none());
}

#[test]
fn id_login_and_delete_still_work() {
    let (_g, _cfg_dir) = test_setup();
    let acc = login_by_password("test2", "test2").unwrap();
    logout_account().unwrap();
    // quick account-switch via id still works
    let back = login_account(&acc.id).unwrap();
    assert_eq!(back.id, acc.id);
    delete_account(&acc.id).unwrap();
    assert!(list_accounts().unwrap().iter().all(|a| a.id != acc.id));
    assert!(active_account().unwrap().is_none());
    // reseeded on next load
    let cfg = load().unwrap();
    assert!(cfg
        .accounts
        .iter()
        .any(|a| a.username.as_deref() == Some("test2")));
}

#[test]
fn ensure_seed_accounts_is_a_noop_when_present() {
    let (_g, _cfg_dir) = test_setup();
    let mut cfg = AppSettings::default();
    ensure_seed_accounts(&mut cfg);
    let n = cfg.accounts.len();
    assert_eq!(n, SEED_ACCOUNTS.len());
    ensure_seed_accounts(&mut cfg);
    assert_eq!(cfg.accounts.len(), n);
    // password hashes are deterministic and never equal the raw password
    for acc in &cfg.accounts {
        let uname = acc.username.as_deref().unwrap();
        assert!(acc.password_hash.as_deref().unwrap() != uname);
        assert_eq!(
            acc.password_hash.as_deref().unwrap(),
            hash_password(uname, uname)
        );
    }
}
