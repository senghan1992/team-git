pub mod accounts;
pub mod ai;
pub mod commands;
pub mod config_store;
pub mod error;
pub mod git;
pub mod google_login;
pub mod gpconfig;
pub mod notify;
pub mod peer;
pub mod pre_push_hook;

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::Manager;

/// X(닫기) 버튼은 창을 숨길 뿐 종료하지 않는다 — 트레이에서 복귀할 수 있게.
/// '종료' 메뉴로 나갈 때만 true 로 바꾸고 정말 끈다.
static QUITTING: AtomicBool = AtomicBool::new(false);
/// 창을 숨긴 뒤 "트레이에 있습니다" 안내는 앱 켤 때 한 번만.
static TRAY_HINT_SENT: AtomicBool = AtomicBool::new(false);

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // 창이 트레이에 숨어 있을 때 앱을 다시 실행하면 새 프로세스 대신
            // 기존 창을 다시 꺼내 준다.
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let _ = setup_tray(app.handle());
            let cfg_dir = match config_store::config_dir() {
                Ok(d) => d,
                Err(_) => return Ok(()),
            };
            // 등록된 로컬 저장소의 pre-push hook을 매 실행마다 최신 템플릿으로
            // 갱신한다. 앱을 업데이트해도 예전 hook이 남아 알림 분류가 틀리는
            // 일을 막는다. 실패해도 앱 실행은 계속한다 (hook은 fail-open).
            if let Ok(cfg) = config_store::load() {
                for repo in &cfg.repositories {
                    if !repo.ssh_host.is_empty() {
                        continue; // 원격(SSH) 저장소에는 로컬 hook을 걸 수 없다.
                    }
                    let path = std::path::Path::new(&repo.path);
                    if path.join(".git").exists() {
                        let _ = pre_push_hook::install(path);
                    }
                }
            }

            let peer = config_store::load()
                .map(|cfg| cfg.peer.clone())
                .unwrap_or_default();
            if peer.device_token.is_empty() {
                return Ok(());
            }
            let dir = cfg_dir;
            let backend_url = if peer.backend_url.is_empty() {
                "http://127.0.0.1:8000".into()
            } else {
                peer.backend_url.clone()
            };
            let _peer_token = peer.device_token;
            std::thread::spawn(move || {
                let port_path = dir.join("peer_port");
                let sidecar_path = std::env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(|p| p.join("gc-peer-listener")))
                    .unwrap_or_else(|| std::path::PathBuf::from("gc-peer-listener"));
                let child = crate::git::new_command(&sidecar_path.to_string_lossy())
                    .env(
                        "GC_PEER_DB",
                        dir.join("inbox.db").to_string_lossy().to_string(),
                    )
                    .env("GC_PEER_PORT", "0")
                    .env("GC_BACKEND_URL", &backend_url)
                    // 리스너가 "부모가 죽으면 함께 죽기" 위한 값 — 앱이 크래시로
                    // 죽거나 설치기(taskkill)에 강제 종료돼도 리스너가 exe 파일을
                    // 잠근 채 남지 않게 한다.
                    .env("GC_PARENT_PID", std::process::id().to_string())
                    .stdout(std::process::Stdio::piped())
                    .spawn();
                if let Ok(mut child) = child {
                    // Windows: 잡 객체에 넣어 두면 앱 프로세스가 죽는 순간
                    // 리스너도 즉시 내려간다 (부모 감시 폴링보다 더 확실).
                    #[cfg(windows)]
                    crate::win_job::assign_kill_on_close(child.id());
                    if let Some(stdout) = child.stdout.take() {
                        use std::io::{BufRead, BufReader};
                        let mut buf = String::new();
                        if BufReader::new(stdout).read_line(&mut buf).is_ok() {
                            if let Ok(port) = buf.trim().parse::<u16>() {
                                let _ = std::fs::write(&port_path, port.to_string());
                            }
                        }
                    }
                }
            });
            Ok(())
        })
        // X(닫기)를 누르면 종료하지 않고 트레이로 숨긴다. '종료' 메뉴로 나갈
        // 때(QUITTING)만 진짜로 내려간다. google-login 처럼 보조 창은 예외
        // — 정상적으로 닫혀야 한다 (닫힘 = 로그인 취소).
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if !QUITTING.load(Ordering::Relaxed) && window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                    // 처음 숨길 때만 "숨은 게 아니라 트레이에 남았다"를 알린다.
                    if !TRAY_HINT_SENT.swap(true, Ordering::Relaxed) {
                        use tauri_plugin_notification::NotificationExt;
                        let _ = window
                            .app_handle()
                            .notification()
                            .builder()
                            .title("Git Companion")
                            .body(
                                "앱이 꺼지지 않고 트레이 아이콘에 남아 있습니다. \
                                 트레이 아이콘을 클릭하면 언제든 다시 열 수 있습니다.",
                            )
                            .show();
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            // repo commands
            commands::repo::list_repositories,
            commands::repo::register_repository,
            commands::repo::init_repository,
            commands::repo::browse_ssh_dir,
            commands::repo::remove_repository,
            commands::repo::update_repository,
            // git ops commands
            commands::git::list_branches,
            commands::git::list_commits,
            commands::git::status,
            commands::git::add_files,
            commands::git::commit,
            commands::git::push,
            commands::project::push_credentials_list,
            commands::project::push_credential_set,
            commands::project::push_credential_delete,
            commands::account::account_register,
            commands::account::account_login_by_password,
            google_login::google_login_start,
            commands::account::account_logout,
            commands::account::account_current,
            commands::account::account_refresh,
            commands::account::account_update_profile,
            commands::account::account_change_password,
            commands::account::account_delete_self,
            commands::account::account_search,
            commands::project::project_config_get,
            commands::project::project_config_set,
            commands::project::project_config_commit,
            commands::git::fetch_repo,
            commands::git::list_pending_branches,
            commands::git::start_merge,
            commands::git::merge_state,
            commands::git::base_unpushed_count,
            commands::git::branch_file_diff,
            commands::git::list_merged_remote_branches,
            commands::git::merge_timeline,
            commands::git::delete_remote_branch,
            commands::git::conflict_detail,
            commands::git::resolve_conflict,
            commands::git::abort_merge,
            commands::git::complete_merge,
            commands::auto::merge_auto_resolve,
            commands::auto::merge_backup_list,
            commands::auto::merge_backup_restore,
            commands::auto::sync_branch,
            commands::config::get_ai_config,
            commands::config::set_ai_config,
            commands::config::ai_default_prompt,
            commands::git::pull,
            commands::git::diff,
            commands::git::stash,
            commands::git::stash_list,
            commands::git::create_branch,
            commands::git::checkout_branch,
            // config / SSH
            commands::config::get_ssh_profile,
            commands::config::set_ssh_profile,
            commands::config::test_ssh_connection,
            // peer / team commands
            commands::peer::peer_register_device,
            commands::peer::peer_create_project,
            commands::peer::peer_join_project,
            commands::peer::peer_list_projects,
            commands::peer::peer_link_repo_to_project,
            commands::peer::peer_unlink_repo,
            commands::peer::peer_repos_for_project,
            commands::peer::peer_local_url,
            commands::peer::peer_get_config,
            commands::peer::peer_set_backend_url,
            commands::peer::peer_check_backend,
            commands::peer::peer_poll_now,
            commands::peer::peer_leave_project,
            commands::peer::peer_list_team_events,
            commands::peer::peer_mark_team_read,
            commands::peer::peer_mark_all_team_read,
            commands::peer::peer_unread_count,
            commands::ai::ai_suggest_resolution,
            commands::peer::peer_invite_by_email,
            commands::peer::peer_list_members,
            commands::peer::peer_remove_email_invite,
        ])
        .run(tauri::generate_context!())
        .expect("failed to launch tauri application");
}

