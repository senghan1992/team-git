//! 알림 배달 파이프라인 — **널뛰는(flapping) 서버** 아래에서의 혼돈 시험.
//!
//! team_sim_notify.rs 가 "맑은 날"의 배달 경로를 검증했다면, 여기서는 팀
//! 서버가 죽었다 살아나기를 반복하는 동안 "다른 팀원이 한 push 를 끝내
//! 모르게 되는 일은 없다"를 못박는다. 실제 구성요소만 잇는 것은 동일하다:
//!
//!   실제 바이너리(`git-companion hook emit`)
//!     → 실제 FastAPI 백엔드(uvicorn 자식, 격리 SQLite — 재기동 시 같은 DB)
//!     → 보낸 쪽 오프라인 스풀(pending_events.jsonl, peer.rs:221-285)
//!     → 기기별 poll(commands/peer.rs peer_poll_now 의 HTTP 루프 재현)
//!     → 페르소나별 로컬 수신함(notify/store.rs, inbox.db).
//!
//! 격리 설계(페르소나 = XDG_CONFIG_HOME TempDir, 전역 Mutex 직렬화,
//! Drop 가드로 uvicorn 필살)는 team_sim_notify.rs 의 패턴을 그대로 쓴다.
//! 이 파일의 포트는 전부 ≥ 8200 이다.
//!
//! ── 코드로 못박아 둔 알려진 창(window) ──────────────────────────────────────
//! * NCHAOS-1 (문서화): POST /events 는 202-성격이다 — 이벤트 행을 커밋한 뒤
//!   배달 레코드 생성을 `asyncio.create_task(queue_event(...))` 로 미룬다
//!   (backend/app/routes/events.py:73-77, delivery.py:38-48). 응답과 create_task
//!   커밋 사이에 서버가 죽으면 PushEvent 는 남지만 EventDelivery 가 0줄이고,
//!   재기동 후 이를 메꾸는 복구 스캔이 없다 → 그 push 는 아무에게도 배달되지
//!   않는다. 프로세스 킬 타이밍을 결정적으로 만들 수 없어 테스트로 재현하지
//!   않고, 이 파일의 모든 "정지"는 마지막 전송 후 1초 뒤에 일어난다.
//! * NCHAOS-2 (문서화 + 인접 보장 테스트 c4): 서버는 poll **응답 시점**에
//!   delivered_at 을 찍고(routes/events.py:110-113, 137-139) 클라이언트는 응답을
//!   받은 **뒤에** 수신함에 넣는다(commands/peer.rs:296). 그 사이에 수신 앱이
//!   죽으면 서버는 배달됐다고 믿고 다시 주지 않는다 — ack 엔드포인트
//!   (routes/events.py:144-184, acked_at)는 존재하지만 클라이언트가 호출하지
//!   않는다. c4 는 인접 보장(아직 넘겨받지 않은 것은 절대 잃지 않음)을 검증한다.
//!
//! NCHAOS-3(poll 401 시 자동 재등록)·NCHAOS-4(4xx 로 거부된 스풀 줄 폐기)는
//! 고쳐졌다 — c3 이 새 동작을 회귀-방지로 고정한다. NCHAOS-1/2 는 여전히
//! 문서로만 못박아 둔 창이다.

use std::io::{Read, Write as IoWrite};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use chrono::Utc;
use tempfile::TempDir;
use uuid::Uuid;

use git_companion::config_store::{self, Repository};
use git_companion::notify::store::{Store, TeamEventRow};
use git_companion::peer::{self, RepoProjects, SpooledEvent};

// ── 직렬화 락 (env 는 프로세스 전역이다) ─────────────────────────────────────

static LOCK: Mutex<()> = Mutex::new(());

fn serialize_tests() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

// ── 백엔드 서버 가드 ─────────────────────────────────────────────────────────

struct Backend {
    child: Option<Child>,
    port: u16,
    db_url: String,
    stderr_path: PathBuf,
    tmp: TempDir,
}

fn backend_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .join("backend")
}

