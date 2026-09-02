//! CONFLICT HELL — team simulation stress tests.
//!
//! A 6-person team (1 merge manager + 5 members) shares one bare origin.
//! Every member is a separate clone; every app action goes through the
//! crate's public API (`git_companion::git::*` / `gpconfig::*`) with
//! `Target::Local`. Raw git is used only for setup and verification.
//!
//! 회귀 테스트: 아래 BUG-n 은 소스에서 수정 완료 — 각 테스트는 수정된 새
//! 동작을 고정한다 (BUG-4 만 의도적으로 유지된 현재 동작을 문서화).
//! 번호는 최종 findings 리포트와 같다:
//!   BUG-2  .gpconfig 충돌 중 read_config_effective가 커밋본으로 폴백 (수정)
//!   BUG-3  CRLF 충돌 마커의 \r\n 형태 고정 (프런트 파서가 \r 허용하도록 수정)
//!   BUG-4  rename/rename 에서 빠진 스테이지는 빈 문자열/None (문서화된 동작)
//!   BUG-5  Manual 해결이 충돌 마커 남은 본문을 거부 (수정)
//!   BUG-6  valid_ai_body가 줄 단위 판정 — 정당한 "=======" 허용 (수정)
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Mutex;

use tempfile::TempDir;

use git_companion::git::auto::{auto_resolve_merge, list_backups, restore_backup, AutoResolveOptions};
use git_companion::git::merge::{
    complete_merge, conflict_detail, list_pending_branches, merge_in_progress,
    remaining_conflicts, resolve_conflict, start_merge, Resolution,
};
use git_companion::git::{push, sync_to_base, Target};
use git_companion::gpconfig::{self, member_from_account, ProjectConfig};

/// Serializes the tests that mutate the process-global GC_BACKUP_DIR env var.
/// (This file is its own test process, so a file-local mutex suffices.)
static ENV_LOCK: Mutex<()> = Mutex::new(());

// ── Raw-git helpers (setup / verification only) ─────────────────────────────

fn git_try(dir: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .current_dir(dir)
        .env("LC_ALL", "C.UTF-8")
        .env("LANG", "C.UTF-8")
        .args(args)
        .output()
        .expect("failed to spawn git")
}

fn git(dir: &Path, args: &[&str]) -> Output {
    let out = git_try(dir, args);
    assert!(
        out.status.success(),
        "git {args:?} failed in {dir:?}:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn git_stdout(dir: &Path, args: &[&str]) -> String {
    String::from_utf8_lossy(&git(dir, args).stdout)
        .trim()
        .to_string()
}

fn write_file(dir: &Path, rel: &str, bytes: &[u8]) {
    let full = dir.join(rel);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(full, bytes).unwrap();
}

/// Per-clone identity + deterministic merge output. `merge.conflictstyle`
/// is pinned to "merge" because the host machine may carry diff3 globally
/// (this dev box does!) which changes marker shape.
fn configure(dir: &Path, name: &str, email: &str) {
    git(dir, &["config", "user.email", email]);
    git(dir, &["config", "user.name", name]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    git(dir, &["config", "merge.conflictstyle", "merge"]);
}

/// Bare origin + the merge manager's clone, with `seed` files committed on
/// main and pushed. Returns (bare_dir, manager_dir) — TempDirs keep both alive.
fn team_origin(seed: &[(&str, &[u8])]) -> (TempDir, TempDir, PathBuf) {
    let bare_td = TempDir::new().unwrap();
    let bare = bare_td.path().join("origin.git");
    std::fs::create_dir(&bare).unwrap();
    git(&bare, &["init", "--bare", "-q", "-b", "main"]);

    let (mgr_td, mgr) = clone_member(&bare, "관리자", "manager@team.x");
    for (rel, bytes) in seed {
        write_file(&mgr, rel, bytes);
    }
    git(&mgr, &["add", "-A"]);
    git(&mgr, &["commit", "-q", "-m", "seed"]);
    git(&mgr, &["push", "-q", "-u", "origin", "main"]);

    // TempDir of the bare must outlive the clones.
    (bare_td, mgr_td, mgr)
}

fn bare_path(bare_td: &TempDir) -> PathBuf {
    bare_td.path().join("origin.git")
}

/// A team member: a fresh clone of the bare origin with its own identity.
fn clone_member(bare: &Path, name: &str, email: &str) -> (TempDir, PathBuf) {
    let td = TempDir::new().unwrap();
    let repo = td.path().join("repo");
    git(
        td.path(),
        &["clone", "-q", bare.to_str().unwrap(), repo.to_str().unwrap()],
    );
    configure(&repo, name, email);
    (td, repo)
}

/// Member workflow: branch off origin/main, apply edits, commit, push.
fn member_push_branch(repo: &Path, branch: &str, msg: &str, edits: impl FnOnce(&Path)) {
    git(repo, &["checkout", "-q", "-b", branch]);
    edits(repo);
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "-m", msg]);
    git(repo, &["push", "-q", "-u", "origin", branch]);
}

fn target(repo: &Path) -> Target {
    Target::Local(repo.to_path_buf())
}

fn read(repo: &Path, rel: &str) -> Vec<u8> {
    std::fs::read(repo.join(rel)).unwrap()
}

fn read_str(repo: &Path, rel: &str) -> String {
    String::from_utf8(read(repo, rel)).unwrap()
}

