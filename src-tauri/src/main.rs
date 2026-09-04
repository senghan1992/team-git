//! Entry point. Handles the `hook emit` subcommand (used by the pre-push hook)
//! and the GUI application.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use git_companion::config_store;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && args.get(1).map(|s| s.as_str()) == Some("hook") {
        let rest: &[String] = if args.get(2).map(|s| s.as_str()) == Some("emit") {
            &args[3..]
        } else {
            &args[2..]
        };
        let code = match run_hook_subcommand(rest) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("[ERROR] {e}");
                1
            }
        };
        std::process::exit(code);
    }
    // OpenSSH SSH_ASKPASS 헬퍼 — sshpass 가 없는 환경(Windows, 기본 macOS)에서
    // 비밀번호 SSH 인증을 가능하게 하는 통로다. ssh 는 헬퍼를 서브커맨드 인자
    // 없이 `<이 앱> <프롬프트>` 형태로 호출한다 (프롬프트는 "password" /
    // "passphrase" 를 담는 문장). 그 모양일 때만 askpass 로 동작하고, 평소 GUI
    // 실행(인자 없음)이나 `hook` 서브커맨드와는 겹치지 않는다. 비밀번호는
    // 부모(앱의 ssh 실행)가 `SSHPASS` 환경변수로 내려 준 값을 그대로 출력한다.
    let is_askpass = args.get(1).map(|s| s.as_str() == "askpass").unwrap_or(false)
        || (args.len() == 2
            && args[1] != "hook"
            && (args[1].ends_with(':') || {
                let lower = args[1].to_ascii_lowercase();
                lower.contains("password") || lower.contains("passphrase")
            }));
    if is_askpass {
        if let Ok(p) = std::env::var("SSHPASS") {
            println!("{p}");
        }
        return;
    }
    run_gui();
}

fn run_gui() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();
    if let Err(e) = config_store::ensure_dirs() {
        eprintln!("config init failed: {e}");
    }
    git_companion::run();
}

/// Minimal event enum for the hook subcommand — mirrors what peer backend expects.
#[derive(Debug, serde::Serialize)]
#[serde(tag = "kind", content = "data")]
enum HookEvent {
    #[serde(rename = "main_push")]
    MainPush {
        author: String,
        message: String,
        sha: String,
        repo_name: String,
        url: String,
        branch: String,
    },
    #[serde(rename = "branch_push")]
    BranchPush {
        author: String,
        message: String,
        sha: String,
        repo_name: String,
        url: String,
        branch: String,
    },
    #[serde(rename = "release")]
    Release {
        author: String,
        repo_name: String,
        url: String,
        version: String,
    },
}

impl HookEvent {
    fn event_kind_str(&self) -> &'static str {
        match self {
            HookEvent::MainPush { .. } => "main_push",
            HookEvent::BranchPush { .. } => "branch_push",
            HookEvent::Release { .. } => "release",
        }
    }
}

#[derive(Debug)]
struct HookArgs {
    event: String,
    author: String,
    message: String,
    sha: String,
    branch: Option<String>,
    version: Option<String>,
    remote_url: String,
    repo: String,
}