/// 이 파일의 규약: 포트는 항상 ≥ 8200 (기본 8000/8010, notify 스위트의 8100
/// 대역, vite 5173 을 전부 피한다).
fn free_port() -> u16 {
    loop {
        let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind :0");
        let p = l.local_addr().unwrap().port();
        if p >= 8200 {
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
            tmp,
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

    /// 정지 — DB 파일은 남는다. restart() 가 "같은 DB로 재기동"이다.
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

    /// 서버 재설치/초기화 시나리오: **완전히 빈 새 DB** 로 같은 포트에 재기동.
    /// 기기 토큰·프로젝트·배달 레코드가 전부 사라진다.
    fn restart_fresh_db(&mut self) {
        self.stop();
        let db_path = self
            .tmp
            .path()
            .join(format!("peer-fresh-{}.db", Uuid::new_v4()));
        self.db_url = format!("sqlite+pysqlite:///{}", db_path.display());
        self.spawn();
        self.wait_healthy();
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        self.stop(); // 어떤 경로로 끝나든 uvicorn 고아를 남기지 않는다.
    }
}

// ── git 셋업 헬퍼 ────────────────────────────────────────────────────────────

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

    fn seed(&self, name: &str, email: &str) -> TempDir {
        let dir = TempDir::new().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        set_identity(dir.path(), name, email);
        write_file(dir.path(), "README.md", "git companion chaos\n");
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-q", "-m", "init: 혼돈 시작"]);
        git(dir.path(), &["remote", "add", "origin", &self.url]);
        git(dir.path(), &["push", "-q", "-u", "origin", "main"]);
        dir
    }

    fn clone(&self, name: &str, email: &str) -> TempDir {
        let dir = TempDir::new().unwrap();
        git(dir.path(), &["clone", "-q", &self.url, "."]);
        set_identity(dir.path(), name, email);
        dir
    }
}

// ── 페르소나 = 기기 하나 ─────────────────────────────────────────────────────

struct Persona {
    name: &'static str,
    home: TempDir,
    token: String,
    device_id: String,
    clone: Option<TempDir>,
}

impl Persona {
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

    fn activate(&self) {
        std::env::set_var("XDG_CONFIG_HOME", self.home.path());
    }

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

    fn store(&self) -> Store {
        self.activate();
        Store::open().expect("inbox open")
    }

    fn spool_path(&self) -> PathBuf {
        self.activate();
        config_store::config_dir().unwrap().join("pending_events.jsonl")
    }
}

// ── hook emit — 실제 바이너리 ────────────────────────────────────────────────

fn hook_emit(p: &Persona, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_git-companion"))
        .env("XDG_CONFIG_HOME", p.home.path())
        .arg("hook")
        .arg("emit")
        .args(args)
        .output()
        .expect("hook emit spawn")
}

/// pre-push 가 부르는 그대로 — sha 자리에 마커를 실어 payload 추적을 가능하게
/// 한다 (hook 바이너리는 sha 를 불투명 문자열로 취급한다).
fn emit_push(p: &Persona, branch: &str, sha_marker: &str, remote_url: &str) -> Output {
    let repo = p.repo_path();
    hook_emit(
        p,
        &[
            "--event",
            "branch-push",
            "--author",
            p.name,
            "--message",
            "chaos: 작업",
            "--sha",
            sha_marker,
            "--branch",
            branch,
            "--remote-url",
            remote_url,
            "--repo",
            &repo.display().to_string(),
        ],
    )
}

fn stdout_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// hook 공통 불변식: push 를 절대 막지 않는다(종료 0) + 패닉 없음.
fn assert_hook_fail_open(out: &Output, ctx: &str) {
    assert!(
        out.status.success(),
        "[{ctx}] hook emit 은 서버 상태와 무관하게 0 으로 끝나야 push 가 산다: {}",
        stderr_of(out)
    );
    assert!(
        !stderr_of(out).contains("panicked"),
        "[{ctx}] hook 은 패닉하면 안 된다: {}",
        stderr_of(out)
    );
}

// ── poll — commands/peer.rs peer_poll_now 의 HTTP 루프 (오류 허용 버전) ──────
//
// c4 가 "폴링 도중 서버가 죽는" 상황을 다루므로, notify 스위트의 panic 버전과
// 달리 Result 를 돌려준다. 필드 매핑은 peer_poll_now(commands/peer.rs:274-296)
// 와 동일 — 관용적(lenient) 매핑: 없는 필드는 빈 문자열, payload 는 불투명.

fn try_poll_once(
    rt: &tokio::runtime::Runtime,
    backend_url: &str,
    token: &str,
) -> Result<Option<serde_json::Value>, String> {
    rt.block_on(async {
        let client = reqwest::Client::new();
        let resp = client
            .post(format!(
                "{}/events/poll?wait=0",
                backend_url.trim_end_matches('/')
            ))
            .header("Authorization", format!("Bearer {token}"))
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| format!("poll send failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("poll returned {}", resp.status()));
        }
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("poll JSON: {e}"))?;
        Ok(match body.get("event") {
            None | Some(serde_json::Value::Null) => None,
            Some(v) => Some(v.clone()),
        })
    })
}

fn store_polled(store: &Store, ev: &serde_json::Value) -> TeamEventRow {
    let get = |k: &str| -> String {
        ev.get(k)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let sender = {
        let name = get("sender_device_name");
        if name.is_empty() {
            get("sender_device_id")
        } else {
            name
        }
    };
    let row = TeamEventRow {
        id: Uuid::new_v4().to_string(),
        project_id: get("project_id"),
        sender_device_name: sender,
        event_kind: get("event_kind"),
        repo_name: get("repo_name"),
        payload: get("payload"),
        received_at: Utc::now(),
        read: false,
    };
    store.insert_team_event(&row).unwrap();
    row
}

fn poll_drain(
    rt: &tokio::runtime::Runtime,
    backend_url: &str,
    token: &str,
    store: &Store,
) -> Result<Vec<TeamEventRow>, String> {
    let mut out = Vec::new();
    loop {
        match try_poll_once(rt, backend_url, token)? {
            None => break,
            Some(ev) => out.push(store_polled(store, &ev)),
        }
    }
    Ok(out)
}

/// POST /events 의 배달 레코드 생성이 비동기(routes/events.py:77)라 이벤트
/// 직후의 poll 은 빈 손일 수 있다 — 기대 개수가 찰 때까지 재시도한다.
fn poll_until(
    rt: &tokio::runtime::Runtime,
    backend_url: &str,
    token: &str,
    store: &Store,
    expect: usize,
    secs: u64,
) -> Vec<TeamEventRow> {
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut all = poll_drain(rt, backend_url, token, store).expect("poll drain");
    while all.len() < expect && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(120));
        all.extend(poll_drain(rt, backend_url, token, store).expect("poll drain"));
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
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .expect("poll send")
            .status()
            .as_u16()
    })
}

// ── 마커 payload — 이벤트 하나하나를 끝까지 추적하는 지문 ────────────────────