/// Slug used by auto.rs::backup_root for a repo path (mirrors git_auto_merge.rs).
fn backup_slug(repo: &Path) -> String {
    repo.to_string_lossy()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

// ── Test-side mirror of ui/components/conflictParser.ts ────────────────────
// Simplified mirror covering the canonical, well-formed marker shape used in
// this file (column-0 markers, `=======` exact line, non-empty replacements).
// The real TS parser additionally handles diff3 base sections, CRLF markers,
// stray start-marker lookalikes and empty(=delete) replacements — those edge
// cases are covered by ui/components/conflictParser.edge.test.ts.

struct Block {
    start_line: usize, // 1-based, inclusive (the <<<<<<< line)
    end_line: usize,   // 1-based, inclusive (the >>>>>>> line)
    ours: String,
    theirs: String,
}

fn parse_blocks(content: &str) -> Vec<Block> {
    let lines: Vec<&str> = content.split('\n').collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < lines.len() {
        if !lines[i].starts_with("<<<<<<< ") {
            i += 1;
            continue;
        }
        let start_line = i + 1;
        let mut j = i + 1;
        while j < lines.len() && lines[j] != "=======" {
            j += 1;
        }
        if j >= lines.len() {
            break;
        }
        let ours = lines[i + 1..j].join("\n");
        let mut k = j + 1;
        while k < lines.len() && !lines[k].starts_with(">>>>>>> ") {
            k += 1;
        }
        if k >= lines.len() {
            break;
        }
        let theirs = lines[j + 1..k].join("\n");
        out.push(Block {
            start_line,
            end_line: k + 1,
            ours,
            theirs,
        });
        i = k + 1;
    }
    out
}

fn reassemble(content: &str, blocks: &[Block], replacements: &[String]) -> String {
    assert_eq!(blocks.len(), replacements.len());
    let lines: Vec<&str> = content.split('\n').collect();
    let mut out: Vec<String> = Vec::new();
    let mut cursor = 1usize; // 1-based
    for (b, rep) in blocks.iter().zip(replacements) {
        for l in &lines[cursor - 1..b.start_line - 1] {
            out.push((*l).to_string());
        }
        out.push(rep.clone());
        cursor = b.end_line + 1;
    }
    if cursor - 1 < lines.len() {
        for l in &lines[cursor - 1..] {
            out.push((*l).to_string());
        }
    }
    out.join("\n")
}

// ═════════════════════════════════════════════════════════════════════════════
// 1. Five-branch pileup on one file — sequential merges, mixed resolutions.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn five_branch_pileup_on_one_file() {
    let base_shared = b"alpha\nbravo\ncharlie\ndelta\necho\n";
    let (bare_td, _mgr_td, mgr) = team_origin(&[("shared.txt", base_shared)]);
    let bare = bare_path(&bare_td);

    // Five members, all branching from the same origin/main, all touching the
    // same line of shared.txt plus a file of their own.
    let mut member_tds = Vec::new();
    for i in 1..=5u32 {
        let (td, repo) = clone_member(&bare, &format!("멤버{i}"), &format!("m{i}@team.x"));
        member_push_branch(&repo, &format!("feature/m{i}"), &format!("feat m{i}"), |r| {
            write_file(
                r,
                "shared.txt",
                format!("alpha\nbravo-m{i}\ncharlie\ndelta\necho\n").as_bytes(),
            );
            write_file(r, &format!("m{i}.txt"), format!("work of m{i}\n").as_bytes());
        });
        member_tds.push(td);
    }

    // Manager sees all five as pending.
    git(&mgr, &["fetch", "-q", "--prune", "origin"]);
    let t = target(&mgr);
    let pending = list_pending_branches(&t, "origin", "main").unwrap();
    assert_eq!(pending.len(), 5, "five member branches pending");
    for i in 1..=5u32 {
        assert!(
            pending.iter().any(|b| b.short_name == format!("feature/m{i}")),
            "feature/m{i} must be pending"
        );
    }

    // Round 1: first merge is clean (main hasn't moved yet).
    let out = start_merge(&t, "origin/feature/m1", "main", "origin", None).unwrap();
    assert!(out.ok && !out.conflicted, "first merge is conflict-free");
    let p = push(&t, Some("main"), None).unwrap();
    assert!(p.ok, "push after m1: {}", p.message);
    assert_eq!(
        list_pending_branches(&t, "origin", "main").unwrap().len(),
        4,
        "pending shrinks to 4 after m1"
    );

    // Round 2: conflict, resolve OURS (keep m1's shared.txt line; m2's own
    // file still arrives via the merge).
    let out = start_merge(&t, "origin/feature/m2", "main", "origin", None).unwrap();
    assert!(out.conflicted);
    assert_eq!(out.conflicted_files, vec!["shared.txt".to_string()]);
    let d = conflict_detail(&t, "shared.txt").unwrap();
    assert!(d.ours.contains("bravo-m1"), "ours = merged main state");
    assert!(d.theirs.contains("bravo-m2"), "theirs = incoming branch");
    assert_eq!(
        d.base.as_deref(),
        Some(std::str::from_utf8(base_shared).unwrap()),
        "base = common ancestor"
    );
    let rem = resolve_conflict(&t, "shared.txt", &Resolution::Ours).unwrap();
    assert!(rem.is_empty());
    let done = complete_merge(&t, Some("feature/m2 브랜치 병합")).unwrap();
    assert!(done.ok);
    assert!(mgr.join("m2.txt").exists(), "m2's own file landed despite Ours");
    let p = push(&t, Some("main"), None).unwrap();
    assert!(p.ok);
    assert_eq!(list_pending_branches(&t, "origin", "main").unwrap().len(), 3);
    assert!(read_str(&mgr, "shared.txt").contains("bravo-m1"));

    // Round 3: conflict, resolve THEIRS.
    let out = start_merge(&t, "origin/feature/m3", "main", "origin", None).unwrap();
    assert!(out.conflicted);
    let rem = resolve_conflict(&t, "shared.txt", &Resolution::Theirs).unwrap();
    assert!(rem.is_empty());
    assert!(complete_merge(&t, Some("feature/m3 브랜치 병합")).unwrap().ok);
    assert!(push(&t, Some("main"), None).unwrap().ok);
    assert_eq!(list_pending_branches(&t, "origin", "main").unwrap().len(), 2);
    assert!(read_str(&mgr, "shared.txt").contains("bravo-m3"));

    // Round 4: conflict, resolve MANUAL (combine).
    let out = start_merge(&t, "origin/feature/m4", "main", "origin", None).unwrap();
    assert!(out.conflicted);
    let manual4 = "alpha\nbravo-m3+m4\ncharlie\ndelta\necho\n";
    let rem = resolve_conflict(
        &t,
        "shared.txt",
        &Resolution::Manual {
            content: manual4.into(),
        },
    )
    .unwrap();
    assert!(rem.is_empty());
    assert!(complete_merge(&t, Some("feature/m4 브랜치 병합")).unwrap().ok);
    assert!(push(&t, Some("main"), None).unwrap().ok);
    assert_eq!(list_pending_branches(&t, "origin", "main").unwrap().len(), 1);

    // Round 5: conflict, resolve MANUAL again.
    let out = start_merge(&t, "origin/feature/m5", "main", "origin", None).unwrap();
    assert!(out.conflicted);
    let final_shared = "alpha\nbravo-m3+m4+m5\ncharlie\ndelta\necho\n";
    let rem = resolve_conflict(
        &t,
        "shared.txt",
        &Resolution::Manual {
            content: final_shared.into(),
        },
    )
    .unwrap();
    assert!(rem.is_empty());
    assert!(complete_merge(&t, Some("feature/m5 브랜치 병합")).unwrap().ok);
    assert!(push(&t, Some("main"), None).unwrap().ok);
    assert!(
        list_pending_branches(&t, "origin", "main").unwrap().is_empty(),
        "no branch left pending after the pileup"
    );
    assert!(!merge_in_progress(&t).unwrap());

    // No commit lost: every member's own file is on remote main and every
    // member's branch tip is an ancestor of main.
    for i in 1..=5u32 {
        let show = git_try(&bare, &["show", &format!("main:m{i}.txt")]);
        assert!(show.status.success(), "m{i}.txt must be on remote main");
        let tip = git_stdout(&mgr, &["rev-parse", &format!("origin/feature/m{i}")]);
        let anc = git_try(&mgr, &["merge-base", "--is-ancestor", &tip, "main"]);
        assert!(anc.status.success(), "feature/m{i} tip must be inside main");
    }
    // History carries exactly 5 merge commits and the final file state is sane.
    assert_eq!(git_stdout(&mgr, &["rev-list", "--merges", "--count", "main"]), "5");
    assert_eq!(read_str(&mgr, "shared.txt"), final_shared);
    let head_shared = git_stdout(&mgr, &["show", "HEAD:shared.txt"]);
    assert_eq!(head_shared, final_shared.trim_end_matches('\n'));
}

