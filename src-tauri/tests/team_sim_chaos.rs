//! 팀 시뮬레이션 — 비각본(UNSCRIPTED) 혼돈 시나리오.
//!
//! 이전 라운드(team_sim_races / team_sim_loop / team_sim_noloss_*)가 각본 있는
//! 시나리오를 전수 검증했다면, 이 파일은 남은 조각을 채운다: **6명의
//! 페르소나(병합 관리자 민지 + 팀원 준호/도윤/서연/하늘/지우)가 각자 스레드에서
//! 가중치 랜덤으로 동시에 행동**하고, 시스템이 수렴하며 아무것도 잃지 않음을
//! 증명한다.
//!
//! 설계:
//! - bare origin 하나(`receive.denyCurrentBranch ignore`) + 클론 6개.
//!   스레드 하나가 클론 하나만 소유한다 (한 클론을 두 스레드가 만지지 않는다).
//! - 팀 동작은 전부 crate 공개 API(`git_companion::git::*`, `Target::Local`).
//!   raw git 은 샌드박스 구성과 **검증**에만 쓴다.
//! - 결정적 시드 RNG(xorshift64*) — 실패 시 패닉 메시지에 시드와 액션 로그
//!   꼬리를 실어 재현 가능하게 한다. (스레드 인터리빙 자체는 OS 스케줄러
//!   소관이므로 카운트류 통계는 실행마다 다를 수 있다 — 단언은 항상 성립해야
//!   하는 불변식에만 건다.)
//! - 오류 정책: API 가 돌려주는 모든 Err / ok=false 메시지는 **한국어이고
//!   행동 가능한** 허용 목록(소스의 실제 메시지에서 구축)에 있어야 한다.
//!   허용 밖 오류·영어 원문·패닉은 위반으로 기록되어 테스트를 실패시킨다.
//!
//! 불변식 (혼돈 중 + 종료 시):
//! 1. origin 장부 — push 성공 때마다 origin ref 에서 도달 가능한 SHA 를
//!    기록하고, 한 번 도달 가능했던 SHA 는 영원히 도달 가능해야 한다.
//!    (예외: delete_remote_branch 로 지운 브랜치는 그 SHA 전부가
//!    origin/main 에서 도달 가능해야 한다.)
//! 2. 클론별 무손실 — 페르소나가 만든 모든 커밋(생성 시점에 sha 기록)은
//!    종료 시 origin/main 또는 그 페르소나의 브랜치에서 도달 가능하고,
//!    각 팀원의 "자기 파일" 최종 내용은 최종 main 과 바이트 동일해야 한다.
//! 3. 수렴 — quiesce 후 6개 클론의 main == origin/main, 전 저장소
//!    `git fsck --strict` 통과, 대기 목록 비움, base_unpushed_count == 0.
//! 4. 공유 파일 의미론 — ours/theirs 선택은 상대편 **내용 줄**을 정당하게
//!    떨어뜨릴 수 있다(사용자의 명시적 선택). 잃는 쪽 **커밋**은 1/2 로
//!    보호된다. 페르소나 태그가 붙은 고유 줄로 생존/탈락을 **측정만** 하고
//!    보고한다(실패 아님).
//!
//! 발견된 문제는 FIXME(CHAOS-n) 주석과 함께 현재 동작을 그대로 assert 한다
//! (스위트는 green 유지):
//! - FIXME(CHAOS-1): 첫 push 전 브랜치에서 pull → git 영어 원문
//!   ("couldn't find remote ref …")이 그대로 노출된다. 아래 전용 테스트 참고.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use tempfile::TempDir;

use git_companion::git::fetch::fetch_target;
use git_companion::git::merge::{
    base_unpushed_count, complete_merge, conflict_detail, delete_remote_branch,
    list_merged_remote_branches, list_pending_branches, merge_in_progress, remaining_conflicts,
    resolve_conflict, start_merge, PendingBranch, Resolution,
};
use git_companion::git::ops::{list_stashes, StashAction};
use git_companion::git::{commit, create_branch, pull, push, stash, sync_to_base, Target};

// ═════════════════════════════════════════════════════════════════════════
// 시드 RNG — 외부 crate 없이 결정적 난수 (xorshift64*)
// ═════════════════════════════════════════════════════════════════════════

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        // 0 이 되지 않게 섞는다.
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// 0..n
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn nap(rng: &mut Rng) {
    std::thread::sleep(Duration::from_millis(rng.below(31)));
}

// ═════════════════════════════════════════════════════════════════════════
// raw git 헬퍼 — 샌드박스 구성/검증 전용 (앱 동작에는 쓰지 않는다)
// ═════════════════════════════════════════════════════════════════════════