/// 시스템 트레이(우측 하단) 아이콘. 왼쪽 클릭으로 창을 열고/숨기고,
/// 메뉴로 창 토글과 종료를 고른다.
fn setup_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    let toggle = MenuItem::with_id(app, "toggle", "창 열기 / 숨기기", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Git Companion 종료", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&toggle, &quit])?;

    let mut builder = TrayIconBuilder::with_id("main-tray")
        .tooltip("Git Companion")
        .menu(&menu)
        // 왼쪽 클릭은 창 토글, 메뉴는 오른쪽 클릭으로.
        .show_menu_on_left_click(false);
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }
    builder = builder.on_menu_event(|app, event| match event.id().as_ref() {
        "toggle" => toggle_main_window(app),
        "quit" => {
            QUITTING.store(true, Ordering::Relaxed);
            app.exit(0);
        }
        _ => {}
    });
    // 아이콘 자체를 클릭해도 창이 열고 닫힌다 (메뉴는 오른쪽 클릭).
    builder = builder.on_tray_icon_event(|tray, event| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            toggle_main_window(tray.app_handle());
        }
    });
    builder.build(app)?;
    Ok(())
}

fn toggle_main_window(app: &tauri::AppHandle) {
    use tauri::Manager;
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
    } else {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Windows 전용: 자식 프로세스를 "잡 객체"에 넣어 앱 프로세스가 끝나면
/// (정상 종료든, 크래시든, 설치기가 taskkill 로 강제 종료하든) 자식도 함께
/// 죽게 만든다. 백그라운드 리스너(gc-peer-listener)가 혼자 남아 exe 파일을
/// 잠그는 일이 없도록 하는 것이 목적 — 그 잠금 때문에 새 버전 설치가
/// "Error opening file for writing" 으로 실패했었다.
#[cfg(windows)]
mod win_job {
    use std::sync::OnceLock;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
    };

    /// 앱 수명 동안 유지되는 잡 (KILL_ON_JOB_CLOSE). 핸들을 OnceLock 에
    /// 넣어 두면 프로세스가 끝날 때 OS 가 함께 정리한다.
    /// HANDLE(=*mut c_void) 은 Send/Sync 가 아니므로 래퍼로 감싼다.
    #[derive(Clone, Copy)]
    struct JobHandle(HANDLE);
    unsafe impl Send for JobHandle {}
    unsafe impl Sync for JobHandle {}

    static JOB: OnceLock<JobHandle> = OnceLock::new();

    fn job() -> Option<HANDLE> {
        let handle = JOB
            .get_or_init(|| unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if job.is_null() {
                    return JobHandle(std::ptr::null_mut());
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if ok == 0 {
                    CloseHandle(job);
                    return JobHandle(std::ptr::null_mut());
                }
                JobHandle(job)
            })
            .0;
        (!handle.is_null()).then_some(handle)
    }

    /// `pid` 를 잡에 넣는다. 실패(이미 다른 잡 소속 등)는 조용히 무시한다 —
    /// 리스너 쪽의 부모 감시(polling)가 대신 막아 준다.
    pub fn assign_kill_on_close(pid: u32) {
        let Some(job) = job() else {
            return;
        };
        unsafe {
            let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
            if process.is_null() {
                return;
            }
            let _ = AssignProcessToJobObject(job, process);
            CloseHandle(process);
        }
    }
}