// ═════════════════════════════════════════════════════════════════════════════
// 2. add/add — two members create the SAME new file with different content.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn add_add_conflict_has_no_base_and_correct_sides() {
    let (bare_td, _mgr_td, mgr) = team_origin(&[("app.txt", b"v1\n")]);
    let bare = bare_path(&bare_td);

    let (_td1, r1) = clone_member(&bare, "멤버1", "n1@team.x");
    member_push_branch(&r1, "feature/n1", "adds config/new.txt", |r| {
        write_file(r, "config/new.txt", b"from-n1\nshared tail\n");
    });
    let (_td2, r2) = clone_member(&bare, "멤버2", "n2@team.x");
    member_push_branch(&r2, "feature/n2", "also adds config/new.txt", |r| {
        write_file(r, "config/new.txt", b"from-n2\nshared tail\n");
    });

    git(&mgr, &["fetch", "-q", "--prune", "origin"]);
    let t = target(&mgr);
    let out = start_merge(&t, "origin/feature/n1", "main", "origin", None).unwrap();
    assert!(out.ok && !out.conflicted);
    assert!(push(&t, Some("main"), None).unwrap().ok);

    let out = start_merge(&t, "origin/feature/n2", "main", "origin", None).unwrap();
    assert!(out.conflicted, "add/add must conflict");
    assert_eq!(out.conflicted_files, vec!["config/new.txt".to_string()]);

    let d = conflict_detail(&t, "config/new.txt").unwrap();
    assert!(d.base.is_none(), "add/add has no base stage → None");
    assert_eq!(d.ours, "from-n1\nshared tail\n", "ours = already-merged n1 body");
    assert_eq!(d.theirs, "from-n2\nshared tail\n", "theirs = incoming n2 body");
    assert!(!d.is_binary && !d.too_large);
    assert!(
        d.working.contains("<<<<<<< ") && d.working.contains("from-n1"),
        "working copy carries the marked union"
    );

    let merged = "from-n1\nfrom-n2\nshared tail\n";
    let rem = resolve_conflict(
        &t,
        "config/new.txt",
        &Resolution::Manual {
            content: merged.into(),
        },
    )
    .unwrap();
    assert!(rem.is_empty());
    assert!(complete_merge(&t, Some("feature/n2 브랜치 병합")).unwrap().ok);
    assert_eq!(read_str(&mgr, "config/new.txt"), merged);
    assert!(!merge_in_progress(&t).unwrap());
}

// ═════════════════════════════════════════════════════════════════════════════
// 3a. rename/rename — one logical conflict surfaces as THREE unmerged paths;
//     conflict_detail silently returns empty bodies for the missing stages.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn rename_rename_conflict_three_paths_and_empty_detail() {
    let body = b"line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n";
    let (bare_td, _mgr_td, mgr) = team_origin(&[("src/a.txt", body)]);
    let bare = bare_path(&bare_td);

    let (_td1, r1) = clone_member(&bare, "멤버A", "a@team.x");
    member_push_branch(&r1, "feature/rename-b", "rename a→b", |r| {
        git(r, &["mv", "src/a.txt", "src/b.txt"]);
    });
    let (_td2, r2) = clone_member(&bare, "멤버B", "b@team.x");
    member_push_branch(&r2, "feature/rename-c", "rename a→c", |r| {
        git(r, &["mv", "src/a.txt", "src/c.txt"]);
    });

    git(&mgr, &["fetch", "-q", "--prune", "origin"]);
    let t = target(&mgr);
    assert!(start_merge(&t, "origin/feature/rename-b", "main", "origin", None).unwrap().ok);
    assert!(push(&t, Some("main"), None).unwrap().ok);

    let out = start_merge(&t, "origin/feature/rename-c", "main", "origin", None).unwrap();
    assert!(out.conflicted, "rename/rename must conflict");

    // One logical conflict, but the app reports THREE separate paths — the
    // old name (stage 1 only) plus both new names (stage 2 / stage 3 only).
    let mut rem = remaining_conflicts(&t).unwrap();
    rem.sort();
    assert_eq!(
        rem,
        vec![
            "src/a.txt".to_string(),
            "src/b.txt".to_string(),
            "src/c.txt".to_string()
        ],
        "rename/rename lists all three paths as unmerged"
    );

    // 문서화된 동작(BUG-4, 의도적으로 유지): conflict_detail 은 빠진 스테이지를
    // 빈 문자열로, 빠진 base 를 None 으로 돌려준다. 그래서 src/b.txt 는
    // "theirs 가 빈 add/add"처럼 보이지만, 해결은 아래의 stage-missing →
    // `git rm` 폴백으로 세 경로 모두 막힘 없이 진행된다.
    let db = conflict_detail(&t, "src/b.txt").unwrap();
    assert_eq!(db.ours, String::from_utf8_lossy(body), "stage2 present");
    assert_eq!(db.theirs, "", "stage3 missing → 빈 문자열 (문서화된 동작)");
    assert!(db.base.is_none(), "stage1 lives under src/a.txt → None here (문서화된 동작)");
    assert_eq!(db.working, String::from_utf8_lossy(body), "worktree has b.txt");

    let da = conflict_detail(&t, "src/a.txt").unwrap();
    assert_eq!(da.ours, "", "old path has no stage2 → 빈 문자열 (문서화된 동작)");
    assert_eq!(da.theirs, "", "old path has no stage3 → 빈 문자열 (문서화된 동작)");
    assert_eq!(
        da.base.as_deref(),
        Some(String::from_utf8_lossy(body).as_ref()),
        "base stage does exist — under the OLD path only"
    );
    assert_eq!(da.working, "", "no working copy for the old name → empty editor");

    // Resolution is still possible (no dead-end): keep the manager-side
    // rename. Ours on b.txt keeps it; Ours on a.txt/c.txt hits the
    // stage-missing fallback (same hole as modify/delete) and removes them.
    let rem = resolve_conflict(&t, "src/b.txt", &Resolution::Ours).unwrap();
    assert_eq!(rem.len(), 2);
    let rem = resolve_conflict(&t, "src/a.txt", &Resolution::Ours).unwrap();
    assert_eq!(rem.len(), 1);
    let rem = resolve_conflict(&t, "src/c.txt", &Resolution::Ours).unwrap();
    assert!(rem.is_empty(), "all three paths resolvable: {rem:?}");
    assert!(complete_merge(&t, Some("feature/rename-c 브랜치 병합")).unwrap().ok);

    assert!(mgr.join("src/b.txt").exists());
    assert!(!mgr.join("src/a.txt").exists());
    assert!(!mgr.join("src/c.txt").exists(), "losing rename removed");
    assert_eq!(
        git_stdout(&mgr, &["ls-tree", "-r", "--name-only", "HEAD"]),
        "src/b.txt"
    );
}