/// fanout_event 로 보내는 이벤트의 payload. hook emit 이벤트는 sha 필드가
/// 마커를 실으므로, 어느 경로든 `row.payload.contains(marker)` 로 잡힌다.
fn chaos_payload(kind: &str, author: &str, marker: &str) -> String {
    serde_json::json!({
        "kind": kind,
        "data": {
            "author": author,
            "message": "chaos: 작업",
            "sha": marker,
            "branch": format!("chaos/{marker}"),
            "repo_name": "팀 저장소",
            "marker": marker,
        }
    })
    .to_string()
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 1 — 플랩 폭풍 + 전수 회계: 6대 기기, 이벤트 15발을
// up → down → up(같은 DB) → down → up 스케줄 사이사이에 발사한다.
// 다운 중의 실패분은 보낸 쪽 스풀에 남고, 마지막 up 후 각 송신자의
// flush_spooled_events → 전 수신자 drain. 회계 불변식: 발사된 모든 이벤트가
// 보낸 기기를 뺀 **모든** 멤버의 수신함에 ≥1 행 — 어떤 push 도 끝내 모르게
// 되지 않는다. 중복은 허용(at-least-once)하되 정직하게 집계해 보고한다.
// ═════════════════════════════════════════════════════════════════════════════
#[test]
fn c1_flap_storm_no_event_is_ever_silently_unknowable() {
    let _g = serialize_tests();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut backend = Backend::start();
    let rig = Rig::new();
    let url = backend.url();

    // ── 팀 셋업: 민지(관리자) + 5명, 프로젝트 1개, 송신자마다 링크된 저장소 ──
    let mut minji = Persona::register(&rt, &url, "민지");
    minji.activate();
    let project = rt
        .block_on(peer::create_project(&url, &minji.token, "혼돈팀"))
        .unwrap();
    minji.attach_repo(rig.seed("민지", "minji@t.com"), "팀 저장소", "main", &[&project.id]);

    let mut personas: Vec<Persona> = vec![minji];
    for name in ["준호", "도윤", "서연", "하늘", "지우"] {
        let mut p = Persona::register(&rt, &url, name);
        rt.block_on(peer::join_project(&url, &p.token, &project.join_code))
            .unwrap();
        let email = format!("{name}@t.com");
        p.attach_repo(rig.clone(name, &email), "팀 저장소", "main", &[&project.id]);
        personas.push(p);
    }
    let idx = |name: &str| personas.iter().position(|p| p.name == name).unwrap();
    let (mj, jh, dy, sy, hn, jw) = (
        idx("민지"),
        idx("준호"),
        idx("도윤"),
        idx("서연"),
        idx("하늘"),
        idx("지우"),
    );

    // 발사 기록: (마커, 송신자 인덱스). 마커는 두 자리 0-패딩이라 부분문자열
    // 충돌이 없다 (evt01 은 evt10 과 겹치지 않는다).
    let mut fired: Vec<(String, usize)> = Vec::new();
    let mk = |n: usize| format!("nchaos1evt{n:02}");

    // 서버 살아있을 때의 fanout — 반드시 성공.
    let live_fanout = |sender: usize,
                       kind: &str,
                       marker: &str,
                       fired: &mut Vec<(String, usize)>,
                       personas: &[Persona]| {
        let p = &personas[sender];
        rt.block_on(peer::fanout_event(
            &url,
            &p.token,
            &project.id,
            kind,
            "팀 저장소",
            &chaos_payload(kind, p.name, marker),
        ))
        .unwrap_or_else(|e| panic!("{} 의 live fanout({marker}) 실패: {e:?}", p.name));
        fired.push((marker.to_string(), sender));
    };
    // 서버 죽었을 때의 fanout — 실패를 확인하고, hook 이 하는 그대로
    // (main.rs:195-205) 보낸 쪽 스풀에 보관한다.
    let down_fanout = |sender: usize,
                       kind: &str,
                       marker: &str,
                       fired: &mut Vec<(String, usize)>,
                       personas: &[Persona]| {
        let p = &personas[sender];
        let payload = chaos_payload(kind, p.name, marker);
        let r = rt.block_on(peer::fanout_event(
            &url, &p.token, &project.id, kind, "팀 저장소", &payload,
        ));
        assert!(r.is_err(), "서버가 죽어 있으니 fanout 은 실패해야 한다: {marker}");
        p.activate();
        peer::spool_event(&SpooledEvent {
            project_id: project.id.clone(),
            event_kind: kind.to_string(),
            repo_name: "팀 저장소".to_string(),
            payload,
        })
        .unwrap();
        fired.push((marker.to_string(), sender));
    };
    // 실제 hook 바이너리 발사 — up 이든 down 이든 종료 0 (push 는 절대 안 막힘).
    let hook_fire = |sender: usize,
                     branch: &str,
                     marker: &str,
                     server_up: bool,
                     fired: &mut Vec<(String, usize)>,
                     personas: &[Persona]| {
        let p = &personas[sender];
        let out = emit_push(p, branch, marker, &rig.url);
        assert_hook_fail_open(&out, marker);
        if server_up {
            assert!(
                stdout_of(&out).contains("event posted to 1/1 project(s)"),
                "[{marker}] up 상태의 hook 은 1/1 전송: {}",
                stdout_of(&out)
            );
        } else {
            assert!(
                stdout_of(&out).contains("event posted to 0/1 project(s)"),
                "[{marker}] down 상태의 hook 은 정직하게 0/1: {}",
                stdout_of(&out)
            );
            assert!(
                stderr_of(&out).contains("보관했습니다"),
                "[{marker}] down 상태의 hook 은 스풀 보관을 경고: {}",
                stderr_of(&out)
            );
        }
        fired.push((marker.to_string(), sender));
    };

    // ── UP #1: evt01 은 진짜 커밋·push 까지 밟는다 ──
    {
        let jrepo = personas[jh].repo_path();
        git(&jrepo, &["checkout", "-q", "-b", "feature/chaos-j1"]);
        write_file(&jrepo, "chaos1.txt", "준호 혼돈 작업\n");
        git(&jrepo, &["add", "-A"]);
        git(&jrepo, &["commit", "-q", "-m", "feat: 혼돈 1"]);
        git(&jrepo, &["push", "-q", "-u", "origin", "feature/chaos-j1"]);
    }
    hook_fire(jh, "feature/chaos-j1", &mk(1), true, &mut fired, &personas);
    // 관리자의 main push — .gpconfig 없이도 등록 기본 브랜치라 main_push 승격.
    hook_fire(mj, "main", &mk(2), true, &mut fired, &personas);
    live_fanout(dy, "branch_push", &mk(3), &mut fired, &personas);
    live_fanout(sy, "branch_push", &mk(4), &mut fired, &personas);
    // NCHAOS-1 창 회피: queue_event(create_task)가 배달 레코드를 커밋할 시간.
    std::thread::sleep(Duration::from_secs(1));
    backend.stop();

    // ── DOWN #1 ──
    hook_fire(jh, "feature/chaos-j2", &mk(5), false, &mut fired, &personas);
    hook_fire(mj, "main", &mk(6), false, &mut fired, &personas);
    down_fanout(hn, "branch_push", &mk(7), &mut fired, &personas);
    down_fanout(jw, "branch_push", &mk(8), &mut fired, &personas);
    backend.restart(); // 같은 DB — 기기·프로젝트·미배달 레코드 전부 생존.

    // ── UP #2 ──
    live_fanout(mj, "main_push", &mk(9), &mut fired, &personas);
    hook_fire(jh, "feature/chaos-j3", &mk(10), true, &mut fired, &personas);
    live_fanout(dy, "branch_push", &mk(11), &mut fired, &personas);
    std::thread::sleep(Duration::from_secs(1));
    backend.stop();

    // ── DOWN #2 ──
    hook_fire(mj, "main", &mk(12), false, &mut fired, &personas);
    down_fanout(sy, "branch_push", &mk(13), &mut fired, &personas);
    backend.restart();

    // ── UP #3 (최종) ──
    live_fanout(hn, "branch_push", &mk(14), &mut fired, &personas);
    hook_fire(jh, "feature/chaos-j4", &mk(15), true, &mut fired, &personas);
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(fired.len(), 15, "이벤트 15발이 전부 발사됐다");

    // ── 최종 up 후: 각 송신자의 스풀 재전송 (peer_poll_now 첫 단계와 동일) ──
    let expected_flush = [(mj, 2usize), (jh, 1), (dy, 0), (sy, 1), (hn, 1), (jw, 1)];
    for (i, expect) in expected_flush {
        let p = &personas[i];
        p.activate();
        let sent = rt
            .block_on(peer::flush_spooled_events(&url, &p.token))
            .unwrap();
        assert_eq!(
            sent, expect,
            "{} 의 스풀 재전송 건수가 다르다 (down 중 발사분)",
            p.name
        );
        assert!(
            !p.spool_path().exists(),
            "{} 의 스풀은 재전송 후 비워져야 한다",
            p.name
        );
    }

    // ── 전 수신자 drain ──
    let stores: Vec<Store> = personas.iter().map(|p| p.store()).collect();
    let mut inboxes: Vec<Vec<TeamEventRow>> = Vec::new();
    for (i, p) in personas.iter().enumerate() {
        let expect = fired.iter().filter(|(_, s)| *s != i).count();
        let got = poll_until(&rt, &url, &p.token, &stores[i], expect, 30);
        assert!(
            got.len() >= expect,
            "{} 는 자기가 보낸 것을 뺀 {expect}건을 받아야 하는데 {}건: 유실 발생",
            p.name,
            got.len()
        );
        inboxes.push(stores[i].list_team_events(1000, false).unwrap());
    }

    // ── 회계 불변식 + at-least-once 중복 집계 ──
    let mut pairs = 0usize; // (수신자, 이벤트) 커버리지
    let mut dup_rows = 0usize;
    for (marker, sender) in &fired {
        for (i, p) in personas.iter().enumerate() {
            let n = inboxes[i]
                .iter()
                .filter(|r| r.payload.contains(marker))
                .count();
            if i == *sender {
                assert_eq!(
                    n, 0,
                    "{} 는 자기 push({marker}) 알림을 받지 않는다 (delivery.py:33)",
                    p.name
                );
                continue;
            }
            assert!(
                n >= 1,
                "[데이터손실] {marker} (송신 {}) 가 {} 의 수신함에 없다 — \
                 push 가 끝내 모르는 사건이 됐다",
                personas[*sender].name,
                p.name
            );
            pairs += 1;
            dup_rows += n - 1;
        }
    }
    assert_eq!(pairs, 75, "커버리지 = 15 이벤트 × 수신자 5 = 75 쌍");
    let total_rows: usize = inboxes.iter().map(|v| v.len()).sum();
    println!(
        "[NCHAOS c1] at-least-once 통계: 수신함 총 {total_rows}행, \
         고유 (수신자,이벤트) {pairs}쌍, 중복 {dup_rows}행"
    );

    // 이벤트 종별 스팟체크: 관리자의 main push 는 main_push 로 승격돼 도착.
    let jw_inbox = &inboxes[jw];
    let kind_of = |marker: &str| -> String {
        jw_inbox
            .iter()
            .find(|r| r.payload.contains(marker))
            .map(|r| r.event_kind.clone())
            .unwrap_or_default()
    };
    assert_eq!(kind_of(&mk(1)), "branch_push");
    assert_eq!(kind_of(&mk(2)), "main_push", "hook 의 병합 브랜치 승격 (main.rs:132)");
    assert_eq!(kind_of(&mk(6)), "main_push", "스풀을 거쳐도 종별이 보존된다");

    // drain 후 재폴링은 빈 손 — 서버가 배달을 다시 주지 않는다.
    for (i, p) in personas.iter().enumerate() {
        let again = poll_drain(&rt, &url, &p.token, &stores[i]).unwrap();
        assert!(again.is_empty(), "{} 에게 중복 재배달이 있으면 안 된다", p.name);
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 2 — 스풀 순서·멱등성: 다운 동안 3발 → 스풀 파일에 발사 순서대로
// 쌓인다 → 재기동 후 flush 두 번 연속: 첫 번째가 3건 재전송, 두 번째는
// 완전한 no-op(스풀 파일 부재). 수신자는 정확히 3건 — created_at 순서는
// 보고만 하고 강제하지 않는다.
// ═════════════════════════════════════════════════════════════════════════════
#[test]
fn c2_spool_preserves_order_and_flush_is_idempotent() {
    let _g = serialize_tests();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut backend = Backend::start();
    let rig = Rig::new();
    let url = backend.url();

    let mut minji = Persona::register(&rt, &url, "민지");
    minji.activate();
    let project = rt
        .block_on(peer::create_project(&url, &minji.token, "스풀팀"))
        .unwrap();
    minji.attach_repo(rig.seed("민지", "minji@t.com"), "스풀 저장소", "main", &[&project.id]);

    let junho = Persona::register(&rt, &url, "준호");
    rt.block_on(peer::join_project(&url, &junho.token, &project.join_code))
        .unwrap();
    let jstore = junho.store();

    // ── 다운 → 실제 hook 바이너리로 3발 (전부 종료 0, 스풀에 보관) ──
    backend.stop();
    let markers = ["nchaos2evt1", "nchaos2evt2", "nchaos2evt3"];
    for (i, m) in markers.iter().enumerate() {
        let out = emit_push(&minji, &format!("feature/spool-{}", i + 1), m, &rig.url);
        assert_hook_fail_open(&out, m);
        assert!(stdout_of(&out).contains("posted to 0/1"), "{}", stdout_of(&out));
    }

    // 스풀 파일: 발사 순서 그대로 3줄 (append-only, peer.rs:221-233).
    let spool = minji.spool_path();
    let text = std::fs::read_to_string(&spool).expect("스풀 파일이 있어야 한다");
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 3, "3발 = 3줄");
    for (line, m) in lines.iter().zip(markers.iter()) {
        let ev: SpooledEvent = serde_json::from_str(line).expect("스풀 줄은 JSON");
        assert!(
            ev.payload.contains(m),
            "스풀은 발사 순서를 보존한다: {line} 에 {m} 이 없다"
        );
        assert_eq!(ev.project_id, project.id);
        assert_eq!(ev.event_kind, "branch_push");
    }

    // ── 재기동(같은 DB) → flush 두 번 연속 ──
    backend.restart();
    minji.activate();
    let sent1 = rt
        .block_on(peer::flush_spooled_events(&url, &minji.token))
        .unwrap();
    assert_eq!(sent1, 3, "첫 flush 가 3건 전부 재전송");
    assert!(!spool.exists(), "flush 후 스풀 파일은 지워진다 (peer.rs:279-281)");

    let sent2 = rt
        .block_on(peer::flush_spooled_events(&url, &minji.token))
        .unwrap();
    assert_eq!(sent2, 0, "두 번째 flush 는 no-op — 중복 재전송이 없다");
    assert!(!spool.exists(), "no-op flush 가 스풀 파일을 되살리면 안 된다");

    // ── 수신자: 정확히 3건, 그 이상은 없다 ──
    let got = poll_until(&rt, &url, &junho.token, &jstore, 3, 15);
    assert_eq!(got.len(), 3, "정확히 3건 — flush 멱등성의 수신자 측 증거");
    for m in &markers {
        assert_eq!(
            got.iter().filter(|r| r.payload.contains(m)).count(),
            1,
            "{m} 은 정확히 한 번"
        );
    }
    // 순서는 서버의 PushEvent.created_at 정렬(routes/events.py:104)을 따른다 —
    // flush 가 순차 전송하므로 보통 발사 순서지만, utcnow 해상도 탓에 강제하지
    // 않고 관측만 보고한다.
    let arrival: Vec<&str> = got
        .iter()
        .map(|r| *markers.iter().find(|m| r.payload.contains(*m)).unwrap())
        .collect();
    println!("[NCHAOS c2] 발사 순서 {markers:?} → 도착 순서(created_at) {arrival:?}");

    let again = poll_drain(&rt, &url, &junho.token, &jstore).unwrap();
    assert!(again.is_empty(), "재폴링에 중복이 없어야 한다");
    assert_eq!(jstore.list_team_events(50, false).unwrap().len(), 3);
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 3 — 서버 DB 초기화(재설치): 빈 DB 로 재기동하면 기기 토큰을 서버가
// 모른다 → poll 401. peer_poll_now 는 이제 여기서 **스스로 회복한다**
// (NCHAOS-3 고침, commands/peer.rs:258-277): 401 을 받으면 들고 있던 토큰으로
// 재등록하고(서버가 제시 토큰을 채택, devices.py 멱등) 새 device_id·
// backend_url 을 config.json 에 저장한 뒤 1회 재시도한다 — 401 이 영원히
// 반복되며 주기 폴링이 조용히 죽던 문제의 회귀 방지. 다만 프로젝트·멤버십은
// 서버에서 사라졌으므로 팀 재구축(프로젝트 재생성 + 전원 재합류 + 저장소
// 재링크)은 여전히 사람 몫이고, 그 과정에서 로컬 상태(수신함·설정·링크)는
// 훼손되지 않아야 한다. 스풀에 남은 옛 프로젝트 이벤트는 서버가 4xx 로
// 거부하는 순간 폐기된다(NCHAOS-4 고침, peer.rs:266-273) — 매 폴링마다
// 재시도되는 독약 스풀의 회귀 방지.
// ═════════════════════════════════════════════════════════════════════════════
#[test]
fn c3_server_db_wipe_auto_reregisters_but_team_rebuild_stays_manual() {
    let _g = serialize_tests();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut backend = Backend::start();
    let rig = Rig::new();
    let url = backend.url();

    // ── 초기화 전: 정상 팀 + 배달 1건 (민지 수신함에 남는다) ──
    let mut minji = Persona::register(&rt, &url, "민지");
    minji.activate();
    let project = rt
        .block_on(peer::create_project(&url, &minji.token, "재설치팀"))
        .unwrap();
    minji.attach_repo(rig.seed("민지", "minji@t.com"), "재설치 저장소", "main", &[&project.id]);
    let old_device_id = minji.device_id.clone();

    let junho = Persona::register(&rt, &url, "준호");
    rt.block_on(peer::join_project(&url, &junho.token, &project.join_code))
        .unwrap();
    rt.block_on(peer::fanout_event(
        &url,
        &junho.token,
        &project.id,
        "branch_push",
        "재설치 저장소",
        &chaos_payload("branch_push", "준호", "nchaos3-before"),
    ))
    .unwrap();
    let mstore = minji.store();
    let got = poll_until(&rt, &url, &minji.token, &mstore, 1, 10);
    assert_eq!(got.len(), 1, "초기화 전 파이프라인 정상");

    // ── 서버 재설치: 완전히 빈 DB 로 같은 포트에 재기동 ──
    backend.restart_fresh_db();

    // 서버는 이제 이 토큰을 모른다 → 맨몸 poll 은 401 (deps.py:42-44).
    assert_eq!(poll_status(&rt, &url, &minji.token), 401);

    // NCHAOS-3 회귀 방지: peer_poll_now 는 401 을 받으면 들고 있던 토큰으로
    // 재등록하고(서버가 제시 토큰을 채택 — 멱등) 새 device_id 를 config.json
    // 에 저장한 뒤 1회 재시도해 성공한다 (commands/peer.rs:258-277).
    // 옛 device_id 가 남아 401 이 영원히 반복되며 주기 폴링이 조용히 죽던
    // 문제가 다시 생기면 여기서 깨진다.
    minji.activate();
    rt.block_on(git_companion::commands::peer::peer_poll_now())
        .expect("NCHAOS-3 새 동작: 재설치 후 peer_poll_now 는 자동 재등록으로 회복한다");
    minji.activate();
    let cfg = config_store::load().unwrap();
    assert!(!cfg.peer.device_id.is_empty());
    assert_ne!(
        cfg.peer.device_id, old_device_id,
        "빈 DB 라 기기 id 는 새로 발급되고, 새 id 가 config.json 에 저장된다"
    );
    assert_eq!(cfg.peer.backend_url, url, "backend_url 도 함께 저장된다");
    assert_eq!(poll_status(&rt, &url, &minji.token), 200, "자동 재등록 즉시 폴링 회복");

    // 자동 회복이 로컬 상태를 망가뜨리지 않았다: 수신함·저장소·링크 전부 무사.
    assert_eq!(
        mstore.list_team_events(50, false).unwrap().len(),
        1,
        "초기화 전에 받은 알림은 로컬에 남는다"
    );
    assert_eq!(cfg.repositories.len(), 1, "config.json 의 저장소 목록이 온전하다");
    let rp = RepoProjects::load().unwrap();
    assert_eq!(
        rp.projects_for(&minji.repo_path().display().to_string()),
        vec![project.id.clone()],
        "repo_projects.json 이 온전하다 (옛 프로젝트 id 로 남는다)"
    );

    // ── 이 창에서 push 가 일어나면: hook 은 fail-open + 스풀 보관 ──
    // (기기는 재등록됐지만 프로젝트 멤버십이 사라져 403 → 전송 0건.)
    let out = emit_push(&minji, "feature/wiped", "nchaos3-wiped", &rig.url);
    assert_hook_fail_open(&out, "wiped-window");
    assert!(
        stdout_of(&out).contains("posted to 0/1"),
        "옛 프로젝트는 서버에 없으므로 전송 0건: {}",
        stdout_of(&out)
    );
    assert!(minji.spool_path().exists(), "실패분은 스풀에 보관된다");

    // ── 프로젝트/멤버십은 사라졌다 — 실패는 명확해야 하고 상태를 안 망친다 ──
    let e = rt
        .block_on(peer::fanout_event(
            &url,
            &minji.token,
            &project.id,
            "branch_push",
            "재설치 저장소",
            "{}",
        ))
        .expect_err("사라진 프로젝트로의 fanout 은 실패해야 한다");
    assert!(
        format!("{e:?}").contains("403"),
        "멤버십 없음 → 403 (routes/events.py:62): {e:?}"
    );
    let e = rt
        .block_on(peer::join_project(&url, &minji.token, &project.join_code))
        .expect_err("옛 join code 는 더 이상 유효하지 않다");
    assert!(format!("{e:?}").contains("404"), "Invalid join code → 404: {e:?}");
    assert!(
        rt.block_on(peer::list_projects(&url, &minji.token))
            .unwrap()
            .is_empty(),
        "서버 관점의 프로젝트 목록은 비었다"
    );

    // NCHAOS-4 회귀 방지: 서버가 4xx(여기서는 403 — 사라진 프로젝트)로 거부한
    // 스풀 줄은 영구 실패라 폐기된다 (peer.rs:266-273; 네트워크 오류·5xx 만
    // 보존). 전송 0건 + 빈 스풀 파일 삭제 — 같은 줄이 매 폴링마다 재시도되는
    // 독약 스풀이 다시 생기면 여기서 깨진다.
    minji.activate();
    let sent = rt
        .block_on(peer::flush_spooled_events(&url, &minji.token))
        .unwrap();
    assert_eq!(sent, 0, "옛 프로젝트 id → 403 → 재전송 0건");
    assert!(
        !minji.spool_path().exists(),
        "403 으로 거부된 줄은 폐기되고, 빈 스풀 파일은 지워진다"
    );
    // 다음 폴링은 그 줄을 다시 시도하지 않는다 — flush 는 완전한 no-op.
    let sent = rt
        .block_on(peer::flush_spooled_events(&url, &minji.token))
        .unwrap();
    assert_eq!(sent, 0, "폐기된 줄의 재시도는 없다");
    assert!(!minji.spool_path().exists());

    // ── 팀이 다시 해야 하는 일(여전히 사람 몫): ① 프로젝트 재생성 ② 나머지
    //    기기 재등록+재합류(민지는 위에서 자동 재등록됨) ③ 저장소 재링크.
    //    그 후 파이프라인이 완전히 회복되는지 확인. ──
    let project2 = rt
        .block_on(peer::create_project(&url, &minji.token, "재설치팀-2기"))
        .unwrap();
    rt.block_on(peer::register_device(&url, &junho.token, "준호"))
        .unwrap();
    rt.block_on(peer::join_project(&url, &junho.token, &project2.join_code))
        .unwrap();
    minji.activate();
    let mut rp = RepoProjects::load().unwrap();
    rp.link(&minji.repo_path().display().to_string(), &project2.id);
    rp.save().unwrap();

    // 재링크 후 hook: 새 프로젝트 1건 전송 + 옛 링크 1건은 발신 시점에 또
    // 스풀(정직한 1/2) — hook 은 실패 종류를 모르고 일단 보관한다(main.rs).
    let out = emit_push(&minji, "feature/rebuilt", "nchaos3-after", &rig.url);
    assert_hook_fail_open(&out, "rebuilt");
    assert!(
        stdout_of(&out).contains("posted to 1/2"),
        "옛 프로젝트 링크가 남아 1/2 — 재링크만으로는 낡은 링크가 정리되지 않는다: {}",
        stdout_of(&out)
    );
    assert!(minji.spool_path().exists(), "403 짜리 줄이 발신 시점에 다시 보관된다");
    // …그리고 다음 flush 가 그 줄을 4xx 폐기로 정리한다 (NCHAOS-4 의 마무리).
    let sent = rt
        .block_on(peer::flush_spooled_events(&url, &minji.token))
        .unwrap();
    assert_eq!(sent, 0);
    assert!(!minji.spool_path().exists(), "독약 줄은 다음 폴링에서 폐기된다");

    let jstore = junho.store();
    let got = poll_until(&rt, &url, &junho.token, &jstore, 1, 10);
    assert_eq!(got.len(), 1, "재구축 후 파이프라인 회복");
    assert!(got[0].payload.contains("nchaos3-after"));
    assert_eq!(got[0].project_id, project2.id);
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 4 — 수신자 크래시 창의 인접 보장: 3건 밀린 수신자가 1건만 받은 뒤
// 서버가 죽는다(= peer_poll_now 루프가 도중에 에러로 끊기는 상황). 이미
// 넘겨받아 저장한 1건은 그대로, 아직 넘겨받지 않은 2건은 서버에 미배달로
// 남아 재기동 후 도착한다 — "넘겨받지 않은 것은 절대 잃지 않는다".
//
// NCHAOS-2 (테스트 불가능한 창의 문서화): 진짜 위험 창은 "서버가 poll 응답을
// 만들며 delivered_at 을 찍은 뒤(routes/events.py:110-113, 137-139) 클라이언트가
// 수신함 INSERT(commands/peer.rs:296) 를 마치기 전"이다. 이 사이에 수신 앱
// 프로세스가 죽으면 서버는 배달 완료로 믿고 다시 주지 않으므로 그 알림은
// 유실된다. acked_at 필드와 /events/{id}/ack (routes/events.py:144-184) 가
// 있지만 클라이언트가 부르지 않는다 — 최소 수정은 "poll 은 표시만, ack 에서
// delivered_at 확정" 또는 클라이언트가 INSERT 후 ack 를 보내고 서버가
// ack 없는 배달을 재서빙하는 것. in-process 테스트로는 INSERT 직전 킬을
// 재현할 수 없어 여기 문서로만 못박는다.
// ═════════════════════════════════════════════════════════════════════════════
#[test]
fn c4_server_death_mid_drain_loses_nothing_not_yet_handed_over() {
    let _g = serialize_tests();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut backend = Backend::start();
    let url = backend.url();

    let minji = Persona::register(&rt, &url, "민지");
    let project = rt
        .block_on(peer::create_project(&url, &minji.token, "중단팀"))
        .unwrap();
    let doyun = Persona::register(&rt, &url, "도윤");
    rt.block_on(peer::join_project(&url, &doyun.token, &project.join_code))
        .unwrap();

    // 3건 백로그 (도윤은 아직 한 번도 폴링하지 않았다).
    for n in 1..=3 {
        rt.block_on(peer::fanout_event(
            &url,
            &minji.token,
            &project.id,
            "branch_push",
            "중단 저장소",
            &chaos_payload("branch_push", "민지", &format!("nchaos4evt{n}")),
        ))
        .unwrap();
        std::thread::sleep(Duration::from_millis(40)); // created_at 순서
    }
    std::thread::sleep(Duration::from_millis(700)); // 배달 레코드 커밋 대기

    // 1건만 넘겨받아 저장한다 (peer_poll_now 루프의 첫 반복과 동일:
    // 응답 수신 → INSERT → 다음 poll).
    let dstore = doyun.store();
    let first = try_poll_once(&rt, &url, &doyun.token)
        .unwrap()
        .expect("백로그 첫 건");
    let first_row = store_polled(&dstore, &first);
    assert!(first_row.payload.contains("nchaos4evt1"), "created_at 순서상 1번");

    // 두 폴링 사이에서 서버 사망 — 루프의 다음 반복은 에러로 끊긴다.
    backend.stop();
    let e = try_poll_once(&rt, &url, &doyun.token).expect_err("죽은 서버로의 poll 은 에러");
    assert!(e.contains("poll send failed"), "연결 실패 계열이어야 한다: {e}");
    // 에러로 끊겨도 이미 넘겨받은 1건은 로컬에 온전하다.
    assert_eq!(dstore.list_team_events(50, false).unwrap().len(), 1);

    // 재기동(같은 DB): 아직 delivered_at 이 없는 2건이 그대로 남아 도착한다.
    backend.restart();
    let rest = poll_until(&rt, &url, &doyun.token, &dstore, 2, 10);
    assert_eq!(rest.len(), 2, "넘겨받지 못했던 2건이 전부 도착한다");

    let all = dstore.list_team_events(50, false).unwrap();
    assert_eq!(all.len(), 3, "총 3건 — 유실도 중복도 없다");
    for n in 1..=3 {
        let m = format!("nchaos4evt{n}");
        assert_eq!(
            all.iter().filter(|r| r.payload.contains(&m)).count(),
            1,
            "{m} 은 정확히 한 번 — 이미 배달된 1번이 다시 오지 않는다"
        );
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 5 — 물량 아래의 수신함 위생: 한 수신함에 이벤트 65건.
// list_team_events(50) 의 상한, count_unread 의 정확성, 부분 읽음 후 배지,
// mark_all_team_read 의 반환값과 행 보존을 검증.
// ═════════════════════════════════════════════════════════════════════════════
#[test]
fn c5_inbox_hygiene_under_volume() {
    let _g = serialize_tests();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let backend = Backend::start();
    let url = backend.url();

    let minji = Persona::register(&rt, &url, "민지");
    let project = rt
        .block_on(peer::create_project(&url, &minji.token, "물량팀"))
        .unwrap();
    let junho = Persona::register(&rt, &url, "준호");
    rt.block_on(peer::join_project(&url, &junho.token, &project.join_code))
        .unwrap();

    const N: usize = 65;
    for n in 1..=N {
        rt.block_on(peer::fanout_event(
            &url,
            &minji.token,
            &project.id,
            "branch_push",
            "물량 저장소",
            &chaos_payload("branch_push", "민지", &format!("nchaos5evt{n:02}")),
        ))
        .unwrap();
    }

    let jstore = junho.store();
    let got = poll_until(&rt, &url, &junho.token, &jstore, N, 40);
    assert_eq!(got.len(), N, "{N}건 전부 도착");

    // 상한: 50 요청 → 50 (수신함 UI 의 기본 페이지).
    assert_eq!(jstore.list_team_events(50, false).unwrap().len(), 50);
    // 미읽음 배지는 상한과 무관하게 정확하다.
    assert_eq!(jstore.count_unread_team_events().unwrap() as usize, N);

    // 부분 읽음: 최신 7건을 읽는다 → 배지 58.
    let listed = jstore.list_team_events(50, false).unwrap();
    for r in listed.iter().take(7) {
        jstore.mark_team_read(&r.id).unwrap();
    }
    assert_eq!(jstore.count_unread_team_events().unwrap() as usize, N - 7);
    assert_eq!(
        jstore.list_team_events(200, true).unwrap().len(),
        N - 7,
        "unread_only 목록도 정확"
    );
    assert_eq!(
        jstore.list_team_events(50, true).unwrap().len(),
        50,
        "unread_only 도 상한을 지킨다"
    );

    // 모두 읽음: 남은 미읽음 수를 돌려주고 배지가 0 이 된다. 행은 남는다.
    assert_eq!(jstore.mark_all_team_read().unwrap() as usize, N - 7);
    assert_eq!(jstore.count_unread_team_events().unwrap(), 0);
    assert_eq!(jstore.list_team_events(200, false).unwrap().len(), N);

    // 전부 읽은 뒤 새 이벤트 1건 → 배지 1 (부분 읽음 뒤의 증분이 정확하다).
    rt.block_on(peer::fanout_event(
        &url,
        &minji.token,
        &project.id,
        "branch_push",
        "물량 저장소",
        &chaos_payload("branch_push", "민지", "nchaos5evt66"),
    ))
    .unwrap();
    let more = poll_until(&rt, &url, &junho.token, &jstore, 1, 10);
    assert_eq!(more.len(), 1);
    assert_eq!(jstore.count_unread_team_events().unwrap(), 1);
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 6 — payload 강건성: 미니파이/키 재정렬/미지의 여분 필드/아예
// JSON 이 아닌 payload 도 파이프라인(서버 String 컬럼 → poll → 관용적 매핑 →
// SQLite)을 바이트 그대로 통과한다. 한글·이모지 마커 바이트 일치 포함.
// ═════════════════════════════════════════════════════════════════════════════
#[test]
fn c6_hostile_payloads_survive_byte_exact_with_lenient_mapping() {
    let _g = serialize_tests();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let backend = Backend::start();
    let url = backend.url();

    let minji = Persona::register(&rt, &url, "민지");
    let project = rt
        .block_on(peer::create_project(&url, &minji.token, "강건팀"))
        .unwrap();
    let junho = Persona::register(&rt, &url, "준호");
    rt.block_on(peer::join_project(&url, &junho.token, &project.join_code))
        .unwrap();

    // ① 미니파이 + 한글/이모지, ② 키 재정렬 + 미지의 필드(중첩 포함),
    // ③ JSON 이 아닌 순수 문자열 — payload 는 어디서도 파싱을 강요받지 않는다
    // (서버는 String(8192) 컬럼, 클라이언트 매핑은 event_kind 필드만 본다).
    let payloads: [(&str, &str, &str); 3] = [
        (
            "branch_push",
            r#"{"kind":"branch_push","data":{"branch":"기능/알림-개편","message":"환영합니다 🎉","marker":"NC6-α-한글🚀"}}"#,
            "NC6-α-한글🚀",
        ),
        (
            "main_push",
            r#"{"미래필드":123,"data":{"branch":"main","unknown_nested":{"a":[1,2,3],"b":null}},"kind":"main_push","marker":"NC6-β-이모지🧨","extra":"서버는 몰라도 됨"}"#,
            "NC6-β-이모지🧨",
        ),
        (
            "branch_push",
            "NC6-γ-그냥문자열 — not json at all 🤷 <html>&잡음</html>",
            "NC6-γ-그냥문자열",
        ),
    ];

    for (kind, payload, _) in &payloads {
        rt.block_on(peer::fanout_event(
            &url,
            &minji.token,
            &project.id,
            kind,
            "강건 저장소 ★",
            payload,
        ))
        .unwrap();
    }

    let jstore = junho.store();
    let got = poll_until(&rt, &url, &junho.token, &jstore, 3, 10);
    assert_eq!(got.len(), 3, "세 payload 전부 저장된다 — 관용적 매핑에 파싱 실패가 없다");

    for (kind, payload, marker) in &payloads {
        let row = got
            .iter()
            .find(|r| r.payload.contains(marker))
            .unwrap_or_else(|| panic!("{marker} 가 수신함에 없다"));
        assert_eq!(
            row.payload, *payload,
            "{marker}: hook→HTTP→서버 DB→poll→SQLite 왕복 후에도 바이트 동일"
        );
        assert_eq!(
            row.event_kind, *kind,
            "{marker}: 종별은 payload 파싱이 아니라 이벤트 필드에서 온다"
        );
        assert_eq!(row.repo_name, "강건 저장소 ★");
        assert_eq!(row.sender_device_name, "민지", "poll 응답의 이름 매핑");
    }

    // SQLite 재조회 왕복도 무손실.
    let listed = jstore.list_team_events(10, false).unwrap();
    for (_, payload, marker) in &payloads {
        let row = listed.iter().find(|r| r.payload.contains(marker)).unwrap();
        assert_eq!(row.payload, *payload, "{marker}: DB 재조회 후에도 바이트 동일");
    }
    assert_eq!(jstore.count_unread_team_events().unwrap(), 3);
}
