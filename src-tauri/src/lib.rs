pub mod accounts;
pub mod ai;
pub mod commands;
pub mod config_store;
pub mod error;
pub mod git;
pub mod gpconfig;
pub mod notify;
pub mod peer;
pub mod pre_push_hook;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_shell::init())
        .setup(|_app| {
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
                    .stdout(std::process::Stdio::piped())
                    .spawn();
                if let Ok(mut child) = child {
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
            // config / SSH / external tools
            commands::config::get_ssh_profile,
            commands::config::set_ssh_profile,
            commands::config::test_ssh_connection,
            commands::config::list_external_tools,
            commands::config::set_external_tool,
            commands::config::remove_external_tool,
            commands::external::open_external_tool,
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