// ═════════════════════════════════════════════════════════════════════════════
// 3b. rename/modify — rename detection folds the conflict onto the NEW path.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn rename_modify_conflict_lands_on_renamed_path() {
    let body = b"line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n";
    let (bare_td, _mgr_td, mgr) = team_origin(&[("src/a.txt", body)]);
    let bare = bare_path(&bare_td);

    let (_td1, r1) = clone_member(&bare, "멤버A", "a@team.x");
    member_push_branch(&r1, "feature/ren-mod", "rename+modify", |r| {
        git(r, &["mv", "src/a.txt", "src/b.txt"]);
        let s = read_str(r, "src/b.txt").replace("line5\n", "line5-A\n");
        write_file(r, "src/b.txt", s.as_bytes());
    });
    let (_td2, r2) = clone_member(&bare, "멤버B", "b@team.x");
    member_push_branch(&r2, "feature/edit", "modify same line", |r| {
        let s = read_str(r, "src/a.txt").replace("line5\n", "line5-B\n");
        write_file(r, "src/a.txt", s.as_bytes());
    });

    git(&mgr, &["fetch", "-q", "--prune", "origin"]);
    let t = target(&mgr);
    assert!(start_merge(&t, "origin/feature/ren-mod", "main", "origin", None).unwrap().ok);
    assert!(push(&t, Some("main"), None).unwrap().ok);

    let out = start_merge(&t, "origin/feature/edit", "main", "origin", None).unwrap();
    assert!(out.conflicted);
    // Unlike rename/rename, this is ONE unmerged path — the renamed one.
    assert_eq!(remaining_conflicts(&t).unwrap(), vec!["src/b.txt".to_string()]);

    let d = conflict_detail(&t, "src/b.txt").unwrap();
    assert_eq!(
        d.base.as_deref(),
        Some(String::from_utf8_lossy(body).as_ref()),
        "base carries the pre-rename body under the new path"
    );
    assert!(d.ours.contains("line5-A"));
    assert!(d.theirs.contains("line5-B"));
    assert!(d.working.contains("<<<<<<< "), "markers rendered in the new path");

    let merged = String::from_utf8_lossy(body).replace("line5\n", "line5-AB\n");
    let rem = resolve_conflict(
        &t,
        "src/b.txt",
        &Resolution::Manual {
            content: merged.clone(),
        },
    )
    .unwrap();
    assert!(rem.is_empty());
    assert!(complete_merge(&t, Some("feature/edit 브랜치 병합")).unwrap().ok);
    assert_eq!(read_str(&mgr, "src/b.txt"), merged);
    assert!(!mgr.join("src/a.txt").exists(), "old name stays gone");
}

// ═════════════════════════════════════════════════════════════════════════════
// 4a. CRLF — markers themselves come out with \r\n line endings.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn crlf_conflict_markers_carry_cr_and_manual_resolution_keeps_crlf() {
    let base = b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\n";
    let (bare_td, _mgr_td, mgr) = team_origin(&[("win.txt", base)]);
    let bare = bare_path(&bare_td);

    let (_td1, r1) = clone_member(&bare, "멤버W", "w@team.x");
    member_push_branch(&r1, "feature/win", "edit line3 (crlf)", |r| {
        write_file(r, "win.txt", b"one\r\ntwo\r\nthree-member\r\nfour\r\nfive\r\n");
    });
    // Manager edits the same line on main.
    write_file(&mgr, "win.txt", b"one\r\ntwo\r\nthree-manager\r\nfour\r\nfive\r\n");
    git(&mgr, &["commit", "-q", "-am", "manager edit"]);
    git(&mgr, &["push", "-q", "origin", "main"]);
    git(&mgr, &["fetch", "-q", "origin"]);

    let t = target(&mgr);
    let out = start_merge(&t, "origin/feature/win", "main", "origin", None).unwrap();
    assert!(out.conflicted);

    // 회귀 노트(BUG-3): git 은 CRLF 파일에서 충돌 마커 줄 자체를 \r\n 으로
    // 쓴다 (git 2.43 확인). conflictParser.ts 가 이제 마커 끝의 \r 를
    // 허용하므로(conflictParser.edge.test.ts 에서 검증), 이 테스트는 파서가
    // 감당해야 하는 마커 형태를 고정하는 역할만 한다.
    let working = read(&mgr, "win.txt");
    let ws = String::from_utf8_lossy(&working);
    assert!(
        ws.contains("<<<<<<< HEAD\r\n"),
        "start marker ends with CRLF: {ws:?}"
    );
    assert!(ws.contains("=======\r\n"), "mid marker ends with CRLF (마커 형태 고정)");
    assert!(ws.contains(">>>>>>> "), "end marker present");

    let d = conflict_detail(&t, "win.txt").unwrap();
    assert!(!d.is_binary, "CRLF text must not be mistaken for binary");
    assert!(d.ours.contains("three-manager\r\n"), "ours keeps CRLF bodies");
    assert!(d.theirs.contains("three-member\r\n"), "theirs keeps CRLF bodies");

    // Manual resolution writes bytes verbatim — CRLF preserved end to end.
    let merged = "one\r\ntwo\r\nthree-merged\r\nfour\r\nfive\r\n";
    let rem = resolve_conflict(
        &t,
        "win.txt",
        &Resolution::Manual {
            content: merged.into(),
        },
    )
    .unwrap();
    assert!(rem.is_empty());
    assert!(complete_merge(&t, Some("feature/win 브랜치 병합")).unwrap().ok);
    assert_eq!(read(&mgr, "win.txt"), merged.as_bytes(), "byte-exact CRLF result");
    let committed = git(&mgr, &["show", "HEAD:win.txt"]);
    assert_eq!(committed.stdout, merged.as_bytes(), "committed blob byte-exact");
}

