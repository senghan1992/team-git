//! 알림 배달 파이프라인 E2E — 지금까지 git 계층(team_sim_loop)과 분류 로직
//! (gpconfig)은 검증됐지만, **배달 경로 자체**는 한 번도 끝까지 돌린 적이
//! 없었다. 여기서는 실제 구성요소만 잇는다:
//!
//!   pre-push hook 이 부르는 실제 바이너리(`git-companion hook emit`)
//!     → 실제 FastAPI 백엔드(uvicorn 자식 프로세스, 격리 SQLite)
//!     → 기기별 poll (`peer_poll_now` 의 HTTP 루프를 그대로 재현)
//!     → 페르소나별 로컬 수신함(notify::store::Store, inbox.db)
//!     → 읽음 처리(mark_team_read / mark_all_team_read / count_unread).
//!
//! ── 격리 설계 (중요) ────────────────────────────────────────────────────────
//! config.json / peer_token / repo_projects.json / inbox.db 는 **전부**
//! `dirs::config_dir()` 아래에 산다 (config_store.rs:393-405, peer.rs:14-16,
//! store.rs:41). 그래서 "기기 6대"는 페르소나마다 XDG_CONFIG_HOME 용 TempDir
//! 을 하나씩 두고, 그 페르소나의 설정/수신함을 만지기 직전에
//! `Persona::activate()` 로 env 를 바꾸는 방식으로 흉내낸다.
//!
//! env 는 프로세스 전역이므로:
//!   * 이 파일의 모든 테스트는 accounts_session.rs 와 같은 패턴으로 공유
//!     Mutex(poisoned-lock 복구 포함)로 직렬화한다.
//!   * `hook emit` 은 자식 프로세스에 XDG_CONFIG_HOME 을 **명시적으로**
//!     넘기므로 부모의 env 전환과 경합하지 않는다.
//!   * Store 는 open 시점의 config_dir 에 파일 핸들이 고정되므로, 열어 둔
//!     핸들은 env 가 바뀐 뒤에도 자기 페르소나의 inbox.db 를 계속 가리킨다.
//!
//! ── 백엔드 서버 수명 ────────────────────────────────────────────────────────
//! 테스트마다 자유 포트(≥8100, 8000/8010/5173 회피)에 uvicorn 자식을 띄우고
//! Drop 가드로 반드시 죽인다 — OnceLock 공유 서버는 Drop 이 돌지 않아
//! uvicorn 고아를 남기므로 쓰지 않는다.
//!
//! ── 관측된 현재 동작(버그 포함)을 못박는 테스트 ─────────────────────────────
//! `// FIXME(NOTIFY-n):` 주석이 붙은 assert 는 "올바른" 동작이 아니라 **현재**
//! 동작을 문서화한다. 수정되면 그 assert 가 깨져서 알려 준다.

use std::io::{Read, Write as IoWrite};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use chrono::Utc;
use tempfile::TempDir;
use uuid::Uuid;

use git_companion::config_store::{self, Repository};
use git_companion::git::fetch::fetch_target;
use git_companion::git::merge::start_merge;
use git_companion::git::{normalize_remote_url, push, sync_to_base, Target};
use git_companion::gpconfig::{self, member_from_account, ProjectConfig};
use git_companion::notify::store::{Store, TeamEventRow};
use git_companion::peer::{self, RepoProjects};

// ── 직렬화 락 ────────────────────────────────────────────────────────────────

static LOCK: Mutex<()> = Mutex::new(());

fn serialize_tests() -> MutexGuard<'static, ()> {
    // 이전 테스트가 패닉해도 계속 진행할 수 있게 poisoned lock 을 복구한다.
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

// ── 백엔드 서버 가드 ─────────────────────────────────────────────────────────

struct Backend {
    child: Option<Child>,
    port: u16,
    db_url: String,
    stderr_path: PathBuf,
    _tmp: TempDir,
}

fn backend_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .join("backend")
}

/// 8000/8010/5173 을 피해 자유 포트를 얻는다 (에페메랄 대역은 항상 ≥ 8100).
fn free_port() -> u16 {
    loop {
        let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind :0");
        let p = l.local_addr().unwrap().port();
        if p >= 8100 && p != 8000 && p != 8010 {
            return p;
        }
    }
}

fn healthz_ok(port: u16) -> bool {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    match std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(300)) {
        Ok(mut s) => {
            let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
            if s.write_all(b"GET /healthz HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n")
                .is_err()
            {
                return false;
            }
            let mut buf = String::new();
            let _ = s.read_to_string(&mut buf);
            buf.starts_with("HTTP/1.1 200") || buf.starts_with("HTTP/1.0 200")
        }
        Err(_) => false,
    }
}

impl Backend {
    fn start() -> Backend {
        let tmp = tempfile::tempdir().expect("backend tmp");
        let db_path = tmp.path().join("peer.db");
        let db_url = format!("sqlite+pysqlite:///{}", db_path.display());
        let stderr_path = tmp.path().join("uvicorn.stderr");
        let port = free_port();
        let mut b = Backend {
            child: None,
            port,
            db_url,
            stderr_path,
            _tmp: tmp,
        };
        b.spawn();
        b.wait_healthy();
        b
    }