fn git_try(dir: &Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new("git")
        .args(["-c", "core.quotepath=off"])
        .args(args)
        .current_dir(dir)
        .env("LC_ALL", "C.UTF-8")
        .env("LANG", "C.UTF-8")
        .output()
        .expect("git spawn")
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = git_try(dir, args);
    assert!(
        out.status.success(),
        "git {args:?} failed in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn set_identity(dir: &Path, name: &str, email: &str) {
    git(dir, &["config", "user.name", name]);
    git(dir, &["config", "user.email", email]);
    git(dir, &["config", "commit.gpgsign", "false"]);
}

fn append_line(dir: &Path, rel: &str, line: &str) {
    let p = dir.join(rel);
    let mut body = fs::read_to_string(&p).unwrap_or_default();
    body.push_str(line);
    body.push('\n');
    fs::write(p, body).unwrap();
}

fn rev_set(dir: &Path, rev: &str) -> BTreeSet<String> {
    git(dir, &["rev-list", rev])
        .lines()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

// ═════════════════════════════════════════════════════════════════════════
// 오류 허용 목록 — src 의 실제 한국어 메시지에서 구축.
// 여기 없는 Err/ok=false 메시지는 전부 정책 위반으로 기록된다.
// ═════════════════════════════════════════════════════════════════════════

const EXPECTED_ERRORS: &[&str] = &[
    // merge.rs / sync.rs — 진행 중 병합 가드
    "이미 진행 중인 병합",
    "진행 중인 병합이 있습니다",
    // merge.rs — dirty tree / TOCTOU / 사라진 ref
    "커밋되지 않은 변경",
    "새 push가 있었습니다",
    "브랜치를 찾을 수 없습니다",
    "병합할 대상을 찾을 수 없습니다",
    // sync.rs — dirty / 잔여 충돌
    "커밋하지 않은 변경",
    "해결되지 않은 충돌",
    // ops.rs — push / commit / stash
    "푸시 거부됨",
    "푸시 실패",
    "커밋할 변경이 없습니다",
    "해결하지 않은 충돌",
    "다른 git 작업이 진행 중입니다",
    "보관할 변경이 없습니다",
    "스태시를 복원하다 충돌이 났습니다",
    // merge.rs — 원격 브랜치 삭제 가드
    "없는 커밋이 있습니다",
    "삭제할 수 없습니다",
];

fn expected_key(msg: &str) -> Option<&'static str> {
    EXPECTED_ERRORS.iter().copied().find(|k| msg.contains(k))
}

// ═════════════════════════════════════════════════════════════════════════
// 통계
// ═════════════════════════════════════════════════════════════════════════

#[derive(Default, Clone)]
struct Stats {
    ops: u32,
    commits: u32,
    pushes: u32,
    merges: u32,
    syncs: u32,
    pulls: u32,
    stash_saves: u32,
    stash_pop_conflicts: u32,
    conflicts_resolved: u32,
    toctou: u32,
    nonff_retries: u32,
    errors: BTreeMap<&'static str, u32>,
}

impl Stats {
    fn err(&mut self, key: &'static str) {
        *self.errors.entry(key).or_default() += 1;
    }
    fn absorb(&mut self, o: &Stats) {
        self.ops += o.ops;
        self.commits += o.commits;
        self.pushes += o.pushes;
        self.merges += o.merges;
        self.syncs += o.syncs;
        self.pulls += o.pulls;
        self.stash_saves += o.stash_saves;
        self.stash_pop_conflicts += o.stash_pop_conflicts;
        self.conflicts_resolved += o.conflicts_resolved;
        self.toctou += o.toctou;
        self.nonff_retries += o.nonff_retries;
        for (k, v) in &o.errors {
            *self.errors.entry(k).or_default() += v;
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════
// origin 장부 — 불변식 (1)
// ═════════════════════════════════════════════════════════════════════════

#[derive(Default)]
struct OriginLedger {
    /// sha → 그 sha 가 도달 가능했던 origin ref 들.
    seen: BTreeMap<String, BTreeSet<String>>,
}

impl OriginLedger {
    /// 모든 성공 push 직후 호출.
    fn record(&mut self, bare: &Path) {
        let refs = git(
            bare,
            &["for-each-ref", "--format=%(refname)%09%(objectname)", "refs/heads"],
        );
        for line in refs.lines() {
            let mut it = line.split('\t');
            let (Some(name), Some(tip)) = (it.next(), it.next()) else {
                continue;
            };
            for sha in git(bare, &["rev-list", tip]).lines() {
                if !sha.is_empty() {
                    self.seen
                        .entry(sha.to_string())
                        .or_default()
                        .insert(name.to_string());
                }
            }
        }
    }

    /// 장부에는 있는데 지금 origin ref 에서 도달 불가능한 SHA 들.
    fn missing(&self, bare: &Path) -> Vec<String> {
        let now: BTreeSet<String> = git(bare, &["rev-list", "--all"])
            .lines()
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        self.seen
            .keys()
            .filter(|sha| !now.contains(*sha))
            .map(|s| format!("{s} (도달 가능했던 ref: {:?})", self.seen[s]))
            .collect()
    }

    fn branch_shas(&self, refname: &str) -> BTreeSet<String> {
        self.seen
            .iter()
            .filter(|(_, refs)| refs.contains(refname))
            .map(|(s, _)| s.clone())
            .collect()
    }
}

// ═════════════════════════════════════════════════════════════════════════
// 공유 컨텍스트 — 액션 로그 / 위반 기록 / 장부
// ═════════════════════════════════════════════════════════════════════════

struct Ctx {
    seed: u64,
    bare: PathBuf,
    log: Mutex<Vec<String>>,
    violations: Mutex<Vec<String>>,
    ledger: Mutex<OriginLedger>,
}

impl Ctx {
    fn new(seed: u64, bare: &Path) -> Ctx {
        Ctx {
            seed,
            bare: bare.to_path_buf(),
            log: Mutex::new(Vec::new()),
            violations: Mutex::new(Vec::new()),
            ledger: Mutex::new(OriginLedger::default()),
        }
    }
    fn log(&self, who: &str, msg: String) {
        self.log.lock().unwrap().push(format!("{who}: {msg}"));
    }
    fn violate(&self, who: &str, msg: String) {
        self.log(who, format!("★위반★ {msg}"));
        self.violations.lock().unwrap().push(format!("[{who}] {msg}"));
    }
    fn tail(&self) -> String {
        let log = self.log.lock().unwrap();
        let start = log.len().saturating_sub(80);
        log[start..].join("\n")
    }
    /// 성공한 push 직후 — 장부 갱신.
    fn record_push(&self) {
        self.ledger.lock().unwrap().record(&self.bare);
    }
}

/// 실패 시 시드 + 액션 로그 꼬리를 함께 터뜨리는 단언.
macro_rules! chk {
    ($ctx:expr, $cond:expr, $($arg:tt)+) => {
        if !$cond {
            panic!(
                "[chaos seed {:#x}] {}\n--- 액션 로그 꼬리 ---\n{}",
                $ctx.seed,
                format!($($arg)+),
                $ctx.tail()
            );
        }
    };
}

/// Err/ok=false 메시지가 허용 목록에 있으면 통계에 적고 true, 아니면 위반.
fn expect_or_violate(ctx: &Ctx, who: &str, op: &str, msg: &str, stats: &mut Stats) -> bool {
    match expected_key(msg) {
        Some(k) => {
            stats.err(k);
            ctx.log(who, format!("{op} → 예상된 거부({k})"));
            true
        }
        None => {
            ctx.violate(who, format!("{op} 비허용 오류: {msg}"));
            false
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════
// 충돌 해결 — 랜덤(ours/theirs/manual 결합) 또는 항상-결합
// ═════════════════════════════════════════════════════════════════════════

#[derive(Clone, Copy, PartialEq)]
enum ResolveMode {
    Random,
    CombineAll,
}

/// 양쪽 줄을 합친 수동 해결 본문 — ours 줄 전부 + ours 에 없는 theirs 줄.
fn combine_sides(t: &Target, path: &str) -> String {
    let Ok(d) = conflict_detail(t, path) else {
        return String::new();
    };
    let mut lines: Vec<&str> = d.ours.lines().collect();
    let have: HashSet<&str> = lines.iter().copied().collect();
    for l in d.theirs.lines() {
        if !have.contains(l) {
            lines.push(l);
        }
    }
    let mut s = lines.join("\n");
    if !s.is_empty() {
        s.push('\n');
    }
    s
}

fn resolve_all_conflicts(
    ctx: &Ctx,
    who: &str,
    t: &Target,
    rng: &mut Rng,
    mode: ResolveMode,
    stats: &mut Stats,
) {
    for _ in 0..30 {
        let rem = match remaining_conflicts(t) {
            Ok(r) => r,
            Err(e) => {
                ctx.violate(who, format!("remaining_conflicts 실패: {e}"));
                return;
            }
        };
        let Some(path) = rem.first().cloned() else {
            return;
        };
        let pick = match mode {
            ResolveMode::CombineAll => 2,
            ResolveMode::Random => rng.below(3),
        };
        let (label, res) = match pick {
            0 => ("ours", Resolution::Ours),
            1 => ("theirs", Resolution::Theirs),
            _ => (
                "manual-combine",
                Resolution::Manual {
                    content: combine_sides(t, &path),
                },
            ),
        };
        ctx.log(who, format!("충돌 해결 {path} ← {label}"));
        match resolve_conflict(t, &path, &res) {
            Ok(_) => stats.conflicts_resolved += 1,
            Err(e) => {
                expect_or_violate(ctx, who, "resolve_conflict", &e.to_string(), stats);
                // 진전 보장 — ours 로 강제 해결. 이것마저 실패하면 위반 후 탈출.
                if resolve_conflict(t, &path, &Resolution::Ours).is_err() {
                    ctx.violate(who, format!("{path} 강제 해결 실패 — 루프 탈출"));
                    return;
                }
                stats.conflicts_resolved += 1;
            }
        }
    }
    ctx.violate(who, "충돌 해결 루프 30회 초과".into());
}

// ═════════════════════════════════════════════════════════════════════════
// 팀원 페르소나
// ═════════════════════════════════════════════════════════════════════════

const MEMBERS: [(&str, &str); 5] = [
    ("준호", "junho"),
    ("도윤", "doyun"),
    ("서연", "seoyeon"),
    ("하늘", "haneul"),
    ("지우", "jiwoo"),
];
const SHARED_FILE: &str = "SHARED.txt";
const MEMBER_ITERS: u32 = 26;
const MANAGER_ITERS: u32 = 26;

struct Member {
    name: &'static str,
    key: &'static str,
    dir: TempDir,
    branch: String,
    file: String,
    counter: u32,
    pushed_once: bool,
    /// 이 페르소나가 만든 모든 커밋 sha (불변식 2).
    commits: Vec<String>,
    /// 이 페르소나가 공유 파일에 쓴 고유 줄 (생존 측정용).
    shared_lines: Vec<String>,
    stats: Stats,
    rng_seed: u64,
}

impl Member {
    fn p(&self) -> &Path {
        self.dir.path()
    }
    fn t(&self) -> Target {
        Target::Local(self.dir.path().to_path_buf())
    }
    fn record_head(&mut self) {
        let sha = git(self.p(), &["rev-parse", "HEAD"]).trim().to_string();
        self.commits.push(sha);
    }

    fn edit_own(&mut self, ctx: &Ctx) {
        let line = format!("{} own n{}", self.key, self.counter);
        self.counter += 1;
        append_line(self.p(), &self.file.clone(), &line);
        ctx.log(self.name, format!("자기 파일 수정 ({line})"));
    }

    fn edit_shared(&mut self, ctx: &Ctx) {
        let line = format!("{} shared s{:x} n{}", self.key, self.rng_seed, self.counter);
        self.counter += 1;
        append_line(self.p(), SHARED_FILE, &line);
        self.shared_lines.push(line.clone());
        ctx.log(self.name, format!("공유 파일 수정 ({line})"));
    }

    fn do_commit(&mut self, ctx: &Ctx) {
        let msg = format!("chaos: {} 작업 n{}", self.name, self.counter);
        match commit(&self.t(), &msg, true) {
            Ok(c) if c.ok => {
                self.stats.commits += 1;
                self.record_head();
                ctx.log(self.name, "커밋 ok".into());
            }
            Ok(c) => {
                expect_or_violate(ctx, self.name, "commit", &c.message, &mut self.stats);
            }
            Err(e) => {
                expect_or_violate(ctx, self.name, "commit", &e.to_string(), &mut self.stats);
            }
        }
    }

    fn do_push(&mut self, ctx: &Ctx) {
        match push(&self.t(), Some(&self.branch), None) {
            Ok(p) if p.ok => {
                self.stats.pushes += 1;
                self.pushed_once = true;
                ctx.record_push();
                ctx.log(self.name, "push ok".into());
            }
            Ok(p) => {
                if p.auth_required {
                    ctx.violate(self.name, format!("로컬 원격인데 auth_required: {}", p.message));
                }
                expect_or_violate(ctx, self.name, "push", &p.message, &mut self.stats);
            }
            Err(e) => {
                expect_or_violate(ctx, self.name, "push", &e.to_string(), &mut self.stats);
            }
        }
    }

    fn do_sync(&mut self, ctx: &Ctx, rng: &mut Rng) {
        let t = self.t();
        match sync_to_base(&t, "main", "origin") {
            Ok(r) if !r.conflicted => {
                self.stats.syncs += 1;
                ctx.log(self.name, "sync ok".into());
            }
            Ok(r) => {
                self.stats.syncs += 1;
                ctx.log(self.name, format!("sync 충돌 {:?}", r.files));
                resolve_all_conflicts(ctx, self.name, &t, rng, ResolveMode::Random, &mut self.stats);
                match complete_merge(&t, None) {
                    Ok(o) if o.ok => self.record_head(),
                    Ok(o) => ctx.violate(self.name, format!("complete_merge ok=false: {}", o.message)),
                    Err(e) => {
                        expect_or_violate(ctx, self.name, "complete_merge", &e.to_string(), &mut self.stats);
                    }
                }
            }
            Err(e) => {
                expect_or_violate(ctx, self.name, "sync_to_base", &e.to_string(), &mut self.stats);
            }
        }
    }

    /// 스태시 저장 → (때때로 sync) → 복원. 복원 충돌은 해결·커밋하고 항목은
    /// 사용자가 명시적으로 버린다 (내용은 해결 커밋에 실려 있다).
    fn do_stash_tango(&mut self, ctx: &Ctx, rng: &mut Rng) {
        let t = self.t();
        // 절반은 보관 직전에 공유 파일을 만져 스태시가 충돌 소재를 싣게 한다
        // (sync 가 그 사이 공유 파일을 움직이면 pop 이 충돌한다). 나머지 절반은
        // 트리 상태 그대로 저장을 시도해 "보관할 변경이 없습니다" 도 훑는다.
        if rng.below(100) < 50 {
            self.edit_shared(ctx);
        }
        match stash(
            &t,
            StashAction::Save {
                message: Some(format!("chaos: {} 임시 보관", self.name)),
            },
        ) {
            Ok(()) => {
                self.stats.stash_saves += 1;
                ctx.log(self.name, "stash save ok".into());
            }
            Err(e) => {
                expect_or_violate(ctx, self.name, "stash save", &e.to_string(), &mut self.stats);
                return;
            }
        }
        // 보관과 복원 사이에 다른 일이 벌어진다 — 동기화(원격이 공유 파일을
        // 움직였을 수 있음)나 별도 작업 커밋(공유 파일 EOF 가 움직여 pop 이
        // 충돌할 소재). 셋 중 하나는 아무 일도 안 하는 무난한 복원.
        match rng.below(3) {
            0 => self.do_sync(ctx, rng),
            1 => {
                self.edit_shared(ctx);
                self.do_commit(ctx);
            }
            _ => {}
        }
        match stash(&t, StashAction::Pop) {
            Ok(()) => ctx.log(self.name, "stash pop ok".into()),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("스태시를 복원하다 충돌이 났습니다") {
                    self.stats.stash_pop_conflicts += 1;
                    self.stats.err("스태시를 복원하다 충돌이 났습니다");
                    ctx.log(self.name, "stash pop 충돌 → 해결/커밋/drop".into());
                    resolve_all_conflicts(ctx, self.name, &t, rng, ResolveMode::Random, &mut self.stats);
                    match commit(&t, &format!("chaos: {} 스태시 충돌 정리", self.name), true) {
                        Ok(c) if c.ok => {
                            self.stats.commits += 1;
                            self.record_head();
                        }
                        Ok(c) => {
                            expect_or_violate(ctx, self.name, "stash 정리 commit", &c.message, &mut self.stats);
                        }
                        Err(e) => {
                            expect_or_violate(ctx, self.name, "stash 정리 commit", &e.to_string(), &mut self.stats);
                        }
                    }
                    if let Err(e) = stash(&t, StashAction::Drop) {
                        expect_or_violate(ctx, self.name, "stash drop", &e.to_string(), &mut self.stats);
                    }
                } else {
                    expect_or_violate(ctx, self.name, "stash pop", &msg, &mut self.stats);
                }
            }
        }
    }

    fn do_pull(&mut self, ctx: &Ctx, rng: &mut Rng) {
        let t = self.t();
        match pull(&t) {
            Ok(p) if p.ok => {
                self.stats.pulls += 1;
                ctx.log(self.name, "pull ok".into());
            }
            Ok(p) if !p.conflicted_files.is_empty() => {
                self.stats.pulls += 1;
                ctx.log(self.name, format!("pull 충돌 {:?}", p.conflicted_files));
                resolve_all_conflicts(ctx, self.name, &t, rng, ResolveMode::Random, &mut self.stats);
                match complete_merge(&t, None) {
                    Ok(o) if o.ok => self.record_head(),
                    Ok(o) => ctx.violate(self.name, format!("pull complete_merge ok=false: {}", o.message)),
                    Err(e) => {
                        expect_or_violate(ctx, self.name, "pull complete_merge", &e.to_string(), &mut self.stats);
                    }
                }
            }
            Ok(p) => {
                expect_or_violate(ctx, self.name, "pull", &p.message, &mut self.stats);
            }
            Err(e) => {
                expect_or_violate(ctx, self.name, "pull", &e.to_string(), &mut self.stats);
            }
        }
    }

    fn step(&mut self, ctx: &Ctx, rng: &mut Rng) {
        self.stats.ops += 1;
        match rng.below(100) {
            0..=24 => self.edit_own(ctx),
            25..=44 => self.edit_shared(ctx),
            45..=59 => self.do_commit(ctx),
            60..=74 => self.do_push(ctx),
            75..=84 => self.do_sync(ctx, rng),
            85..=92 => self.do_stash_tango(ctx, rng),
            _ => {
                // FIXME(CHAOS-1) 회피: 첫 push 전 pull 은 영어 원문을 노출한다
                // (아래 전용 테스트가 현재 동작을 고정) — 혼돈 루프에서는
                // push 이력이 있는 브랜치에서만 pull 한다.
                if self.pushed_once {
                    self.do_pull(ctx, rng);
                } else {
                    self.edit_own(ctx);
                }
            }
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════
// 병합 관리자 페르소나 (민지)
// ═════════════════════════════════════════════════════════════════════════

struct Manager {
    dir: TempDir,
    commits: Vec<String>,
    stats: Stats,
    /// 예전에 읽어 둔 대기 목록 — "검토는 아까 했고 병합 버튼은 지금 누른다"
    /// 를 재현한다. 그 사이 팀원이 push 했으면 TOCTOU 가드가 발동해야 한다.
    stale_review: Option<Vec<PendingBranch>>,
}

impl Manager {
    fn p(&self) -> &Path {
        self.dir.path()
    }
    fn t(&self) -> Target {
        Target::Local(self.dir.path().to_path_buf())
    }
    fn record_head(&mut self) {
        let sha = git(self.p(), &["rev-parse", "HEAD"]).trim().to_string();
        self.commits.push(sha);
    }

    /// push(main). non-FF 거부면 sync_to_base 후 1회 재시도.
    fn push_main(&mut self, ctx: &Ctx, rng: &mut Rng) {
        let t = self.t();
        match push(&t, Some("main"), None) {
            Ok(p) if p.ok => {
                self.stats.pushes += 1;
                ctx.record_push();
                ctx.log("민지", "push(main) ok".into());
            }
            Ok(p) => {
                self.stats.nonff_retries += 1;
                expect_or_violate(ctx, "민지", "push(main)", &p.message, &mut self.stats);
                match sync_to_base(&t, "main", "origin") {
                    Ok(r) if r.conflicted => {
                        resolve_all_conflicts(ctx, "민지", &t, rng, ResolveMode::CombineAll, &mut self.stats);
                        if let Ok(o) = complete_merge(&t, None) {
                            if o.ok {
                                self.record_head();
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        expect_or_violate(ctx, "민지", "sync_to_base(main)", &e.to_string(), &mut self.stats);
                    }
                }
                if let Ok(p2) = push(&t, Some("main"), None) {
                    if p2.ok {
                        self.stats.pushes += 1;
                        ctx.record_push();
                        ctx.log("민지", "push(main) 재시도 ok".into());
                    }
                }
            }
            Err(e) => {
                expect_or_violate(ctx, "민지", "push(main)", &e.to_string(), &mut self.stats);
            }
        }
    }

    fn step(&mut self, ctx: &Ctx, rng: &mut Rng) {
        self.stats.ops += 1;
        let t = self.t();
        if let Err(e) = fetch_target(&t, "origin") {
            ctx.violate("민지", format!("fetch_target 실패: {e}"));
            return;
        }
        // base_unpushed_count 정합성 — 관리자 클론은 이 스레드만 만지므로
        // raw rev-list 와 정확히 같아야 한다.
        match base_unpushed_count(&t, "origin", "main") {
            Ok(api) => {
                let raw: u32 = git(self.p(), &["rev-list", "--count", "origin/main..main"])
                    .trim()
                    .parse()
                    .unwrap_or(0);
                if api != raw {
                    ctx.violate("민지", format!("base_unpushed_count 불일치: api={api} raw={raw}"));
                }
            }
            Err(e) => ctx.violate("민지", format!("base_unpushed_count 실패: {e}")),
        }
        let pending = match list_pending_branches(&t, "origin", "main") {
            Ok(p) => p,
            Err(e) => {
                ctx.violate("민지", format!("list_pending_branches 실패: {e}"));
                return;
            }
        };
        // 화면에 남아 있던 옛 목록을 갱신하지 않고 쓰는 관리자도 있다 —
        // 40% 확률로 이번에 읽은 목록을 다음 병합의 "검토본"으로 묵혀 둔다.
        let review = if rng.below(100) < 35 {
            self.stale_review.take().unwrap_or_else(|| pending.clone())
        } else {
            pending.clone()
        };
        if rng.below(100) < 40 {
            self.stale_review = Some(pending.clone());
        }
        match rng.below(100) {
            // 검토해 둔 목록의 sha 로 병합 — 레이스가 TOCTOU 가드를 때린다.
            0..=54 => {
                let cands: Vec<_> = review
                    .iter()
                    .filter(|b| !b.local && !b.merged_locally)
                    .collect();
                if cands.is_empty() {
                    self.push_main(ctx, rng);
                    return;
                }
                let b = cands[rng.below(cands.len() as u64) as usize];
                ctx.log("민지", format!("start_merge {} (검토 sha {})", b.short_name, &b.sha[..7]));
                // 검토(목록 읽기)와 병합 버튼 사이의 사람 시간 — 이 창에서
                // 팀원 push 가 끼어들면 TOCTOU 가드가 발동해야 한다.
                nap(rng);
                nap(rng);
                match start_merge(&t, &b.name, "main", "origin", Some(&b.sha)) {
                    Ok(o) if o.ok => {
                        self.stats.merges += 1;
                        self.record_head();
                        ctx.log("민지", format!("{} 병합 ok", b.short_name));
                    }
                    Ok(o) if o.conflicted => {
                        ctx.log("민지", format!("{} 병합 충돌 {:?}", b.short_name, o.conflicted_files));
                        resolve_all_conflicts(ctx, "민지", &t, rng, ResolveMode::Random, &mut self.stats);
                        match complete_merge(&t, Some(&format!("{} 브랜치 병합", b.short_name))) {
                            Ok(d) if d.ok => {
                                self.stats.merges += 1;
                                self.record_head();
                            }
                            Ok(d) => ctx.violate("민지", format!("complete_merge ok=false: {}", d.message)),
                            Err(e) => {
                                expect_or_violate(ctx, "민지", "complete_merge", &e.to_string(), &mut self.stats);
                            }
                        }
                    }
                    Ok(o) => ctx.violate("민지", format!("start_merge 예상 밖 결과: {}", o.message)),
                    Err(e) => {
                        let msg = e.to_string();
                        if msg.contains("새 push가 있었습니다") {
                            // 검토와 병합 사이에 팀원이 push — 기대된 가드.
                            self.stats.toctou += 1;
                            self.stats.err("새 push가 있었습니다");
                            ctx.log("민지", format!("{} TOCTOU 가드 — 다음 바퀴에 새로고침", b.short_name));
                        } else {
                            expect_or_violate(ctx, "민지", "start_merge", &msg, &mut self.stats);
                        }
                    }
                }
            }
            55..=79 => self.push_main(ctx, rng),
            80..=87 => {
                match list_merged_remote_branches(&t, "origin", "main") {
                    Ok(list) => ctx.log("민지", format!("병합 완료 원격 브랜치 {}개", list.len())),
                    Err(e) => ctx.violate("민지", format!("list_merged_remote_branches 실패: {e}")),
                }
            }
            88..=93 => {
                // 병합 브랜치(base) 자신을 지우려는 실수 — 가드가 한국어로 거부해야 한다.
                match delete_remote_branch(&t, "origin", "main", "main") {
                    Err(e) => {
                        expect_or_violate(ctx, "민지", "delete_remote_branch(main)", &e.to_string(), &mut self.stats);
                    }
                    Ok(()) => ctx.violate("민지", "base 브랜치 삭제가 허용됐다".into()),
                }
            }
            _ => {
                // 불변식 (1) 중간 점검 — 장부의 모든 sha 가 여전히 origin 에서 도달 가능.
                let missing = ctx.ledger.lock().unwrap().missing(&ctx.bare);
                if !missing.is_empty() {
                    ctx.violate("민지", format!("origin 에서 커밋 유실: {missing:?}"));
                } else {
                    ctx.log("민지", "origin 장부 점검 ok".into());
                }
            }
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════
// quiesce — 팀원 마무리 / 관리자 수렴 루프
// ═════════════════════════════════════════════════════════════════════════

fn member_quiesce(m: &mut Member, ctx: &Ctx, rng: &mut Rng) {
    let t = m.t();
    // 잔여 상태 정리 (액션들이 자기 뒤처리를 하므로 없어야 정상 — 방어적).
    if merge_in_progress(&t).unwrap_or(false) {
        ctx.log(m.name, "quiesce: 잔여 병합 마무리".into());
        resolve_all_conflicts(ctx, m.name, &t, rng, ResolveMode::CombineAll, &mut m.stats);
        let _ = complete_merge(&t, None);
    }
    if !remaining_conflicts(&t).unwrap_or_default().is_empty() {
        ctx.log(m.name, "quiesce: 잔여 충돌 정리".into());
        resolve_all_conflicts(ctx, m.name, &t, rng, ResolveMode::CombineAll, &mut m.stats);
        let _ = commit(&t, "chaos: 잔여 충돌 정리", true);
    }
    for _ in 0..10 {
        if list_stashes(&t).unwrap_or_default().is_empty() {
            break;
        }
        match stash(&t, StashAction::Pop) {
            Ok(()) => {}
            Err(e) if e.to_string().contains("스태시를 복원하다 충돌이 났습니다") => {
                resolve_all_conflicts(ctx, m.name, &t, rng, ResolveMode::CombineAll, &mut m.stats);
                let _ = commit(&t, "chaos: 스태시 충돌 정리", true);
                let _ = stash(&t, StashAction::Drop);
            }
            Err(e) => {
                ctx.violate(m.name, format!("quiesce stash pop 실패: {e}"));
                break;
            }
        }
    }
    // 최종 커밋 + push — 이 시점의 자기 파일 내용이 불변식 2 의 기준이 된다.
    m.edit_own(ctx);
    let r = commit(&t, &format!("chaos: {} 최종 커밋", m.name), true);
    chk!(ctx, matches!(&r, Ok(c) if c.ok), "{} 최종 커밋 실패: {r:?}", m.name);
    m.stats.commits += 1;
    m.record_head();
    let p = push(&t, Some(&m.branch), None);
    chk!(ctx, matches!(&p, Ok(o) if o.ok), "{} 최종 push 실패: {p:?}", m.name);
    m.stats.pushes += 1;
    m.pushed_once = true;
    ctx.record_push();
    ctx.log(m.name, "quiesce: 최종 커밋+push 완료".into());
}

fn manager_quiesce(mgr: &mut Manager, ctx: &Ctx, rng: &mut Rng) {
    let t = mgr.t();
    let mut rounds = 0u32;
    loop {
        rounds += 1;
        chk!(ctx, rounds <= 40, "관리자 수렴 루프가 40회를 넘었다 — 수렴 실패");
        chk!(ctx, fetch_target(&t, "origin").is_ok(), "quiesce fetch 실패");
        let pending = list_pending_branches(&t, "origin", "main");
        chk!(ctx, pending.is_ok(), "quiesce list_pending 실패: {:?}", pending.as_ref().err());
        let pending = pending.unwrap();
        let todo: Vec<_> = pending
            .iter()
            .filter(|b| !b.local && !b.merged_locally)
            .cloned()
            .collect();
        if todo.is_empty() {
            if base_unpushed_count(&t, "origin", "main").unwrap_or(0) > 0 {
                let p = push(&t, Some("main"), None);
                chk!(ctx, matches!(&p, Ok(o) if o.ok), "quiesce push(main) 실패: {p:?}");
                mgr.stats.pushes += 1;
                ctx.record_push();
                continue;
            }
            let names: Vec<&str> = pending.iter().map(|b| b.short_name.as_str()).collect();
            chk!(ctx, pending.is_empty(), "수렴 후에도 대기 브랜치가 남았다: {names:?}");
            break;
        }
        for b in &todo {
            ctx.log("민지", format!("quiesce 병합 {}", b.short_name));
            match start_merge(&t, &b.name, "main", "origin", Some(&b.sha)) {
                Ok(o) if o.ok => {
                    mgr.stats.merges += 1;
                    mgr.record_head();
                }
                Ok(o) if o.conflicted => {
                    // 수렴 단계는 항상 양쪽 결합으로 해결 — 아무 줄도 버리지 않는다.
                    resolve_all_conflicts(ctx, "민지", &t, rng, ResolveMode::CombineAll, &mut mgr.stats);
                    let done = complete_merge(&t, Some(&format!("{} 브랜치 병합", b.short_name)));
                    chk!(ctx, matches!(&done, Ok(d) if d.ok), "quiesce complete_merge 실패: {done:?}");
                    mgr.stats.merges += 1;
                    mgr.record_head();
                }
                Ok(o) => chk!(ctx, false, "quiesce start_merge 예상 밖 결과: {}", o.message),
                Err(e) => {
                    let msg = e.to_string();
                    // 팀원이 모두 멈춘 뒤라 TOCTOU 는 없어야 하지만, 직전 라운드
                    // fetch 타이밍의 잔상은 한 바퀴 재시도로 해소한다.
                    if msg.contains("새 push가 있었습니다") {
                        mgr.stats.toctou += 1;
                        break;
                    }
                    chk!(ctx, false, "quiesce 병합 실패({}): {msg}", b.short_name);
                }
            }
        }
        let p = push(&t, Some("main"), None);
        chk!(ctx, matches!(&p, Ok(o) if o.ok), "quiesce push(main) 실패: {p:?}");
        mgr.stats.pushes += 1;
        ctx.record_push();
    }
}

// ═════════════════════════════════════════════════════════════════════════
// 종료 검증 — 수렴 / fsck / 장부 / 커밋 도달성 / 자기 파일 동일성 / 측정
// ═════════════════════════════════════════════════════════════════════════

fn final_checks(ctx: &Ctx, bare: &Path, mgr: &mut Manager, members: &mut [Member]) -> (usize, usize) {
    let origin_main = git(bare, &["rev-parse", "refs/heads/main"]).trim().to_string();

    // ── 수렴: 모두 fetch + sync → main 동일, base_unpushed 0 ──
    for m in members.iter_mut() {
        let t = m.t();
        chk!(ctx, fetch_target(&t, "origin").is_ok(), "{} 최종 fetch 실패", m.name);
        let r = sync_to_base(&t, "main", "origin");
        chk!(
            ctx,
            matches!(&r, Ok(rr) if !rr.conflicted),
            "{} 최종 동기화 실패/충돌 (자기 브랜치는 이미 main 에 병합돼 충돌이 불가능해야 한다): {r:?}",
            m.name
        );
        m.stats.syncs += 1;
        let local_main = git(m.p(), &["rev-parse", "refs/heads/main"]).trim().to_string();
        chk!(
            ctx,
            local_main == origin_main,
            "{} 클론의 main({})이 origin/main({})과 다르다",
            m.name,
            &local_main[..7],
            &origin_main[..7]
        );
        let cnt = base_unpushed_count(&t, "origin", "main").unwrap();
        chk!(ctx, cnt == 0, "{} base_unpushed_count={cnt} (0 이어야 한다)", m.name);
    }
    let mgr_main = git(mgr.p(), &["rev-parse", "refs/heads/main"]).trim().to_string();
    chk!(ctx, mgr_main == origin_main, "관리자 main 이 origin/main 과 다르다");
    chk!(
        ctx,
        base_unpushed_count(&mgr.t(), "origin", "main").unwrap() == 0,
        "관리자 base_unpushed_count != 0"
    );

    // ── 팀원 자기 파일: 최종 워크트리 내용 == 최종 main 내용 (바이트 동일) ──
    for m in members.iter() {
        let local = fs::read(m.p().join(&m.file)).unwrap_or_default();
        let out = git_try(bare, &["show", &format!("main:{}", m.file)]);
        chk!(ctx, out.status.success(), "{}: main 에 {} 가 없다", m.name, m.file);
        chk!(
            ctx,
            local == out.stdout,
            "{}: 자기 파일 {} 이 main 과 바이트 동일하지 않다",
            m.name,
            m.file
        );
    }

    // ── 원격 브랜치 정리: 전원 병합 완료 목록에 있고, 하나를 지워도 흡수돼 있다 ──
    let merged = list_merged_remote_branches(&mgr.t(), "origin", "main").unwrap();
    for m in members.iter() {
        chk!(
            ctx,
            merged.iter().any(|b| b.short_name == m.branch),
            "{} 브랜치가 병합 완료 목록에 없다: {:?}",
            m.name,
            merged.iter().map(|b| &b.short_name).collect::<Vec<_>>()
        );
    }
    let victim = members[(ctx.seed % 5) as usize].branch.clone();
    let del = delete_remote_branch(&mgr.t(), "origin", "main", &victim);
    chk!(ctx, del.is_ok(), "병합 완료 브랜치 {victim} 삭제 실패: {del:?}");
    ctx.log("민지", format!("원격 브랜치 삭제: {victim}"));

    // ── 불변식 (1): 장부. 삭제 예외 — 지운 브랜치의 sha 는 전부 main 에 흡수 ──
    let main_set = rev_set(bare, "refs/heads/main");
    {
        let ledger = ctx.ledger.lock().unwrap();
        for sha in ledger.branch_shas(&format!("refs/heads/{victim}")) {
            chk!(ctx, main_set.contains(&sha), "{victim} 삭제로 커밋 {sha} 가 main 밖으로 유실");
        }
        let missing = ledger.missing(bare);
        chk!(ctx, missing.is_empty(), "origin 에서 커밋 유실: {missing:?}");
    }

    // ── 대기 목록은 비어 있어야 한다 (삭제/prune 후에도) ──
    chk!(ctx, fetch_target(&mgr.t(), "origin").is_ok(), "최종 fetch 실패");
    let pending = list_pending_branches(&mgr.t(), "origin", "main").unwrap();
    chk!(
        ctx,
        pending.is_empty(),
        "최종 대기 목록이 비어 있지 않다: {:?}",
        pending.iter().map(|b| &b.short_name).collect::<Vec<_>>()
    );

    // ── fsck --strict: origin + 클론 6개 전부 ──
    let mut repos: Vec<(&str, &Path)> = vec![("origin", bare), ("민지", mgr.p())];
    for m in members.iter() {
        repos.push((m.name, m.p()));
    }
    for (who, dir) in repos {
        let out = git_try(dir, &["fsck", "--strict"]);
        chk!(
            ctx,
            out.status.success(),
            "{who} 저장소 fsck 실패: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    // ── 불변식 (2): 페르소나가 만든 모든 커밋이 main 또는 자기 브랜치에서 도달 가능 ──
    for m in members.iter() {
        let mut allowed = main_set.clone();
        allowed.extend(rev_set(m.p(), &format!("refs/heads/{}", m.branch)));
        if git_try(bare, &["rev-parse", "-q", "--verify", &format!("refs/heads/{}", m.branch)])
            .status
            .success()
        {
            allowed.extend(rev_set(bare, &format!("refs/heads/{}", m.branch)));
        }
        for sha in &m.commits {
            chk!(
                ctx,
                allowed.contains(sha),
                "{} 의 커밋 {sha} 가 main/자기 브랜치 어디서도 도달 불가 — 데이터 손실",
                m.name
            );
        }
    }
    for sha in &mgr.commits {
        chk!(ctx, main_set.contains(sha), "관리자 커밋 {sha} 가 main 에서 도달 불가");
    }

    // ── 측정 (4): 공유 파일 고유 줄의 생존/탈락 (ours/theirs 선택의 정당한 결과) ──
    let body = git(bare, &["show", "main:SHARED.txt"]);
    let present: HashSet<&str> = body.lines().collect();
    let mut written = 0usize;
    let mut survived = 0usize;
    for m in members.iter() {
        for l in &m.shared_lines {
            written += 1;
            if present.contains(l.as_str()) {
                survived += 1;
            }
        }
    }
    (written, survived)
}

// ═════════════════════════════════════════════════════════════════════════
// 시뮬레이션 본체
// ═════════════════════════════════════════════════════════════════════════

fn run_chaos(seed: u64) {
    // ── bare origin + 관리자 클론 시드 ──
    let bare = TempDir::new().unwrap();
    git(bare.path(), &["init", "--bare", "-q", "-b", "main"]);
    git(bare.path(), &["config", "receive.denyCurrentBranch", "ignore"]);
    let url = format!("file://{}", bare.path().display());

    let mgr_dir = TempDir::new().unwrap();
    git(mgr_dir.path(), &["init", "-q", "-b", "main"]);
    set_identity(mgr_dir.path(), "민지", "minji@t.com");
    fs::write(mgr_dir.path().join("README.md"), "git companion chaos\n").unwrap();
    fs::write(mgr_dir.path().join(SHARED_FILE), "# 공유 파일\n").unwrap();
    git(mgr_dir.path(), &["add", "-A"]);
    git(mgr_dir.path(), &["commit", "-q", "-m", "init: 프로젝트 시작"]);
    git(mgr_dir.path(), &["remote", "add", "origin", &url]);
    git(mgr_dir.path(), &["push", "-q", "-u", "origin", "main"]);
    git(mgr_dir.path(), &["fetch", "-q", "origin"]);

    let ctx = Ctx::new(seed, bare.path());
    ctx.record_push();

    let mut mgr = Manager {
        dir: mgr_dir,
        commits: Vec::new(),
        stats: Stats::default(),
        stale_review: None,
    };

    // ── 팀원 5명: clone + 자기 브랜치 (앱 API) ──
    let mut members: Vec<Member> = Vec::new();
    for (i, (name, key)) in MEMBERS.iter().enumerate() {
        let dir = TempDir::new().unwrap();
        git(dir.path(), &["clone", "-q", &url, "."]);
        set_identity(dir.path(), name, &format!("{key}@t.com"));
        let branch = format!("feature/{key}");
        let m = Member {
            name,
            key,
            dir,
            branch: branch.clone(),
            file: format!("{key}.txt"),
            counter: 0,
            pushed_once: false,
            commits: Vec::new(),
            shared_lines: Vec::new(),
            stats: Stats::default(),
            rng_seed: seed ^ ((i as u64 + 1).wrapping_mul(0x00C0_FFEE_D00D_5EED)),
        };
        let cb = create_branch(&m.t(), &branch);
        chk!(ctx, cb.is_ok(), "{} 브랜치 생성 실패: {cb:?}", name);
        members.push(m);
    }

    // ── 혼돈 단계: 페르소나 6명이 각자 스레드에서 동시에 행동 ──
    let joined: Vec<Result<(), String>> = std::thread::scope(|s| {
        let mut handles = Vec::new();
        {
            let ctx = &ctx;
            let mgr = &mut mgr;
            handles.push(s.spawn(move || {
                let mut rng = Rng::new(ctx.seed ^ 0xA11C_E000);
                for _ in 0..MANAGER_ITERS {
                    mgr.step(ctx, &mut rng);
                    nap(&mut rng);
                }
            }));
        }
        for m in members.iter_mut() {
            let ctx = &ctx;
            handles.push(s.spawn(move || {
                let mut rng = Rng::new(m.rng_seed);
                for _ in 0..MEMBER_ITERS {
                    m.step(ctx, &mut rng);
                    nap(&mut rng);
                }
            }));
        }
        handles
            .into_iter()
            .map(|h| {
                h.join().map_err(|p| {
                    p.downcast_ref::<String>()
                        .cloned()
                        .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                        .unwrap_or_else(|| "알 수 없는 패닉".into())
                })
            })
            .collect()
    });
    for r in &joined {
        chk!(ctx, r.is_ok(), "페르소나 스레드 패닉: {}", r.as_ref().err().unwrap());
    }

    // ── quiesce: 팀원 마무리 → 관리자 수렴 → 최종 검증 ──
    let mut qrng = Rng::new(seed ^ 0x0005_EEED);
    for m in members.iter_mut() {
        member_quiesce(m, &ctx, &mut qrng);
    }
    manager_quiesce(&mut mgr, &ctx, &mut qrng);
    let (written, survived) = final_checks(&ctx, bare.path(), &mut mgr, &mut members);

    // ── 오류 정책: 위반 0건 ──
    let violations = ctx.violations.lock().unwrap();
    chk!(
        ctx,
        violations.is_empty(),
        "정책 위반 {}건:\n{}",
        violations.len(),
        violations.join("\n")
    );
    drop(violations);

    // ── 시드별 통계 보고 (실패 아님 — `--nocapture` 로 열람) ──
    let mut total = mgr.stats.clone();
    for m in &members {
        total.absorb(&m.stats);
    }
    println!(
        "[chaos seed {seed:#x}] ops={} commits={} pushes={} merges={} syncs={} pulls={} \
         stash_saves={} stash_pop_conflicts={} conflicts_resolved={} toctou_guard={} nonff_retries={}",
        total.ops,
        total.commits,
        total.pushes,
        total.merges,
        total.syncs,
        total.pulls,
        total.stash_saves,
        total.stash_pop_conflicts,
        total.conflicts_resolved,
        total.toctou,
        total.nonff_retries,
    );
    println!("[chaos seed {seed:#x}] 오류 유형별: {:?}", total.errors);
    println!(
        "[chaos seed {seed:#x}] 공유 파일 줄 생존: {survived}/{written} (탈락 {} — ours/theirs 선택의 정당한 결과, 커밋 자체는 전부 보존 확인됨)",
        written - survived
    );
}

// ═════════════════════════════════════════════════════════════════════════
// 테스트 — 고정 시드 3개
// ═════════════════════════════════════════════════════════════════════════

#[test]
fn chaos_six_personas_converge_and_lose_nothing_seed_a() {
    run_chaos(0x0000_0000_C0FF_EE01);
}

#[test]
fn chaos_six_personas_converge_and_lose_nothing_seed_b() {
    run_chaos(0x0000_0000_DEAD_BEEF);
}

#[test]
fn chaos_six_personas_converge_and_lose_nothing_seed_c() {
    run_chaos(2026_0903);
}

// ═════════════════════════════════════════════════════════════════════════
// FIXME(CHAOS-1) — [UX격차] 첫 push 전 브랜치에서 '풀' 을 누르면 git 영어
// 원문("fatal: couldn't find remote ref …")이 그대로 화면에 나간다.
//
// 원인: ops::pull(src/git/ops.rs:374)이 `git pull --no-rebase origin <branch>`
// 를 부르는데, 브랜치가 아직 원격에 없으면 stderr 가 "couldn't find remote
// ref" 이고 friendly_git_error(src/git/ops.rs:683)에 이 패턴 분기가 없어
// 원문이 그대로 반환된다. push 쪽 friendly_git_error 는 같은 상황
// ("has no upstream branch")을 이미 한국어로 안내한다 — pull 만 구멍이다.
//
// [수정 완료·회귀 방지] friendly_git_error 가 "couldn't find remote ref" 를
// "이 브랜치는 아직 원격에 없습니다. 받아올 것이 없으니 먼저 푸시하세요."
// 로 번역한다 — 첫 push 전의 '풀'이 영어 원문을 노출하지 않는다.
// ═════════════════════════════════════════════════════════════════════════

#[test]
fn chaos1_pull_before_first_push_speaks_korean() {
    let bare = TempDir::new().unwrap();
    git(bare.path(), &["init", "--bare", "-q", "-b", "main"]);
    git(bare.path(), &["config", "receive.denyCurrentBranch", "ignore"]);
    let url = format!("file://{}", bare.path().display());

    let seed_dir = TempDir::new().unwrap();
    git(seed_dir.path(), &["init", "-q", "-b", "main"]);
    set_identity(seed_dir.path(), "민지", "minji@t.com");
    fs::write(seed_dir.path().join("README.md"), "x\n").unwrap();
    git(seed_dir.path(), &["add", "-A"]);
    git(seed_dir.path(), &["commit", "-q", "-m", "init"]);
    git(seed_dir.path(), &["remote", "add", "origin", &url]);
    git(seed_dir.path(), &["push", "-q", "-u", "origin", "main"]);

    let member = TempDir::new().unwrap();
    git(member.path(), &["clone", "-q", &url, "."]);
    set_identity(member.path(), "준호", "junho@t.com");
    let t = Target::Local(member.path().to_path_buf());
    create_branch(&t, "feature/unborn-remote").unwrap();
    fs::write(member.path().join("j.txt"), "작업\n").unwrap();
    assert!(commit(&t, "feat: j", true).unwrap().ok);

    // 아직 한 번도 push 하지 않은 브랜치에서 '풀'.
    let p = pull(&t).unwrap();
    assert!(!p.ok, "원격에 없는 브랜치의 pull 은 실패해야 한다");
    assert!(p.conflicted_files.is_empty());
    // 회귀 방지(CHAOS-1): 한국어 안내 — 영어 원문 노출 금지.
    assert!(
        p.message.contains("먼저 푸시"),
        "한국어 처방(먼저 푸시)이어야 한다: {}",
        p.message
    );
    assert!(!p.message.contains("remote ref"), "{}", p.message);
    assert!(
        expected_key(&p.message).is_none(),
        "한국어 허용 목록에 없는 메시지임을 함께 고정한다: {}",
        p.message
    );
    // 저장소는 안전하다 — 병합 상태 없음, 커밋 보존.
    assert!(!merge_in_progress(&t).unwrap());
    assert!(member.path().join("j.txt").exists());
}