// ═════════════════════════════════════════════════════════════════════════════
// 4b. Legitimate "=======" content — a perfect AI merge with a setext
//     underline is accepted and auto-resolved end to end.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn legit_setext_heading_is_ai_auto_resolved() {
    let _guard = ENV_LOCK.lock().unwrap();
    let base = b"Title\n=======\n\n- item base\n";
    let (bare_td, _mgr_td, mgr) = team_origin(&[("doc.md", base)]);
    let bare = bare_path(&bare_td);
    std::env::set_var("GC_BACKUP_DIR", bare_td.path().join("backups"));

    let (_td1, r1) = clone_member(&bare, "멤버D", "d@team.x");
    member_push_branch(&r1, "feature/doc", "edit item", |r| {
        write_file(r, "doc.md", b"Title\n=======\n\n- item member\n");
    });
    write_file(&mgr, "doc.md", b"Title\n=======\n\n- item manager\n");
    git(&mgr, &["commit", "-q", "-am", "manager edit"]);
    git(&mgr, &["push", "-q", "origin", "main"]);

    let t = target(&mgr);
    let out = start_merge(&t, "origin/feature/doc", "main", "origin", None).unwrap();
    assert!(out.conflicted);

    // The AI returns a PERFECT merged body. It contains the document's
    // legitimate setext underline "=======".
    let ai_body = "Title\n=======\n\n- item manager+member\n";
    let report = auto_resolve_merge(&t, &AutoResolveOptions::default(), |_| {
        Ok(ai_body.to_string())
    })
    .unwrap();

    // 회귀 노트(BUG-6 수정): valid_ai_body 가 줄 단위(column-0 마커)로만
    // 판정하므로, 정당한 setext 밑줄 "=======" 이 든 완벽한 AI 결과가 더는
    // 버려지지 않는다 — 파일은 method "ai" 로 해결되고 병합까지 커밋된다.
    assert_eq!(
        report.resolved.len(),
        1,
        "AI 결과가 채택된다: remaining={:?}",
        report.remaining_reasons
    );
    assert_eq!(report.resolved[0].path, "doc.md");
    assert_eq!(report.resolved[0].method, "ai", "setext 본문도 method=ai 로 해결");
    assert!(report.remaining.is_empty(), "남는 충돌 없음: {:?}", report.remaining);
    assert!(report.committed, "모든 충돌 해결 → 병합 커밋까지 완료");
    assert!(!merge_in_progress(&t).unwrap());

    // AI 본문이 setext 밑줄 그대로 워킹 트리와 HEAD 에 반영된다. (AI 경로는
    // 코드펜스 정리(strip_code_fence)가 끝 공백을 다듬으므로 끝 개행만 빠진다.)
    let expected = ai_body.trim_end();
    assert_eq!(read_str(&mgr, "doc.md"), expected);
    let committed = git(&mgr, &["show", "HEAD:doc.md"]);
    assert_eq!(committed.stdout, expected.as_bytes(), "committed blob byte-exact");
}

// ═════════════════════════════════════════════════════════════════════════════
// 4c. Manual resolution REJECTS leftover conflict markers.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn manual_resolution_with_leftover_markers_is_rejected() {
    let (bare_td, _mgr_td, mgr) = team_origin(&[("f.txt", b"base\n")]);
    let bare = bare_path(&bare_td);
    let (_td1, r1) = clone_member(&bare, "멤버F", "f@team.x");
    member_push_branch(&r1, "feature/f", "edit", |r| {
        write_file(r, "f.txt", b"member\n");
    });
    write_file(&mgr, "f.txt", b"manager\n");
    git(&mgr, &["commit", "-q", "-am", "manager edit"]);
    git(&mgr, &["push", "-q", "origin", "main"]);

    let t = target(&mgr);
    assert!(start_merge(&t, "origin/feature/f", "main", "origin", None).unwrap().conflicted);

    // 회귀 노트(BUG-5 수정): resolve_conflict(Manual)은 줄 첫머리의
    // `<<<<<<< ` / `>>>>>>> ` / `|||||||` 마커가 남은 본문을 거부한다
    // ("충돌 표시" 에러). 거부 시 아무것도 쓰거나 스테이징하지 않으므로
    // 마커가 팀 전체에 커밋될 길이 막혔다. `=======` 단독 줄은 정당한
    // 내용(setext 밑줄 등)일 수 있어 여전히 허용된다.
    let still_marked = "<<<<<<< HEAD\nmanager\n=======\nmember\n>>>>>>> origin/feature/f\n";
    let before = read_str(&mgr, "f.txt");
    let err = resolve_conflict(
        &t,
        "f.txt",
        &Resolution::Manual {
            content: still_marked.into(),
        },
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("충돌 표시"),
        "거부 사유를 한국어로 안내: {err}"
    );
    assert_eq!(read_str(&mgr, "f.txt"), before, "거부 시 워킹 트리는 그대로");
    assert_eq!(
        remaining_conflicts(&t).unwrap(),
        vec!["f.txt".to_string()],
        "거부 시 스테이징도 안 됨 — 여전히 미해결"
    );
    assert!(merge_in_progress(&t).unwrap(), "병합은 계속 진행 중");

    // `=======` 단독 줄이 든 깨끗한 본문은 통과한다.
    let merged = "manager+member\n=======\n(구분선은 정당한 내용)\n";
    let rem = resolve_conflict(
        &t,
        "f.txt",
        &Resolution::Manual {
            content: merged.into(),
        },
    )
    .unwrap();
    assert!(rem.is_empty());
    let done = complete_merge(&t, Some("feature/f 브랜치 병합")).unwrap();
    assert!(done.ok);
    assert_eq!(read_str(&mgr, "f.txt"), merged, "마커 없는 본문만 커밋된다");
}