    fn spawn(&mut self) {
        let errf = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.stderr_path)
            .expect("stderr file");
        let child = Command::new("python3")
            .args([
                "-m",
                "uvicorn",
                "app.main:app",
                "--host",
                "127.0.0.1",
                "--port",
                &self.port.to_string(),
                "--log-level",
                "warning",
            ])
            .current_dir(backend_dir())
            .env("GC_PEER_DB_URL", &self.db_url)
            .stdout(Stdio::null())
            .stderr(Stdio::from(errf))
            .spawn()
            .expect("uvicorn spawn (python3 -m uvicorn)");
        self.child = Some(child);
    }

    fn wait_healthy(&mut self) {
        for _ in 0..150 {
            if let Some(c) = self.child.as_mut() {
                if let Ok(Some(status)) = c.try_wait() {
                    let err = std::fs::read_to_string(&self.stderr_path).unwrap_or_default();
                    panic!("uvicorn 이 조기 종료했다 ({status}):\n{err}");
                }
            }
            if healthz_ok(self.port) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let err = std::fs::read_to_string(&self.stderr_path).unwrap_or_default();
        panic!("uvicorn 이 15초 안에 /healthz 200 을 주지 않았다:\n{err}");
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// 서버 정지 — DB 파일은 남으므로 restart() 로 "재기동" 시나리오를 만든다.
    fn stop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    fn restart(&mut self) {
        self.stop();
        self.spawn();
        self.wait_healthy();
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        self.stop(); // 어떤 경로로 끝나든 uvicorn 고아를 남기지 않는다.
    }
}

// ── git 셋업 헬퍼 (team_sim_loop.rs 의 축약판) ──────────────────────────────

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .env("LC_ALL", "C.UTF-8")
        .env("LANG", "C.UTF-8")
        .args(args)
        .output()
        .expect("git spawn");
    assert!(
        out.status.success(),
        "git {args:?} failed in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn write_file(dir: &Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(p, body).unwrap();
}

fn set_identity(dir: &Path, name: &str, email: &str) {
    git(dir, &["config", "user.name", name]);
    git(dir, &["config", "user.email", email]);
    git(dir, &["config", "commit.gpgsign", "false"]);
}

fn head_sha(dir: &Path) -> String {
    git(dir, &["rev-parse", "HEAD"]).trim().to_string()
}

/// 공유 bare origin.
struct Rig {
    _bare: TempDir,
    url: String,
}

impl Rig {
    fn new() -> Rig {
        let bare = TempDir::new().unwrap();
        git(bare.path(), &["init", "--bare", "-q", "--initial-branch=main"]);
        let url = format!("file://{}", bare.path().display());
        Rig { _bare: bare, url }
    }

    /// 저장소를 처음 만든 사람 — init → 첫 커밋 → origin 등록 → push.
    fn seed(&self, name: &str, email: &str) -> TempDir {
        let dir = TempDir::new().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        set_identity(dir.path(), name, email);
        write_file(dir.path(), "README.md", "git companion\n");
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-q", "-m", "init: 프로젝트 시작"]);
        git(dir.path(), &["remote", "add", "origin", &self.url]);
        git(dir.path(), &["push", "-q", "-u", "origin", "main"]);
        git(dir.path(), &["fetch", "-q", "origin"]);
        dir
    }

    fn clone(&self, name: &str, email: &str) -> TempDir {
        let dir = TempDir::new().unwrap();
        git(dir.path(), &["clone", "-q", &self.url, "."]);
        set_identity(dir.path(), name, email);
        dir
    }
}

// ── 페르소나 = 기기 하나 (격리된 config dir + 토큰 + 수신함) ─────────────────

struct Persona {
    name: &'static str,
    home: TempDir,
    token: String,
    device_id: String,
    clone: Option<TempDir>,
}

impl Persona {
    /// 서버에 실제 등록되는 기기. peer::load_or_create_token → register_device
    /// (앱의 최초 기동 경로, commands/peer.rs ensure_device_registered 와 동일
    /// 한 순서)를 그대로 밟는다.
    fn register(rt: &tokio::runtime::Runtime, backend_url: &str, name: &'static str) -> Persona {
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", home.path());
        let token = peer::load_or_create_token().expect("token");
        let info = rt
            .block_on(peer::register_device(backend_url, &token, name))
            .expect("register_device");
        let mut cfg = config_store::load().unwrap();
        cfg.peer.backend_url = backend_url.to_string();
        cfg.peer.device_token = token.clone();
        cfg.peer.device_id = info.id.clone();
        cfg.peer.device_name = name.to_string();
        config_store::save(&cfg).unwrap();
        Persona {
            name,
            home,
            token,
            device_id: info.id,
            clone: None,
        }
    }

    /// 서버 없이 만든 기기 — 백엔드 다운 시나리오용. hook emit 은
    /// backend_url + device_token 이 비어 있지만 않으면 전송을 시도한다
    /// (main.rs:166).
    fn offline(backend_url: &str, name: &'static str) -> Persona {
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", home.path());
        let mut cfg = config_store::load().unwrap();
        cfg.peer.backend_url = backend_url.to_string();
        cfg.peer.device_token = "tok-offline-없는서버".into();
        cfg.peer.device_id = "dev-offline".into();
        cfg.peer.device_name = name.to_string();
        config_store::save(&cfg).unwrap();
        Persona {
            name,
            home,
            token: "tok-offline-없는서버".into(),
            device_id: "dev-offline".into(),
            clone: None,
        }
    }

    /// 이 페르소나의 config dir 를 현재 프로세스의 config dir 로 만든다.
    fn activate(&self) {
        std::env::set_var("XDG_CONFIG_HOME", self.home.path());
    }

    /// clone 을 이 기기의 등록 저장소로 넣고 프로젝트에 링크한다.
    /// (config.json 의 repositories + repo_projects.json — 둘 다 이 페르소나의
    /// config dir 아래.)
    fn attach_repo(
        &mut self,
        clone: TempDir,
        display_name: &str,
        default_branch: &str,
        project_ids: &[&str],
    ) {
        self.activate();
        let canon = std::fs::canonicalize(clone.path()).unwrap();
        let mut cfg = config_store::load().unwrap();
        cfg.repositories.push(Repository {
            id: Uuid::new_v4(),
            path: canon.display().to_string(),
            display_name: display_name.to_string(),
            default_branch: default_branch.to_string(),
            working_branch: String::new(),
            ssh_host: String::new(),
            ssh_user: String::new(),
            ssh_key_path: String::new(),
            ssh_password: String::new(),
            ed25519_fingerprint: String::new(),
            ssh_port: 22,
            remote_url: String::new(),
            created_at: Utc::now(),
        });
        config_store::save(&cfg).unwrap();
        let mut rp = RepoProjects::load().unwrap();
        for pid in project_ids {
            rp.link(&canon.display().to_string(), pid);
        }
        rp.save().unwrap();
        self.clone = Some(clone);
    }

    fn repo_path(&self) -> PathBuf {
        std::fs::canonicalize(self.clone.as_ref().expect("clone").path()).unwrap()
    }

    fn target(&self) -> Target {
        Target::Local(self.repo_path())
    }

    /// 이 페르소나의 로컬 수신함. open 시점에 env 를 이 페르소나로 돌리므로
    /// inbox.db 는 페르소나마다 완전히 분리된다.
    fn store(&self) -> Store {
        self.activate();
        Store::open().expect("inbox open")
    }
}

// ── hook emit — 실제 바이너리 실행 ───────────────────────────────────────────

/// templates/pre-push 가 하는 그대로 실제 바이너리를 부른다. 자식 프로세스에
/// XDG_CONFIG_HOME 을 명시적으로 실어 이 페르소나의 config.json 과
/// repo_projects.json 을 읽게 한다.
fn hook_emit(p: &Persona, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_git-companion"))
        .env("XDG_CONFIG_HOME", p.home.path())
        .arg("hook")
        .arg("emit")
        .args(args)
        .output()
        .expect("hook emit spawn")
}

fn emit_branch_push(p: &Persona, branch: &str, remote_url: &str) -> std::process::Output {
    let repo = p.repo_path();
    let sha = head_sha(&repo);
    hook_emit(
        p,
        &[
            "--event",
            "branch-push",
            "--author",
            p.name,
            "--message",
            "feat: 작업",
            "--sha",
            &sha,
            "--branch",
            branch,
            "--remote-url",
            remote_url,
            "--repo",
            &repo.display().to_string(),
        ],
    )
}

fn stdout_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

// ── poll — commands/peer.rs peer_poll_now(:234-295) 의 HTTP 루프 재현 ────────

/// 한 번의 "지금 폴링": 서버에 남은 배달을 전부 비우고(?wait=0 반복) 각
/// 이벤트를 이 페르소나의 수신함에 저장한다. 필드 매핑은 peer_poll_now 와
/// 동일하다 — 특히 `sender_device_name: event.sender_device_id`
/// (commands/peer.rs) 의 수정된 매핑(이름 우선, id 폴백)을 그대로 따른다.
fn poll_drain(
    rt: &tokio::runtime::Runtime,
    backend_url: &str,
    token: &str,
    store: &Store,
) -> Vec<TeamEventRow> {
    let client = reqwest::Client::new();
    let mut out = Vec::new();
    loop {
        let url = format!("{}/events/poll?wait=0", backend_url.trim_end_matches('/'));
        let body: serde_json::Value = rt.block_on(async {
            let resp = client
                .post(&url)
                .header("Authorization", format!("Bearer {token}"))
                .timeout(Duration::from_secs(35))
                .send()
                .await
                .expect("poll send");
            assert!(
                resp.status().is_success(),
                "poll returned {}",
                resp.status()
            );
            resp.json().await.expect("poll JSON")
        });
        let ev = match body.get("event") {
            None | Some(serde_json::Value::Null) => break,
            Some(v) => v.clone(),
        };
        // 수정된 peer_poll_now 와 동일: 이름 우선, 없으면 기기 id 폴백.
        let sender_name = ev["sender_device_name"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| ev["sender_device_id"].as_str().unwrap_or_default())
            .to_string();
        let row = TeamEventRow {
            id: Uuid::new_v4().to_string(),
            project_id: ev["project_id"].as_str().unwrap_or_default().to_string(),
            sender_device_name: sender_name,
            event_kind: ev["event_kind"].as_str().unwrap_or_default().to_string(),
            repo_name: ev["repo_name"].as_str().unwrap_or_default().to_string(),
            payload: ev["payload"].as_str().unwrap_or_default().to_string(),
            received_at: Utc::now(),
            read: false,
        };
        store.insert_team_event(&row).unwrap();
        out.push(row);
    }
    out
}

/// POST /events 는 배달 레코드 생성을 asyncio.create_task 로 미룬다
/// (backend/app/routes/events.py:61) — 이벤트 생성 직후의 poll 은 아직 빈 손일
/// 수 있다. 실제 앱은 5초 주기 폴링이라 이 창이 가려지지만, 테스트는 기대
/// 개수가 찰 때까지 짧게 재시도한다. (NOTIFY-5 로 문서화)
fn poll_until(
    rt: &tokio::runtime::Runtime,
    backend_url: &str,
    token: &str,
    store: &Store,
    expect: usize,
    secs: u64,
) -> Vec<TeamEventRow> {
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut all = poll_drain(rt, backend_url, token, store);
    while all.len() < expect && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(120));
        all.extend(poll_drain(rt, backend_url, token, store));
    }
    all
}