fn run_hook_subcommand(args: &[String]) -> anyhow::Result<()> {
    let mut parsed = parse_hook_args(args)?;
    // payload는 팀 전체에 배달된다 — ① `https://user:token@host/…`의
    // 자격증명이 그대로 나가면 push 토큰 유출이고, ② 받는 쪽은 이 URL로
    // 자기 등록 저장소를 찾으므로 (폴더 이름은 사람마다 다르다) 양쪽이
    // 같은 규칙으로 정규화되어 있어야 한다.
    parsed.remote_url = git_companion::git::normalize_remote_url(&parsed.remote_url);
    let repo_path = std::fs::canonicalize(&parsed.repo)?;
    let cfg = config_store::load()?;
    let repo = cfg
        .repositories
        .iter()
        .find(|r| PathBuf::from(&r.path) == repo_path || r.path == parsed.repo)
        .ok_or_else(|| anyhow::anyhow!("repo not registered: {}", parsed.repo))?;

    let event: HookEvent = match parsed.event.as_str() {
        "main-push" => HookEvent::MainPush {
            author: parsed.author.clone(),
            message: parsed.message.clone(),
            sha: parsed.sha.clone(),
            repo_name: repo.display_name.clone(),
            url: parsed.remote_url.clone(),
            branch: parsed
                .branch
                .clone()
                .unwrap_or_else(|| repo.default_branch.clone()),
        },
        "branch-push" => {
            let branch = parsed
                .branch
                .clone()
                .ok_or_else(|| anyhow::anyhow!("branch required for branch-push"))?;
            // 병합 브랜치로의 푸시는 "팀원들이 동기화해야 한다"는 뜻이므로
            // main_push 로 승격한다. 어떤 브랜치가 병합 브랜치인지는 .gpconfig
            // 가 정하고 (main 이 아닐 수도 있다) 언제든 바뀔 수 있으므로 매번
            // 읽는다. hook 은 fail-open 이므로 설정을 못 읽으면 그냥
            // branch_push 로 둔다.
            if is_merge_target(&repo_path, &repo.default_branch, &branch) {
                HookEvent::MainPush {
                    author: parsed.author.clone(),
                    message: parsed.message.clone(),
                    sha: parsed.sha.clone(),
                    repo_name: repo.display_name.clone(),
                    url: parsed.remote_url.clone(),
                    branch,
                }
            } else {
                HookEvent::BranchPush {
                    author: parsed.author.clone(),
                    message: parsed.message.clone(),
                    sha: parsed.sha.clone(),
                    repo_name: repo.display_name.clone(),
                    url: parsed.remote_url.clone(),
                    branch,
                }
            }
        }
        "release" => HookEvent::Release {
            author: parsed.author.clone(),
            repo_name: repo.display_name.clone(),
            url: parsed.remote_url.clone(),
            version: parsed
                .version
                .clone()
                .ok_or_else(|| anyhow::anyhow!("version required for release"))?,
        },
        other => return Err(anyhow::anyhow!("unknown event {other}")),
    };

    // Fan out to peer backend for every linked project.
    let peer_cfg = &cfg.peer;
    if !peer_cfg.backend_url.is_empty() && !peer_cfg.device_token.is_empty() {
        let peer_backend_url = peer_cfg.backend_url.clone();
        let peer_token = peer_cfg.device_token.clone();
        let peer_event_kind = event.event_kind_str();
        let peer_repo_name = repo.display_name.clone();
        let repo_projects = git_companion::peer::RepoProjects::load().unwrap_or_default();
        let project_ids_vec = repo_projects.projects_for(&repo_path.to_string_lossy());
        let payload = serde_json::to_string(&event).unwrap_or_default();
        let n_projects = project_ids_vec.len();
        let mut posted = 0usize;
        let mut spooled = 0usize;
        for project_id in project_ids_vec {
            let sent = tokio::runtime::Runtime::new().ok().and_then(|rt| {
                rt.block_on(git_companion::peer::fanout_event(
                    &peer_backend_url,
                    &peer_token,
                    &project_id,
                    peer_event_kind,
                    &peer_repo_name,
                    &payload,
                ))
                .ok()
            });
            if sent.is_some() {
                posted += 1;
            } else {
                // 서버가 죽어 있어도 알림을 버리지 않는다 — 스풀에 보관해
                // 앱 폴링이 서버가 살아나면 재전송한다. (push 자체는 어떤
                // 경우에도 막지 않는다 — fail-open.)
                let ok = git_companion::peer::spool_event(&git_companion::peer::SpooledEvent {
                    project_id: project_id.clone(),
                    event_kind: peer_event_kind.to_string(),
                    repo_name: peer_repo_name.clone(),
                    payload: payload.clone(),
                })
                .is_ok();
                if ok {
                    spooled += 1;
                }
            }
        }
        // 예전에는 실패해도 "posted to N"을 찍었다 — 시도 수가 아니라
        // 실제 결과를 말한다.
        println!("[OK] event posted to {posted}/{n_projects} project(s)");
        if spooled > 0 {
            eprintln!(
                "[WARN] 서버에 전송하지 못한 알림 {spooled}건을 보관했습니다 — 앱이 서버와 연결되면 자동 재전송됩니다."
            );
        }
    } else {
        println!("[OK] no peer backend configured; event not sent");
    }

    Ok(())
}

/// `branch` 가 이 프로젝트의 병합 대상 브랜치인지.
///
/// `.gpconfig` 의 `merge_targets` 가 우선이고, 비어 있으면
/// `default_base_branch`, 그것도 없으면 앱에 등록된 기본 브랜치를 쓴다.
/// 설정을 읽지 못하면 등록된 기본 브랜치와만 비교한다 (hook 은 절대
/// push 를 막지 않는다).
fn is_merge_target(repo_path: &std::path::Path, registered_default: &str, branch: &str) -> bool {
    let target = git_companion::git::Target::Local(repo_path.to_path_buf());
    // push 시점의 체크아웃 브랜치에는 .gpconfig 사본이 없을 수 있다 —
    // 병합 브랜치에 커밋된 팀 규칙까지 찾아 읽는다.
    let (cfg, exists) =
        match git_companion::gpconfig::read_config_effective(&target, registered_default, "origin")
        {
            Ok(v) => v,
            Err(_) => (Default::default(), false),
        };
    git_companion::gpconfig::is_merge_target(&cfg, exists, registered_default, branch)
}

fn parse_hook_args(args: &[String]) -> anyhow::Result<HookArgs> {
    let mut event = String::new();
    let mut author = String::new();
    let mut message = String::new();
    let mut sha = String::new();
    let mut branch = None;
    let mut version = None;
    let mut remote_url = String::new();
    let mut repo = String::new();
    let mut i = 0;
    while i < args.len() {
        let key = &args[i];
        let val = args
            .get(i + 1)
            .ok_or_else(|| anyhow::anyhow!("missing value for {key}"))?;
        match key.as_str() {
            "--event" => event = val.clone(),
            "--author" => author = val.clone(),
            "--message" => message = val.clone(),
            "--sha" => sha = val.clone(),
            "--branch" => branch = Some(val.clone()),
            "--version" => version = Some(val.clone()),
            "--remote-url" => remote_url = val.clone(),
            "--repo" => repo = val.clone(),
            _ => return Err(anyhow::anyhow!("unknown arg {key}")),
        }
        i += 2;
    }
    if event.is_empty() || repo.is_empty() {
        return Err(anyhow::anyhow!("--event and --repo are required"));
    }
    Ok(HookArgs {
        event,
        author,
        message,
        sha,
        branch,
        version,
        remote_url,
        repo,
    })
}