// ═════════════════════════════════════════════════════════════════════════════
// 5. `.gpconfig` edited on two branches.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn gpconfig_conflict_effective_read_falls_back_to_committed_copy() {
    let (bare_td, _mgr_td, mgr) = team_origin(&[("README.md", b"team\n")]);
    let bare = bare_path(&bare_td);
    let t = target(&mgr);

    // Seed a minimal config on main (empty members ⇒ the members/managers
    // lines are adjacent in the pretty JSON, so the two edits below conflict).
    let mut cfg = ProjectConfig::default();
    cfg.default_base_branch = "main".into();
    gpconfig::save_config(&t, &cfg).unwrap();
    assert!(gpconfig::commit_config(&t).unwrap().ok);
    git(&mgr, &["push", "-q", "origin", "main"]);

    // Member: adds themselves to `members` on their branch.
    let (_td1, r1) = clone_member(&bare, "김철수", "kim@team.x");
    let rt = target(&r1);
    git(&r1, &["checkout", "-q", "-b", "feature/cfg"]);
    let (mut mcfg, exists) = gpconfig::read_config(&rt).unwrap();
    assert!(exists);
    mcfg.members
        .push(member_from_account("2", "김철수", "kim@team.x", "member"));
    gpconfig::save_config(&rt, &mcfg).unwrap();
    assert!(gpconfig::commit_config(&rt).unwrap().ok);
    git(&r1, &["push", "-q", "-u", "origin", "feature/cfg"]);

    // Manager: changes merge_managers (and the member list it depends on) on main.
    let (mut gcfg, _) = gpconfig::read_config(&t).unwrap();
    gcfg.members
        .push(member_from_account("1", "리드", "lead@team.x", "admin"));
    gcfg.merge_managers
        .insert("main".into(), "lead@team.x".into());
    gpconfig::save_config(&t, &gcfg).unwrap();
    assert!(gpconfig::commit_config(&t).unwrap().ok);

    git(&mgr, &["fetch", "-q", "origin"]);
    let out = start_merge(&t, "origin/feature/cfg", "main", "origin", None).unwrap();
    assert!(out.conflicted, ".gpconfig edits must conflict: {}", out.message);
    assert_eq!(out.conflicted_files, vec![".gpconfig".to_string()]);

    // 회귀 노트(BUG-2 수정): 충돌 중 워킹 트리 .gpconfig 는 마커 때문에 파싱
    // 불가 → read_config 는 여전히 Err 이지만, read_config_effective 는 이를
    // "없음"과 똑같이 취급하고 커밋본(origin/<base> 우선)으로 폴백한다 —
    // 관리자가 병합을 처리하는 동안에도 팀 규칙 조회가 살아 있다.
    assert!(
        gpconfig::read_config(&t).is_err(),
        "워킹 트리 사본은 여전히 파싱 불가 → Err"
    );
    let (mid, mid_exists) = gpconfig::read_config_effective(&t, "main", "origin").unwrap();
    assert!(mid_exists, "커밋본 폴백 → exists=true (BUG-2 수정)");
    // origin/main 에 커밋된 사본(멤버 추가 전의 최소 설정)이 돌아온다 — 아직
    // 푸시되지 않은 로컬 편집이 아니라 팀이 공유하는 규칙이다.
    assert_eq!(mid.default_base_branch, "main");
    assert!(mid.members.is_empty(), "origin/main 사본에는 아직 멤버가 없다");
    assert!(mid.merge_managers.is_empty(), "관리자 지정도 아직 커밋 전");

    // Resolve manually with the union of both edits, in the app's own format.
    let mut merged = ProjectConfig::default();
    merged.default_base_branch = "main".into();
    merged
        .members
        .push(member_from_account("1", "리드", "lead@team.x", "admin"));
    merged
        .members
        .push(member_from_account("2", "김철수", "kim@team.x", "member"));
    merged
        .merge_managers
        .insert("main".into(), "lead@team.x".into());
    let body = serde_json::to_string_pretty(&merged).unwrap();
    let rem = resolve_conflict(&t, ".gpconfig", &Resolution::Manual { content: body }).unwrap();
    assert!(rem.is_empty());
    assert!(complete_merge(&t, Some("feature/cfg 브랜치 병합")).unwrap().ok);

    // Post-merge: valid JSON in HEAD, both edits present, format intact.
    let head_cfg = git_stdout(&mgr, &["show", "HEAD:.gpconfig"]);
    let parsed: serde_json::Value = serde_json::from_str(&head_cfg).expect("valid JSON committed");
    assert!(parsed.get("gpconfig_version").is_some());

    let (back, exists) = gpconfig::read_config(&t).unwrap();
    assert!(exists);
    assert_eq!(back.members.len(), 2, "both member edits survive");
    assert_eq!(
        back.merge_managers.get("main").map(String::as_str),
        Some("lead@team.x"),
        "manager assignment survives"
    );
    let (eff, eff_exists) = gpconfig::read_config_effective(&t, "main", "origin").unwrap();
    assert!(eff_exists);
    assert_eq!(eff.members.len(), 2);
}

// ═════════════════════════════════════════════════════════════════════════════
// 6. Auto-resolve at scale — 4 conflicted files, AI failing.
// ═════════════════════════════════════════════════════════════════════════════