fn poll_status(rt: &tokio::runtime::Runtime, backend_url: &str, token: &str) -> u16 {
    rt.block_on(async {
        reqwest::Client::new()
            .post(format!(
                "{}/events/poll?wait=0",
                backend_url.trim_end_matches('/')
            ))
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
            .expect("poll send")
            .status()
            .as_u16()
    })
}

fn payload_json(row: &TeamEventRow) -> serde_json::Value {
    serde_json::from_str(&row.payload).unwrap_or_else(|e| {
        panic!("payload 가 JSON 이어야 한다: {e}: {}", row.payload)
    })
}

/// 6인 팀 .gpconfig.
fn gp(base: &str, targets: &[&str]) -> ProjectConfig {
    let mut cfg = ProjectConfig::default();
    cfg.default_base_branch = base.to_string();
    cfg.members
        .push(member_from_account("", "민지", "minji@t.com", "admin"));
    cfg.members
        .push(member_from_account("", "준호", "junho@t.com", "member"));
    cfg.merge_managers
        .insert(base.to_string(), "minji@t.com".to_string());
    cfg.merge_targets = targets.iter().map(|s| s.to_string()).collect();
    cfg
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 1 — 6대 기기 풀 루프: 준호의 branch-push 가 실제 hook 바이너리로
// 발사되어, 보낸 기기를 뺀 **모든** 프로젝트 멤버의 수신함에 도착한다.
// ═════════════════════════════════════════════════════════════════════════════
#[test]
fn n1_full_loop_six_devices_branch_push_fans_out_to_all_but_sender() {
    let _g = serialize_tests();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let backend = Backend::start();
    let rig = Rig::new();

    // 민지가 저장소 씨앗 + 프로젝트 생성, 나머지 5명은 clone + join.
    let mut minji = Persona::register(&rt, &backend.url(), "민지");
    minji.activate();
    let project = rt
        .block_on(peer::create_project(&backend.url(), &minji.token, "우리팀"))
        .unwrap();
    assert_eq!(project.role, "owner");
    assert!(
        project.join_code.contains('-'),
        "join code 는 K7H2-9XQA 꼴: {}",
        project.join_code
    );
    let minji_clone = rig.seed("민지", "minji@t.com");
    minji.attach_repo(minji_clone, "팀 저장소", "main", &[&project.id]);

    let mut members: Vec<Persona> = Vec::new();
    for name in ["준호", "도윤", "서연", "하늘", "지우"] {
        let mut p = Persona::register(&rt, &backend.url(), name);
        let joined = rt
            .block_on(peer::join_project(
                &backend.url(),
                &p.token,
                &project.join_code, // 대시 포함 코드 — 서버가 정규화한다
            ))
            .unwrap();
        assert_eq!(joined.id, project.id);
        assert_eq!(joined.role, "member");
        let email = format!("{name}@t.com");
        let clone = rig.clone(name, &email);
        p.attach_repo(clone, "팀 저장소", "main", &[&project.id]);
        members.push(p);
    }

    // 준호: 브랜치 커밋 → 진짜 push → 진짜 hook emit 바이너리.
    let junho = &members[0];
    let jrepo = junho.repo_path();
    git(&jrepo, &["checkout", "-q", "-b", "feature/junho"]);
    write_file(&jrepo, "j1.txt", "준호 작업\n");
    git(&jrepo, &["add", "-A"]);
    git(&jrepo, &["commit", "-q", "-m", "feat: j1 추가"]);
    git(&jrepo, &["push", "-q", "-u", "origin", "feature/junho"]);
    let sha = head_sha(&jrepo);

    let out = emit_branch_push(junho, "feature/junho", &rig.url);
    assert!(out.status.success(), "hook emit 실패: {}", stderr_of(&out));
    assert!(
        stdout_of(&out).contains("event posted to 1/1 project(s)"),
        "stdout: {}",
        stdout_of(&out)
    );

    // 수신자: 민지(관리자) + 도윤/서연/하늘/지우 — 서버 fanout 은 보낸 기기만
    // 제외하고 (backend/app/delivery.py:29-36) 프로젝트 멤버 전원에게 간다.
    // "관리자에게만 branch_push 알림"은 서버가 아니라 **클라이언트**
    // (ui/lib/app.ts:199-210, isMergeManagerFor)가 토스트 단계에서 거른다 —
    // 비관리자 기기의 수신함/미읽음 배지에는 그대로 쌓인다.
    let expected_url = normalize_remote_url(&rig.url);
    let mut receivers: Vec<(&Persona, Store)> = Vec::new();
    receivers.push((&minji, minji.store()));
    for p in &members[1..] {
        receivers.push((p, p.store()));
    }
    for (p, store) in &receivers {
        let got = poll_until(&rt, &backend.url(), &p.token, store, 1, 10);
        assert_eq!(got.len(), 1, "{} 는 정확히 1건 받아야 한다", p.name);
        let row = &got[0];
        assert_eq!(row.event_kind, "branch_push", "{}", p.name);
        assert_eq!(row.repo_name, "팀 저장소");
        assert_eq!(row.project_id, project.id);
        let payload = payload_json(row);
        assert_eq!(payload["kind"], "branch_push");
        assert_eq!(payload["data"]["branch"], "feature/junho");
        assert_eq!(payload["data"]["author"], "준호");
        assert_eq!(payload["data"]["sha"], sha.as_str());
        assert_eq!(payload["data"]["repo_name"], "팀 저장소");
        assert_eq!(
            payload["data"]["url"], expected_url.as_str(),
            "payload url 은 normalize_remote_url 결과여야 한다 (git/mod.rs:703)"
        );
        // 회귀 방지(NOTIFY-3): poll 응답이 sender_device_name 을 채우고
        // (backend routes/events.py _event_detail) 클라이언트가 그걸 쓴다 —
        // 폴백 경로의 알림도 보낸 사람이 이름("준호")으로 보인다.
        assert_eq!(
            row.sender_device_name, "준호",
            "보낸 사람은 기기 id 가 아니라 이름이어야 한다"
        );
        // 수신함 상태: 미읽음 1.
        assert_eq!(store.count_unread_team_events().unwrap(), 1, "{}", p.name);
    }
    // FIXME(NOTIFY-4): 위 루프가 보여주듯 비관리자(도윤 등)의 수신함에도
    // branch_push 가 저장되고 미읽음 배지(count_unread_team_events)에 잡힌다.
    // 관리자 필터는 토스트에만 적용된다(ui/lib/app.ts:199-210) — 팀원 입장에선
    // 자기가 처리할 수 없는 이벤트가 배지 숫자를 올린다.

    // 보낸 기기(준호)는 자기 이벤트를 받지 않는다 (delivery.py:33 sender 제외).
    let jstore = junho.store();
    let got = poll_drain(&rt, &backend.url(), &junho.token, &jstore);
    assert!(got.is_empty(), "보낸 사람은 자기 push 알림을 받지 않는다");
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 2 — 병합 대상 브랜치로의 push 는 hook emit 안에서 main_push 로
// 승격된다 (.gpconfig merge_targets → is_merge_target, main.rs:122-151).
// ═════════════════════════════════════════════════════════════════════════════
#[test]
fn n2_merge_target_push_is_promoted_to_main_push() {
    let _g = serialize_tests();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let backend = Backend::start();
    let rig = Rig::new();

    let mut minji = Persona::register(&rt, &backend.url(), "민지");
    minji.activate();
    let project = rt
        .block_on(peer::create_project(&backend.url(), &minji.token, "승격팀"))
        .unwrap();
    let clone = rig.seed("민지", "minji@t.com");
    minji.attach_repo(clone, "승격 저장소", "main", &[&project.id]);

    let junho = Persona::register(&rt, &backend.url(), "준호");
    rt.block_on(peer::join_project(
        &backend.url(),
        &junho.token,
        &project.join_code,
    ))
    .unwrap();
    let jstore = junho.store();

    // ── merge_targets = ["main"] 을 커밋·push ──
    let mt = minji.target();
    gpconfig::save_config(&mt, &gp("main", &["main"])).unwrap();
    assert!(gpconfig::commit_config(&mt).unwrap().ok);
    assert!(push(&mt, Some("main"), None).unwrap().ok);

    // 관리자가 main 을 push 했다 → hook 은 branch-push 로 받지만 main_push 로
    // 승격해 내보내야 한다.
    let out = emit_branch_push(&minji, "main", &rig.url);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let got = poll_until(&rt, &backend.url(), &junho.token, &jstore, 1, 10);
    assert_eq!(got.len(), 1);
    assert_eq!(
        got[0].event_kind, "main_push",
        "merge_targets=[main] 인 main push 는 main_push 로 승격"
    );
    assert_eq!(payload_json(&got[0])["kind"], "main_push");
    assert_eq!(payload_json(&got[0])["data"]["branch"], "main");

    // ── 커스텀 병합 브랜치 develop: merge_targets = ["develop"] ──
    // 워킹 트리 사본이 우선이므로(gpconfig.rs:100-115) 커밋 없이 교체해도
    // hook 이 즉시 새 규칙을 읽는다 — "언제든 바뀔 수 있으므로 매번 읽는다"
    // (main.rs:128-131)의 검증이기도 하다.
    gpconfig::save_config(&mt, &gp("develop", &["develop"])).unwrap();

    let out = emit_branch_push(&minji, "develop", &rig.url);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let got = poll_until(&rt, &backend.url(), &junho.token, &jstore, 1, 10);
    assert_eq!(got.len(), 1);
    assert_eq!(
        got[0].event_kind, "main_push",
        "커스텀 병합 브랜치(develop) push 도 main_push 로 승격"
    );

    // 같은 규칙에서 main 은 이제 병합 대상이 **아니다** — branch_push 유지.
    let out = emit_branch_push(&minji, "main", &rig.url);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let got = poll_until(&rt, &backend.url(), &junho.token, &jstore, 1, 10);
    assert_eq!(got.len(), 1);
    assert_eq!(
        got[0].event_kind, "branch_push",
        "merge_targets=[develop] 이면 main push 는 승격되지 않는다"
    );

    // 일반 feature 브랜치는 언제나 branch_push.
    let out = emit_branch_push(&minji, "feature/x", &rig.url);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let got = poll_until(&rt, &backend.url(), &junho.token, &jstore, 1, 10);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].event_kind, "branch_push");
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 3 — 한 바퀴가 계속 돈다: branch_push → 관리자 병합·main push →
// main_push → 팀원 sync → 두 번째 branch_push. 수신함 누적/읽음 처리/중복
// 없는 drain 을 검증.
// ═════════════════════════════════════════════════════════════════════════════
#[test]
fn n3_round_trip_inbox_accumulates_and_read_marking_and_no_duplicate_drain() {
    let _g = serialize_tests();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let backend = Backend::start();
    let rig = Rig::new();

    let mut minji = Persona::register(&rt, &backend.url(), "민지");
    minji.activate();
    let project = rt
        .block_on(peer::create_project(&backend.url(), &minji.token, "루프팀"))
        .unwrap();
    minji.attach_repo(rig.seed("민지", "minji@t.com"), "루프 저장소", "main", &[&project.id]);

    let mut junho = Persona::register(&rt, &backend.url(), "준호");
    rt.block_on(peer::join_project(&backend.url(), &junho.token, &project.join_code))
        .unwrap();
    junho.attach_repo(rig.clone("준호", "junho@t.com"), "루프 저장소", "main", &[&project.id]);

    let mut doyun = Persona::register(&rt, &backend.url(), "도윤");
    rt.block_on(peer::join_project(&backend.url(), &doyun.token, &project.join_code))
        .unwrap();
    doyun.attach_repo(rig.clone("도윤", "doyun@t.com"), "루프 저장소", "main", &[&project.id]);

    let minji_store = minji.store();
    let doyun_store = doyun.store();
    let junho_store = junho.store();

    // ── 라운드 1: 준호 branch push ──
    let jrepo = junho.repo_path();
    git(&jrepo, &["checkout", "-q", "-b", "feature/junho"]);
    write_file(&jrepo, "j1.txt", "라운드1\n");
    git(&jrepo, &["add", "-A"]);
    git(&jrepo, &["commit", "-q", "-m", "feat: j1"]);
    git(&jrepo, &["push", "-q", "-u", "origin", "feature/junho"]);
    assert!(emit_branch_push(&junho, "feature/junho", &rig.url).status.success());

    let got = poll_until(&rt, &backend.url(), &minji.token, &minji_store, 1, 10);
    assert_eq!(got.len(), 1, "관리자에게 branch_push 도착");
    assert_eq!(got[0].event_kind, "branch_push");
    let got = poll_until(&rt, &backend.url(), &doyun.token, &doyun_store, 1, 10);
    assert_eq!(got.len(), 1, "도윤(비관리자)에게도 서버는 배달한다");

    // ── 관리자: 병합 → main push → hook emit (main 은 등록 기본 브랜치라
    // .gpconfig 가 없어도 is_merge_target 폴백으로 main_push 승격 — 정상동작) ──
    let mt = minji.target();
    fetch_target(&mt, "origin").unwrap();
    let o = start_merge(&mt, "origin/feature/junho", "main", "origin", None).unwrap();
    assert!(o.ok && !o.conflicted, "{}", o.message);
    assert!(push(&mt, Some("main"), None).unwrap().ok);
    assert!(emit_branch_push(&minji, "main", &rig.url).status.success());

    let got = poll_until(&rt, &backend.url(), &doyun.token, &doyun_store, 1, 10);
    assert_eq!(got.len(), 1);
    assert_eq!(
        got[0].event_kind, "main_push",
        ".gpconfig 없이도 등록 기본 브랜치 push 는 main_push 로 승격 (gpconfig.rs:249-265 폴백)"
    );
    let got = poll_until(&rt, &backend.url(), &junho.token, &junho_store, 1, 10);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].event_kind, "main_push");

    // ── 팀원: 알림을 받았으니 동기화한다 (git 쪽 라운드트립 확인) ──
    let jt = junho.target();
    let r = sync_to_base(&jt, "main", "origin").unwrap();
    assert!(!r.conflicted, "{}", r.message);
    let dt = doyun.target();
    let r = sync_to_base(&dt, "main", "origin").unwrap();
    assert!(!r.conflicted, "{}", r.message);
    assert!(
        doyun.repo_path().join("j1.txt").exists(),
        "동기화 후 준호의 작업이 도윤 트리에 있다"
    );

    // ── 라운드 2: 준호 두 번째 push → 두 번째 알림 라운드 ──
    write_file(&jrepo, "j2.txt", "라운드2\n");
    git(&jrepo, &["add", "-A"]);
    git(&jrepo, &["commit", "-q", "-m", "feat: j2"]);
    git(&jrepo, &["push", "-q", "origin", "feature/junho"]);
    assert!(emit_branch_push(&junho, "feature/junho", &rig.url).status.success());

    let got = poll_until(&rt, &backend.url(), &doyun.token, &doyun_store, 1, 10);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].event_kind, "branch_push");

    // ── 수신함 누적 + 읽음 처리 ──
    let rows = doyun_store.list_team_events(50, false).unwrap();
    assert_eq!(rows.len(), 3, "도윤 수신함: branch_push, main_push, branch_push");
    assert_eq!(doyun_store.count_unread_team_events().unwrap(), 3);

    doyun_store.mark_team_read(&rows[0].id).unwrap();
    assert_eq!(doyun_store.count_unread_team_events().unwrap(), 2);
    assert_eq!(doyun_store.list_team_events(50, true).unwrap().len(), 2);

    let cleared = doyun_store.mark_all_team_read().unwrap();
    assert_eq!(cleared, 2, "모두 읽음은 남은 미읽음 수를 돌려준다");
    assert_eq!(doyun_store.count_unread_team_events().unwrap(), 0);
    assert_eq!(doyun_store.list_team_events(50, false).unwrap().len(), 3, "읽어도 행은 남는다");

    // 없는 id 읽음 처리는 깔끔한 에러.
    assert!(doyun_store.mark_team_read("없는-id").is_err());

    // ── drain 은 정확히 한 번: 즉시 재폴링 + 잠깐 뒤 재폴링 모두 0 ──
    let again = poll_drain(&rt, &backend.url(), &doyun.token, &doyun_store);
    assert!(again.is_empty(), "이미 소비한 배달이 다시 오면 안 된다: {again:?}");
    std::thread::sleep(Duration::from_millis(300));
    let again = poll_drain(&rt, &backend.url(), &doyun.token, &doyun_store);
    assert!(again.is_empty(), "시간이 지나도 중복 배달은 없어야 한다");
    assert_eq!(doyun_store.list_team_events(50, false).unwrap().len(), 3);
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 4a — 백엔드 다운: hook 은 fail-open. push 를 절대 막지 않는다.
// ═════════════════════════════════════════════════════════════════════════════
#[test]
fn n4a_backend_down_hook_emit_exits_zero_but_claims_success() {
    let _g = serialize_tests();
    let rig = Rig::new();

    // 아무도 듣지 않는 포트.
    let dead_url = format!("http://127.0.0.1:{}", free_port());
    let mut p = Persona::offline(&dead_url, "준호");
    p.attach_repo(rig.seed("준호", "junho@t.com"), "고립 저장소", "main", &["proj-어딘가"]);

    let out = emit_branch_push(&p, "feature/x", &rig.url);
    assert!(
        out.status.success(),
        "백엔드가 죽어도 hook emit 은 0 으로 끝나 push 를 살린다 (main.rs:175-189 fanout .ok()): {}",
        stderr_of(&out)
    );
    let err = stderr_of(&out);
    assert!(
        !err.contains("panicked") && !err.contains("[ERROR]"),
        "패닉/에러 출력 없이 조용히 지나가야 한다: {err}"
    );
    // 회귀 방지(NOTIFY-1): 이제 실제 결과를 말한다 — 전송 0건이면 0/1 을
    // 찍고, 실패분은 스풀(pending_events.jsonl)에 보관했다고 경고한다.
    assert!(
        stdout_of(&out).contains("[OK] event posted to 0/1 project(s)"),
        "정직한 결과 문구: {}",
        stdout_of(&out)
    );
    assert!(
        stderr_of(&out).contains("보관했습니다"),
        "스풀 경고: {}",
        stderr_of(&out)
    );
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 4b — 다운 동안 발사된 이벤트는 스풀(pending_events.jsonl)에 보관되고,
// 서버가 살아난 뒤 보낸 쪽의 폴링(flush_spooled_events)이 재전송한다.
// ═════════════════════════════════════════════════════════════════════════════
#[test]
fn n4b_events_fired_while_backend_down_are_spooled_and_resent() {
    let _g = serialize_tests();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut backend = Backend::start();
    let rig = Rig::new();

    let mut minji = Persona::register(&rt, &backend.url(), "민지");
    minji.activate();
    let project = rt
        .block_on(peer::create_project(&backend.url(), &minji.token, "정전팀"))
        .unwrap();
    minji.attach_repo(rig.seed("민지", "minji@t.com"), "정전 저장소", "main", &[&project.id]);

    let junho = Persona::register(&rt, &backend.url(), "준호");
    rt.block_on(peer::join_project(&backend.url(), &junho.token, &project.join_code))
        .unwrap();
    let jstore = junho.store();

    // 서버 정지 → push + hook emit (성공으로 끝난다) → 서버 재기동(같은 DB).
    backend.stop();
    let out = emit_branch_push(&minji, "feature/blackout", &rig.url);
    assert!(out.status.success(), "다운 중에도 push 는 산다");
    backend.restart();

    // 회귀 방지(NOTIFY-2): 다운 동안의 이벤트는 보낸 쪽 스풀에 남는다.
    minji.activate();
    let spool = git_companion::config_store::config_dir()
        .unwrap()
        .join("pending_events.jsonl");
    assert!(spool.exists(), "다운 중의 이벤트가 스풀에 보관돼야 한다");

    // 재전송 전에는 아직 서버에 없다.
    std::thread::sleep(Duration::from_millis(300));
    let got = poll_drain(&rt, &backend.url(), &junho.token, &jstore);
    assert!(got.is_empty(), "재전송 전에는 도착하지 않는다: {got:?}");

    // 보낸 쪽(민지)의 폴링이 하는 일과 동일한 재전송 — 스풀이 비워지고
    // 수신자에게 도착한다.
    let sent = rt
        .block_on(git_companion::peer::flush_spooled_events(
            &backend.url(),
            &minji.token,
        ))
        .unwrap();
    assert_eq!(sent, 1, "스풀 1건이 재전송돼야 한다");
    assert!(!spool.exists(), "재전송 후 스풀은 비워진다");
    let got = poll_until(&rt, &backend.url(), &junho.token, &jstore, 1, 10);
    assert_eq!(got.len(), 1, "정전 동안의 이벤트가 결국 도착한다");
    assert_eq!(payload_json(&got[0])["data"]["branch"], "feature/blackout");

    // 재기동 뒤의 새 이벤트는 정상 배달 — 서버 DB(기기·프로젝트)가 살아 있다.
    let out = emit_branch_push(&minji, "feature/after", &rig.url);
    assert!(out.status.success());
    let got = poll_until(&rt, &backend.url(), &junho.token, &jstore, 1, 10);
    assert_eq!(got.len(), 1, "재기동 후 파이프라인은 즉시 회복된다");
    assert_eq!(payload_json(&got[0])["data"]["branch"], "feature/after");
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 4c — 한동안 폴링하지 않은 기기: 배달 레코드는 서버에 남아 있다가
// 다음 폴링에서 생성 순서대로 전부 온다.
// ═════════════════════════════════════════════════════════════════════════════
#[test]
fn n4c_offline_device_receives_all_pending_events_in_order_on_next_poll() {
    let _g = serialize_tests();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let backend = Backend::start();

    let minji = Persona::register(&rt, &backend.url(), "민지");
    let project = rt
        .block_on(peer::create_project(&backend.url(), &minji.token, "밀린팀"))
        .unwrap();
    let doyun = Persona::register(&rt, &backend.url(), "도윤");
    rt.block_on(peer::join_project(&backend.url(), &doyun.token, &project.join_code))
        .unwrap();

    // 도윤이 한 번도 폴링하지 않는 동안 이벤트 3발 (fanout_event 직접 —
    // hook emit 의 최종 전송 함수와 동일 경로, peer.rs:202-245).
    for n in 1..=3 {
        rt.block_on(peer::fanout_event(
            &backend.url(),
            &minji.token,
            &project.id,
            "branch_push",
            "밀린 저장소",
            &format!("{{\"seq\":{n}}}"),
        ))
        .unwrap();
        std::thread::sleep(Duration::from_millis(40)); // created_at 순서 보장
    }

    let dstore = doyun.store();
    let got = poll_until(&rt, &backend.url(), &doyun.token, &dstore, 3, 10);
    assert_eq!(got.len(), 3, "밀린 3건이 모두 도착해야 한다");
    let seqs: Vec<i64> = got
        .iter()
        .map(|r| payload_json(r)["seq"].as_i64().unwrap())
        .collect();
    assert_eq!(
        seqs,
        vec![1, 2, 3],
        "PushEvent.created_at 순으로 배달된다 (routes/events.py:88-90)"
    );
    assert_eq!(dstore.count_unread_team_events().unwrap(), 3);
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 4d — 서버가 모르는 토큰의 폴링은 401. 그 토큰으로 register 하면
// (서버가 제시된 토큰을 그대로 채택, devices.py:42-56 멱등) 폴링이 회복된다 —
// ensure_device_registered(commands/peer.rs:27-52)의 회복 경로.
// ═════════════════════════════════════════════════════════════════════════════
#[test]
fn n4d_unknown_token_gets_401_then_register_with_same_token_recovers() {
    let _g = serialize_tests();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let backend = Backend::start();

    let fresh_token = format!("{}{}", Uuid::new_v4(), Uuid::new_v4());
    assert_eq!(
        poll_status(&rt, &backend.url(), &fresh_token),
        401,
        "미등록 토큰의 폴링은 401 (deps.py:42-44)"
    );

    // register 가 제시된 베어러 토큰을 채택한다.
    let info = rt
        .block_on(peer::register_device(&backend.url(), &fresh_token, "새 기기"))
        .unwrap();
    assert_eq!(
        poll_status(&rt, &backend.url(), &fresh_token),
        200,
        "등록 직후 같은 토큰으로 폴링이 된다"
    );

    // 멱등성: 같은 토큰 재등록은 새 기기를 만들지 않는다.
    let again = rt
        .block_on(peer::register_device(&backend.url(), &fresh_token, "다른 이름"))
        .unwrap();
    assert_eq!(again.id, info.id, "재등록은 기존 기기를 돌려준다 (devices.py:46-56)");
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 4e — config 에 없는 저장소로 hook emit: 패닉 없이 깨끗한 에러 +
// 종료코드 1. push 는 hook 스크립트의 `|| true`(templates/pre-push:20) 덕에
// 스크립트 계층에서 살아남는다 — 여기서는 바이너리 동작만 검증한다.
// ═════════════════════════════════════════════════════════════════════════════
#[test]
fn n4e_unregistered_repo_fails_cleanly_without_panic() {
    let _g = serialize_tests();
    let rig = Rig::new();

    let p = Persona::offline("http://127.0.0.1:8990", "준호"); // 저장소 0개 config
    let stray = rig.seed("준호", "junho@t.com"); // 존재하지만 등록 안 된 repo

    let out = hook_emit(
        &p,
        &[
            "--event",
            "branch-push",
            "--author",
            "준호",
            "--message",
            "m",
            "--sha",
            "deadbeef",
            "--branch",
            "feature/x",
            "--remote-url",
            &rig.url,
            "--repo",
            &stray.path().display().to_string(),
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "등록 안 된 저장소는 종료코드 1 (main.rs:17-24)"
    );
    let err = stderr_of(&out);
    assert!(
        err.contains("repo not registered"),
        "친절한 에러 메시지 (main.rs:108): {err}"
    );
    assert!(!err.contains("panicked"), "패닉이 아니어야 한다: {err}");

    // 존재하지 않는 경로도 마찬가지로 깨끗하게 실패한다 (canonicalize 에러).
    let out = hook_emit(
        &p,
        &[
            "--event",
            "branch-push",
            "--author",
            "준호",
            "--message",
            "m",
            "--sha",
            "deadbeef",
            "--branch",
            "b",
            "--remote-url",
            "u",
            "--repo",
            "/없는/경로/repo",
        ],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(!stderr_of(&out).contains("panicked"));
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 4f — 한 저장소가 두 프로젝트에 링크되어 있으면 이벤트는 두 프로젝트
// 모두로 팬아웃된다 (main.rs:171-188, RepoProjects::projects_for).
// ═════════════════════════════════════════════════════════════════════════════
#[test]
fn n4f_repo_linked_to_two_projects_fans_out_to_both() {
    let _g = serialize_tests();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let backend = Backend::start();
    let rig = Rig::new();

    let mut minji = Persona::register(&rt, &backend.url(), "민지");
    minji.activate();
    let proj_a = rt
        .block_on(peer::create_project(&backend.url(), &minji.token, "프로젝트A"))
        .unwrap();
    let proj_b = rt
        .block_on(peer::create_project(&backend.url(), &minji.token, "프로젝트B"))
        .unwrap();
    minji.attach_repo(
        rig.seed("민지", "minji@t.com"),
        "겹치는 저장소",
        "main",
        &[&proj_a.id, &proj_b.id],
    );

    let doyun = Persona::register(&rt, &backend.url(), "도윤");
    for code in [&proj_a.join_code, &proj_b.join_code] {
        rt.block_on(peer::join_project(&backend.url(), &doyun.token, code))
            .unwrap();
    }

    let out = emit_branch_push(&minji, "feature/dual", &rig.url);
    assert!(out.status.success(), "{}", stderr_of(&out));
    assert!(
        stdout_of(&out).contains("event posted to 2/2 project(s)"),
        "두 프로젝트로 발사: {}",
        stdout_of(&out)
    );

    let dstore = doyun.store();
    let got = poll_until(&rt, &backend.url(), &doyun.token, &dstore, 2, 10);
    assert_eq!(got.len(), 2, "프로젝트마다 한 건씩, 총 2건");
    let mut pids: Vec<&str> = got.iter().map(|r| r.project_id.as_str()).collect();
    pids.sort_unstable();
    let mut expect = [proj_a.id.as_str(), proj_b.id.as_str()];
    expect.sort_unstable();
    assert_eq!(pids, expect, "서로 다른 두 프로젝트에서 온 같은 push");
    for r in &got {
        assert_eq!(r.event_kind, "branch_push");
        assert_eq!(payload_json(r)["data"]["branch"], "feature/dual");
    }

    // 보낸 기기는 어느 프로젝트에서도 받지 않는다.
    let mstore = minji.store();
    assert!(poll_drain(&rt, &backend.url(), &minji.token, &mstore).is_empty());
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 5 — 페이로드 무결성: 한글 저장소/브랜치/작성자/메시지가 수신함까지
// 바이트 그대로 살아남고, 원격 URL 의 자격증명은 반드시 벗겨진다.
// ═════════════════════════════════════════════════════════════════════════════
#[test]
fn n5_korean_payload_survives_byte_exact_and_credentials_are_stripped() {
    let _g = serialize_tests();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let backend = Backend::start();
    let rig = Rig::new();

    let mut junho = Persona::register(&rt, &backend.url(), "준호");
    junho.activate();
    let project = rt
        .block_on(peer::create_project(&backend.url(), &junho.token, "한글팀 ★"))
        .unwrap();
    junho.attach_repo(
        rig.seed("준호", "junho@t.com"),
        "우리 저장소 ★",
        "main",
        &[&project.id],
    );

    let minji = Persona::register(&rt, &backend.url(), "민지");
    rt.block_on(peer::join_project(&backend.url(), &minji.token, &project.join_code))
        .unwrap();

    let author = "준호";
    let message = "기능: 한글 메시지 — 로그인 화면 🚀";
    let branch = "기능/한글-브랜치";
    let cred_url = "http://user:token123@Host.example.com/team/한글저장소.git";
    let repo = junho.repo_path();
    let sha = head_sha(&repo);

    let out = hook_emit(
        &junho,
        &[
            "--event",
            "branch-push",
            "--author",
            author,
            "--message",
            message,
            "--sha",
            &sha,
            "--branch",
            branch,
            "--remote-url",
            cred_url,
            "--repo",
            &repo.display().to_string(),
        ],
    );
    assert!(out.status.success(), "{}", stderr_of(&out));

    let mstore = minji.store();
    let got = poll_until(&rt, &backend.url(), &minji.token, &mstore, 1, 10);
    assert_eq!(got.len(), 1);
    let row = &got[0];

    // 행 자체의 필드 — 한글 그대로.
    assert_eq!(row.repo_name, "우리 저장소 ★");
    assert_eq!(row.event_kind, "branch_push");

    // 페이로드 — 파이프라인(hook 바이너리 → HTTP → 서버 DB → poll → SQLite)
    // 을 다 지나고도 바이트 그대로.
    let payload = payload_json(row);
    assert_eq!(payload["data"]["author"], author);
    assert_eq!(payload["data"]["message"], message);
    assert_eq!(payload["data"]["branch"], branch);
    assert_eq!(payload["data"]["repo_name"], "우리 저장소 ★");
    assert_eq!(payload["data"]["sha"], sha.as_str());

    // 자격증명 스트리핑 (git/mod.rs:703-727 normalize_remote_url, main.rs:96-101).
    assert_eq!(
        payload["data"]["url"], "host.example.com/team/한글저장소",
        "scheme/userinfo/.git 제거 + 호스트 소문자"
    );
    assert!(
        !row.payload.contains("token123") && !row.payload.contains("user:"),
        "payload 어디에도 자격증명이 남으면 안 된다: {}",
        row.payload
    );

    // 수신함 SQLite 를 다시 읽어도 동일 — 저장/조회 왕복 무손실.
    let listed = mstore.list_team_events(10, false).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].payload, row.payload, "DB 왕복 후에도 바이트 동일");
    assert_eq!(listed[0].repo_name, "우리 저장소 ★");
}