/// Rewrite stage 2 of `path` to the base blob — simulates the "one side is
/// byte-identical to base yet git still conflicts" shape (line-ending / mode /
/// rename cases; see auto.rs::one_sided_change). Raw git, setup only.
fn set_stage2_to_base(repo: &Path, path: &str) {
    let out = git(repo, &["ls-files", "-u", "--", path]);
    let text = String::from_utf8_lossy(&out.stdout);
    let base_sha = text
        .lines()
        .find(|l| l.split_whitespace().nth(2) == Some("1"))
        .expect("stage 1 entry")
        .split_whitespace()
        .nth(1)
        .unwrap()
        .to_string();
    let mut child = Command::new("git")
        .current_dir(repo)
        .args(["update-index", "--index-info"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("100644 {base_sha} 2\t{path}\n").as_bytes())
        .unwrap();
    assert!(child.wait().unwrap().success());
}

#[test]
fn auto_resolve_scale_dispositions_backup_and_restore() {
    let _guard = ENV_LOCK.lock().unwrap();

    // Build a >1MiB text body (each line ~22 bytes × 60k ≈ 1.3 MiB).
    let mut huge_base = String::with_capacity(1_400_000);
    huge_base.push_str("hdr-base\n");
    for i in 0..60_000u32 {
        huge_base.push_str(&format!("abcdefghijklm {i:07}\n"));
    }

    let (bare_td, _mgr_td, mgr) = team_origin(&[
        ("one_sided.txt", b"keep-1\nkeep-2\n".as_slice()),
        ("both.txt", b"shared\n".as_slice()),
        ("logo.bin", b"\x00LOGO base\x00".as_slice()),
        ("huge.txt", huge_base.as_bytes()),
    ]);
    let backups_dir = bare_td.path().join("backups");
    std::env::set_var("GC_BACKUP_DIR", &backups_dir);

    // Member branch changes all four.
    let (_td1, r1) = clone_member(&bare_path(&bare_td), "멤버X", "x@team.x");
    member_push_branch(&r1, "feature/x", "member edits", |r| {
        write_file(r, "one_sided.txt", b"feature-line\nkeep-2\n");
        write_file(r, "both.txt", b"feature change\n");
        write_file(r, "logo.bin", b"\x00LOGO feature\x00");
        let huge_feature = huge_base.replacen("hdr-base", "hdr-feature", 1);
        write_file(r, "huge.txt", huge_feature.as_bytes());
    });

    // Manager edits the same four on main.
    write_file(&mgr, "one_sided.txt", b"main-line\nkeep-2\n");
    write_file(&mgr, "both.txt", b"main change\n");
    write_file(&mgr, "logo.bin", b"\x00LOGO main\x00");
    let huge_main = huge_base.replacen("hdr-base", "hdr-main", 1);
    write_file(&mgr, "huge.txt", huge_main.as_bytes());
    git(&mgr, &["commit", "-q", "-am", "manager edits"]);
    git(&mgr, &["push", "-q", "origin", "main"]);

    let t = target(&mgr);
    let out = start_merge(&t, "origin/feature/x", "main", "origin", None).unwrap();
    assert!(out.conflicted);
    let mut files = remaining_conflicts(&t).unwrap();
    files.sort();
    assert_eq!(
        files,
        vec!["both.txt", "huge.txt", "logo.bin", "one_sided.txt"],
        "all four files conflicted"
    );

    // Turn one_sided.txt into a "ours == base" conflict (the shape git leaves
    // for line-ending/mode/rename-noise conflicts).
    set_stage2_to_base(&mgr, "one_sided.txt");

    // Sanity-check the classification inputs.
    let d = conflict_detail(&t, "one_sided.txt").unwrap();
    assert_eq!(d.base.as_deref(), Some(d.ours.as_str()), "ours == base");
    assert!(conflict_detail(&t, "logo.bin").unwrap().is_binary);
    let dh = conflict_detail(&t, "huge.txt").unwrap();
    assert!(dh.too_large, ">1MiB stages must be flagged too_large");
    assert!(dh.ours.is_empty() && dh.theirs.is_empty(), "large bodies not embedded");

    // Snapshot every pre-resolution working body (byte-exact reference).
    let snap: Vec<(String, Vec<u8>)> = files
        .iter()
        .map(|f| (f.clone(), read(&mgr, f)))
        .collect();

    // AI fails on every file; options are the SAFE defaults
    // (binary/huge → theirs, both-sided text → leave for a human).
    let report = auto_resolve_merge(&t, &AutoResolveOptions::default(), |_| {
        Err(git_companion::error::AppError::Config("AI down".into()))
    })
    .unwrap();

    // Dispositions per the documented safety rules.
    let method_of = |p: &str| {
        report
            .resolved
            .iter()
            .find(|r| r.path == p)
            .map(|r| r.method.clone())
    };
    assert_eq!(
        method_of("one_sided.txt").as_deref(),
        Some("theirs"),
        "one-sided (ours==base) → the changed side is the correct merge"
    );
    assert_eq!(method_of("logo.bin").as_deref(), Some("theirs"), "binary → side strategy");
    assert_eq!(method_of("huge.txt").as_deref(), Some("theirs"), "huge → side strategy");
    assert_eq!(
        report.remaining,
        vec!["both.txt".to_string()],
        "both-sided text is left for a human"
    );
    assert_eq!(report.remaining_reasons.len(), 1);
    assert_eq!(report.remaining_reasons[0].path, "both.txt");
    assert!(
        report.remaining_reasons[0]
            .note
            .as_deref()
            .unwrap_or("")
            .contains("양쪽"),
        "reason explains both sides changed: {:?}",
        report.remaining_reasons[0].note
    );
    assert!(!report.committed, "must not commit while a conflict remains");
    assert!(merge_in_progress(&t).unwrap(), "MERGE_HEAD kept for the merge center");

    // Resolved contents took the branch side.
    assert_eq!(read(&mgr, "one_sided.txt"), b"feature-line\nkeep-2\n");
    assert_eq!(read(&mgr, "logo.bin"), b"\x00LOGO feature\x00");
    assert!(read_str(&mgr, "huge.txt").starts_with("hdr-feature\n"));

    // Backup contains ALL FOUR pre-resolution bodies, byte-exact.
    let backup_id = report.backup_id.clone().expect("backup always created");
    let backups = list_backups(&t).unwrap();
    assert_eq!(backups.len(), 1);
    assert_eq!(backups[0].id, backup_id);
    let mut listed = backups[0].files.clone();
    listed.sort();
    assert_eq!(listed, files, "every conflicted file backed up");
    let backup_dir = backups_dir.join(backup_slug(&mgr)).join(&backup_id);
    for (f, bytes) in &snap {
        assert_eq!(
            &std::fs::read(backup_dir.join(f)).unwrap(),
            bytes,
            "backup of {f} must be byte-exact"
        );
    }

    // restore_backup brings every file back byte-exact.
    let n = restore_backup(&t, &backup_id).unwrap();
    assert_eq!(n, 4);
    for (f, bytes) in &snap {
        assert_eq!(&read(&mgr, f), bytes, "restored {f} must be byte-exact");
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// 7. Conflict during member sync — unpushed commits must survive.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn member_sync_conflict_preserves_unpushed_commits() {
    let (bare_td, _mgr_td, mgr) = team_origin(&[("shared.txt", b"one\ntwo\nthree\n")]);
    let bare = bare_path(&bare_td);

    // Member works on a branch with TWO unpushed commits.
    let (_td1, member) = clone_member(&bare, "멤버S", "s@team.x");
    git(&member, &["checkout", "-q", "-b", "feature/me"]);
    write_file(&member, "shared.txt", b"one\ntwo-me\nthree\n");
    git(&member, &["commit", "-q", "-am", "my shared edit"]);
    write_file(&member, "notes/me.txt", b"private notes\n");
    git(&member, &["add", "-A"]);
    git(&member, &["commit", "-q", "-m", "my notes"]);
    let pre_sync_head = git_stdout(&member, &["rev-parse", "HEAD"]);

    // Meanwhile the manager lands a conflicting edit on main.
    write_file(&mgr, "shared.txt", b"one\ntwo-boss\nthree\n");
    git(&mgr, &["commit", "-q", "-am", "boss edit"]);
    git(&mgr, &["push", "-q", "origin", "main"]);

    // Member syncs their branch to base — same resolve/complete path, in the
    // MEMBER's clone.
    let mt = target(&member);
    let sync = sync_to_base(&mt, "main", "origin").unwrap();
    assert!(sync.conflicted, "sync must conflict: {}", sync.message);
    assert_eq!(sync.files, vec!["shared.txt".to_string()]);
    assert!(merge_in_progress(&mt).unwrap());

    let d = conflict_detail(&mt, "shared.txt").unwrap();
    assert!(d.ours.contains("two-me"), "ours = the member's branch");
    assert!(d.theirs.contains("two-boss"), "theirs = freshly merged main");

    let merged = "one\ntwo-me+boss\nthree\n";
    let rem = resolve_conflict(
        &mt,
        "shared.txt",
        &Resolution::Manual {
            content: merged.into(),
        },
    )
    .unwrap();
    assert!(rem.is_empty());
    // None → git's prepared MERGE_MSG is used.
    assert!(complete_merge(&mt, None).unwrap().ok);
    assert!(!merge_in_progress(&mt).unwrap());

    // The member's own unpushed commits survive the sync.
    let anc = git_try(&member, &["merge-base", "--is-ancestor", &pre_sync_head, "HEAD"]);
    assert!(anc.status.success(), "pre-sync HEAD must remain an ancestor");
    assert!(member.join("notes/me.txt").exists());
    assert_eq!(read_str(&member, "shared.txt"), merged);
    // origin/main..HEAD = the two own commits + the sync merge commit.
    assert_eq!(
        git_stdout(&member, &["rev-list", "--count", "origin/main..HEAD"]),
        "3"
    );
    let parents = git_stdout(&member, &["log", "-1", "--pretty=%P"]);
    assert_eq!(parents.split_whitespace().count(), 2, "sync produced a merge commit");
}

// ═════════════════════════════════════════════════════════════════════════════
// 8. Mixed per-block methods in ONE file (ours/theirs/manual via Manual path).
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn mixed_methods_across_six_blocks_in_one_file() {
    // 6 sections, each: 1 contested line + 4 stable pad lines (enough context
    // that git keeps 6 separate conflict hunks).
    let mut base = String::new();
    for i in 1..=6 {
        base.push_str(&format!(
            "s{i} value base\npad{i}-1\npad{i}-2\npad{i}-3\npad{i}-4\n"
        ));
    }
    let (bare_td, _mgr_td, mgr) = team_origin(&[("mixed.txt", base.as_bytes())]);
    let bare = bare_path(&bare_td);

    let (_td1, r1) = clone_member(&bare, "멤버M", "mm@team.x");
    let theirs_body = base.replace("value base", "value theirs");
    member_push_branch(&r1, "feature/mix", "member edits all sections", |r| {
        write_file(r, "mixed.txt", theirs_body.as_bytes());
    });
    let ours_body = base.replace("value base", "value ours");
    write_file(&mgr, "mixed.txt", ours_body.as_bytes());
    git(&mgr, &["commit", "-q", "-am", "manager edits all sections"]);
    git(&mgr, &["push", "-q", "origin", "main"]);

    let t = target(&mgr);
    let out = start_merge(&t, "origin/feature/mix", "main", "origin", None).unwrap();
    assert!(out.conflicted);

    // Parse the working copy exactly the way the block editor (conflictParser
    // .ts) does, then resolve block-by-block with mixed methods.
    let working = read_str(&mgr, "mixed.txt");
    let blocks = parse_blocks(&working);
    assert_eq!(blocks.len(), 6, "six independent conflict blocks expected");
    for (idx, b) in blocks.iter().enumerate() {
        let i = idx + 1;
        assert_eq!(b.ours, format!("s{i} value ours"), "block {i} ours body");
        assert_eq!(b.theirs, format!("s{i} value theirs"), "block {i} theirs body");
    }

    // Blocks 1-2 → ours, 3-4 → theirs, 5-6 → manual free text.
    let replacements: Vec<String> = vec![
        blocks[0].ours.clone(),
        blocks[1].ours.clone(),
        blocks[2].theirs.clone(),
        blocks[3].theirs.clone(),
        "s5 value manual".to_string(),
        "s6 value manual".to_string(),
    ];
    let resolved_body = reassemble(&working, &blocks, &replacements);
    assert!(
        !resolved_body.contains("<<<<<<<")
            && !resolved_body.contains("\n=======\n")
            && !resolved_body.contains(">>>>>>>"),
        "no leftover markers in the reassembled body"
    );

    let rem = resolve_conflict(
        &t,
        "mixed.txt",
        &Resolution::Manual {
            content: resolved_body.clone(),
        },
    )
    .unwrap();
    assert!(rem.is_empty(), "single Manual write clears all six blocks");
    assert!(complete_merge(&t, Some("feature/mix 브랜치 병합")).unwrap().ok);
    assert!(!merge_in_progress(&t).unwrap());

    // What the app wrote is exactly what git expects: chosen line per section,
    // pads untouched, trailing newline preserved.
    let mut expected = String::new();
    let choice = ["ours", "ours", "theirs", "theirs", "manual", "manual"];
    for i in 1..=6usize {
        expected.push_str(&format!(
            "s{i} value {}\npad{i}-1\npad{i}-2\npad{i}-3\npad{i}-4\n",
            choice[i - 1]
        ));
    }
    assert_eq!(read_str(&mgr, "mixed.txt"), expected, "byte-exact final body");
    let committed = git(&mgr, &["show", "HEAD:mixed.txt"]);
    assert_eq!(committed.stdout, expected.as_bytes());
    // Index is fully staged and clean.
    let status = git_stdout(&mgr, &["status", "--porcelain"]);
    assert!(status.is_empty(), "clean tree after merge commit: {status}");
}
