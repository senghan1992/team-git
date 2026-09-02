//! Merge engine for the in-app merge center workflow.
//!
//! All public functions take `&Target` so the same code path covers both local
//! repositories and SSH targets. The merger is intentionally small — it leans
//! on the same primitives (`run_at_target`, `write_file_at_target`) that the
//! pull path already uses.
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::git::{run_at_target, GitOutput, Target};

/// One changed file in a pending branch. `kind` is the single character
/// `git diff --name-status` emits (A/M/D/R/C/U) — kept as a string so the
/// frontend can colour-code without us re-mapping on the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangedPath {
    pub path: String,
    pub kind: String,
}

/// A branch that still needs to be merged into the base — either a remote tip
/// (a teammate's pushed work) or a local branch (own unpushed work).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingBranch {
    pub name: String,
    pub short_name: String,
    pub sha: String,
    pub author: String,
    pub unix_time: i64,
    pub subject: String,
    pub ahead: u32,
    pub behind: u32,
    pub changed_files: Vec<ChangedPath>,
    /// True when the branch only exists locally (never pushed).
    pub local: bool,
    /// True when the branch is already merged into the *local* base but the
    /// base itself has not been pushed yet. The UI must not offer "병합"
    /// again for such a branch — the missing step is the push.
    pub merged_locally: bool,
}

/// Outcome of any merge step. When `conflicted == true`, `MERGE_HEAD` is left
/// in place so the conflict-resolution UI can pick up where this left off.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeOutcome {
    pub ok: bool,
    pub conflicted: bool,
    pub conflicted_files: Vec<String>,
    pub message: String,
}

/// One stage of a three-way conflict.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictDetail {
    pub path: String,
    /// True when `:2:`/`:3:` are not valid UTF-8 text.
    pub is_binary: bool,
    /// True when the file is over 1 MiB and we did not embed the bodies.
    pub too_large: bool,
    /// `None` for add/add conflicts where the base stage doesn't exist.
    pub base: Option<String>,
    pub ours: String,
    pub theirs: String,
    /// Current contents of the working copy — may still carry `<<<<<<<`
    /// markers if the user has been hand-editing.
    pub working: String,
}

/// User's resolution for a single conflicted file. The on-the-wire shape
/// matches the variant tags defined on `Resolution` in `commands::git`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Resolution {
    Ours,
    Theirs,
    Manual { content: String },
}

const MAX_TEXT_BYTES: usize = 1024 * 1024;

/// Enumerate branches that still need to land on `<base>`: remote tips first
/// (teammates' pushed work), then local-only branches (own unpushed work).
/// Already-merged branches (ancestor of the base) and HEAD pointers are
/// excluded; a local branch identical to an already-listed remote tip is
/// deduplicated.
pub fn list_pending_branches(
    target: &Target,
    remote: &str,
    base: &str,
) -> AppResult<Vec<PendingBranch>> {
    let fmt =
        "%(refname:short)%09%(objectname)%09%(authorname)%09%(committerdate:unix)%09%(subject)";
    let list = run_at_target(
        target,
        [
            "for-each-ref",
            &format!("refs/remotes/{remote}"),
            "--format",
            fmt,
        ],
    )?;
    if !list.ok() {
        return Err(AppError::Git(format!(
            "for-each-ref failed: {}",
            list.stderr.trim()
        )));
    }

    let head_ref = format!("{remote}/HEAD");
    let base_ref = format!("{remote}/{base}");
    let mut out = Vec::new();
    // Sha of every tip already listed — local branches that point at the same
    // commit as a remote tip are the same branch, shown once.
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for line in list.stdout.lines() {
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(5, '\t');
        let name = parts.next().unwrap_or("").to_string();
        let sha = parts.next().unwrap_or("").to_string();
        let author = parts.next().unwrap_or("").to_string();
        let unix_str = parts.next().unwrap_or("0");
        let subject = parts.next().unwrap_or("").to_string();
        if name == head_ref || name == base_ref || name.is_empty() || sha.is_empty() {
            continue;
        }
        if name
            .rsplit('/')
            .next()
            .map(|s| s == "HEAD")
            .unwrap_or(false)
        {
            continue;
        }
        if let Some(b) = build_pending(
            target, &name, &sha, &author, &unix_str, &subject, remote, &base_ref, false,
        )? {
            seen.insert(b.sha.clone());
            out.push(b);
        }
    }

    // Local branches: skip the base itself, anything already inside the base,
    // and tips we already listed from the remote side.
    let locals = run_at_target(target, ["for-each-ref", "refs/heads", "--format", fmt])?;
    for line in locals.stdout.lines() {
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(5, '\t');
        let name = parts.next().unwrap_or("").to_string();
        let sha = parts.next().unwrap_or("").to_string();
        let author = parts.next().unwrap_or("").to_string();
        let unix_str = parts.next().unwrap_or("0");
        let subject = parts.next().unwrap_or("").to_string();
        if name == base || name == "HEAD" || name.is_empty() || sha.is_empty() {
            continue;
        }
        if seen.contains(&sha) {
            continue;
        }
        if let Some(b) = build_pending(
            target, &name, &sha, &author, &unix_str, &subject, remote, &base_ref, true,
        )? {
            seen.insert(b.sha.clone());
            out.push(b);
        }
    }

    // Newest first by committer time.
    out.sort_by(|a, b| b.unix_time.cmp(&a.unix_time));
    Ok(out)
}

/// Compute the pending-branch payload for one ref, or `None` if it is already
/// contained in the base.
#[allow(clippy::too_many_arguments)]
fn build_pending(
    target: &Target,
    name: &str,
    sha: &str,
    author: &str,
    unix_str: &str,
    subject: &str,
    remote: &str,
    base_ref: &str,
    local: bool,
) -> AppResult<Option<PendingBranch>> {
    // Already merged into base?
    let ancestor = run_at_target(target, ["merge-base", "--is-ancestor", name, base_ref])?;
    if ancestor.ok() {
        return Ok(None);
    }

    // 원격 base에는 아직 없지만 *로컬* base에는 이미 병합된 브랜치 —
    // 병합 직후 push가 실패/취소된 상태다. 다시 "병합 대기"로 세우면
    // 관리자가 같은 병합을 또 하게 되므로, 플래그로 구분해 UI가
    // "푸시 대기"로 보여 주게 한다.
    // base_ref = "<remote>/<base>" — base 이름에 '/'가 들어갈 수 있으므로
    // (release/1.0 등) 접두사만 벗긴다.
    let base = base_ref
        .strip_prefix(&format!("{remote}/"))
        .unwrap_or(base_ref);
    let merged_locally = {
        let local_base = format!("refs/heads/{base}");
        let exists = run_at_target(target, ["rev-parse", "-q", "--verify", &local_base])?;
        exists.ok()
            && run_at_target(target, ["merge-base", "--is-ancestor", name, &local_base])?.ok()
    };

    let (ahead, behind) = ahead_behind(target, base_ref, name)?;

    let diff = run_at_target(
        target,
        ["diff", "--name-status", &format!("{base_ref}...{name}")],
    )?;
    let mut changed_files = Vec::new();
    if diff.ok() {
        for cl in diff.stdout.lines() {
            if cl.is_empty() {
                continue;
            }
            // Format: "<status>\t<path>" (or "R100\told\tnew" for renames).
            // We strip the second tab so renames collapse to the new path.
            let mut fields = cl.split('\t');
            let kind = fields.next().unwrap_or("").to_string();
            let path = fields.next().unwrap_or("").to_string();
            // Rename/Copy: consume the third field too and keep the new path.
            if (kind.starts_with('R') || kind.starts_with('C')) && fields.next().is_some() {
                // already have the "new" path in `path`
            }
            if !path.is_empty() {
                changed_files.push(ChangedPath { path, kind });
            }
        }
    }

    let short_name = if local {
        name.to_string()
    } else {
        name.strip_prefix(&format!("{remote}/"))
            .unwrap_or(name)
            .to_string()
    };

    let unix_time = unix_str.parse::<i64>().unwrap_or(0);

    Ok(Some(PendingBranch {
        name: name.to_string(),
        short_name,
        sha: sha.to_string(),
        author: author.to_string(),
        unix_time,
        subject: subject.to_string(),
        ahead,
        behind,
        changed_files,
        local,
        merged_locally,
    }))
}

/// 병합이 끝나 base에 완전히 포함된 원격 브랜치 — origin에 쌓인 죽은
/// feature 브랜치를 정리할 후보 목록이다.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergedRemoteBranch {
    /// 원격 트래킹 이름 (예: "origin/feature/login").
    pub name: String,
    /// 브랜치 이름 (예: "feature/login") — 삭제 시 이 이름을 쓴다.
    pub short_name: String,
    pub author: String,
    pub unix_time: i64,
}

/// `<remote>/<base>`의 조상이 된(=병합이 끝난) 원격 브랜치를 나열한다.
/// base 자체와 HEAD 포인터는 제외. 커밋이 전혀 없는 브랜치(base와 동일
/// 커밋을 가리키는 방금 만든 브랜치)도 조상이므로 함께 나온다 — 그것도
/// "정리해도 잃는 것이 없는" 브랜치라는 뜻이라 의도된 동작이다.
pub fn list_merged_remote_branches(
    target: &Target,
    remote: &str,
    base: &str,
) -> AppResult<Vec<MergedRemoteBranch>> {
    let base_ref = format!("{remote}/{base}");
    let fmt = "%(refname:short)%09%(objectname)%09%(authorname)%09%(committerdate:unix)";
    let list = run_at_target(
        target,
        [
            "for-each-ref",
            &format!("refs/remotes/{remote}"),
            "--format",
            fmt,
        ],
    )?;
    if !list.ok() {
        return Err(AppError::Git(format!(
            "for-each-ref failed: {}",
            list.stderr.trim()
        )));
    }
    let mut out = Vec::new();
    for line in list.stdout.lines() {
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(4, '\t');
        let name = parts.next().unwrap_or("").to_string();
        let sha = parts.next().unwrap_or("");
        let author = parts.next().unwrap_or("").to_string();
        let unix_time = parts.next().unwrap_or("0").parse::<i64>().unwrap_or(0);
        if name.is_empty() || sha.is_empty() || name == base_ref {
            continue;
        }
        if name.rsplit('/').next().map(|s| s == "HEAD").unwrap_or(false) {
            continue;
        }
        let ancestor = run_at_target(target, ["merge-base", "--is-ancestor", &name, &base_ref])?;
        if !ancestor.ok() {
            continue;
        }
        let short_name = name
            .strip_prefix(&format!("{remote}/"))
            .unwrap_or(&name)
            .to_string();
        out.push(MergedRemoteBranch {
            name,
            short_name,
            author,
            unix_time,
        });
    }
    // 오래된 것부터 — 제일 먼저 정리해도 되는 것.
    out.sort_by(|a, b| a.unix_time.cmp(&b.unix_time));
    Ok(out)
}

/// 병합이 끝난 원격 브랜치를 origin에서 삭제한다 (`push <remote> --delete`).
///
/// 안전장치: base 자신은 거부하고, 브랜치가 `<remote>/<base>`의 조상인지
/// (=커밋이 전부 base에 들어갔는지) 삭제 직전에 다시 확인한다 — 목록을 본
/// 뒤 팀원이 새 커밋을 push했다면 여기서 멈춘다. 성공하면 `fetch --prune`으로
/// 로컬 트래킹 ref도 정리한다.
pub fn delete_remote_branch(
    target: &Target,
    remote: &str,
    base: &str,
    branch: &str,
) -> AppResult<()> {
    if branch == base {
        return Err(AppError::Git(format!(
            "병합 브랜치({base})는 삭제할 수 없습니다."
        )));
    }
    let branch_ref = format!("{remote}/{branch}");
    let base_ref = format!("{remote}/{base}");
    let ancestor = run_at_target(
        target,
        ["merge-base", "--is-ancestor", &branch_ref, &base_ref],
    )?;
    if !ancestor.ok() {
        return Err(AppError::Git(format!(
            "{branch} 브랜치에 아직 {base}에 없는 커밋이 있습니다 — 방금 새 push가 있었을 수 있습니다. 삭제하지 않았습니다."
        )));
    }
    let out = run_at_target(target, ["push", remote, "--delete", branch])?;
    if !out.ok() {
        return Err(AppError::Git(format!(
            "원격 브랜치 삭제 실패: {}",
            crate::git::ops::friendly_git_error(&out.stderr)
        )));
    }
    let _ = run_at_target(target, ["fetch", "--prune", remote]);
    Ok(())
}

/// How many commits the *local* base carries that `<remote>/<base>` doesn't —
/// i.e. a merge that was committed but whose push failed or was cancelled.
/// 0 when the local base doesn't exist or is fully pushed.
pub fn base_unpushed_count(target: &Target, remote: &str, base: &str) -> AppResult<u32> {
    let local_base = format!("refs/heads/{base}");
    let exists = run_at_target(target, ["rev-parse", "-q", "--verify", &local_base])?;
    if !exists.ok() {
        return Ok(0);
    }
    let remote_base = format!("refs/remotes/{remote}/{base}");
    let remote_exists = run_at_target(target, ["rev-parse", "-q", "--verify", &remote_base])?;
    if !remote_exists.ok() {
        return Ok(0);
    }
    let out = run_at_target(
        target,
        [
            "rev-list",
            "--count",
            &format!("{remote_base}..{local_base}"),
        ],
    )?;
    Ok(out.stdout.trim().parse::<u32>().unwrap_or(0))
}

fn ahead_behind(target: &Target, base: &str, other: &str) -> AppResult<(u32, u32)> {
    let out = run_at_target(
        target,
        [
            "rev-list",
            "--left-right",
            "--count",
            &format!("{base}...{other}"),
        ],
    )?;
    if !out.ok() {
        // No common ancestor yet — count as N ahead / 0 behind.
        let ahead = run_at_target(target, ["rev-list", "--count", other])?
            .stdout
            .trim()
            .parse::<u32>()
            .unwrap_or(0);
        return Ok((ahead, 0));
    }
    let mut parts = out.stdout.trim().split_whitespace();
    let behind: u32 = parts.next().unwrap_or("0").parse().unwrap_or(0);
    let ahead: u32 = parts.next().unwrap_or("0").parse().unwrap_or(0);
    Ok((ahead, behind))
}

/// Begin a merge of `branch_ref` onto `<remote>/<base>`.
///
/// 1. Refuses if the working tree has staged/unstaged changes (untracked is OK).
/// 2. Best-effort refresh of `<base>` and `--prune` fetch.
/// 3. `git checkout <base>` (DWIM if only remote).
/// 4. `git merge --no-ff -m "<short> 브렌치 병합" <branch_ref>`.
///
/// The merge commit message follows the team convention "<branch> 브렌치 병합"
/// (the same phrasing aos-git used) so merge commits read consistently across
/// the project history.
///
/// Conflict outcomes leave the repo in MERGING state for the resolution UI.
/// Other failures auto-`merge --abort` so we never leave a half-merged repo.
pub fn start_merge(
    target: &Target,
    branch_ref: &str,
    base: &str,
    remote: &str,
) -> AppResult<MergeOutcome> {
    if has_tracked_changes(target)? {
        return Err(AppError::Git(
            "작업 트리에 커밋되지 않은 변경이 있습니다. 작업 탭에서 커밋하거나 stash하세요.".into(),
        ));
    }

    // Best-effort: advance local base ref without checkout.
    let head_ref = run_at_target(target, ["rev-parse", "--abbrev-ref", "HEAD"])?;
    let current = head_ref.stdout.trim();
    let needs_base_refresh = current != base;
    if needs_base_refresh {
        let _ = run_at_target(target, ["fetch", remote, &format!("{base}:{base}")]);
    }
    let _ = run_at_target(target, ["fetch", "--prune", remote]);

    let checkout = run_at_target(target, ["checkout", base])?;
    if !checkout.ok() {
        return Err(AppError::Git(format!(
            "checkout {base} 실패: {}",
            checkout.stderr.trim()
        )));
    }

    let short = branch_ref
        .strip_prefix(&format!("{remote}/"))
        .unwrap_or(branch_ref)
        .to_string();
    let commit_msg = format!("{short} 브렌치 병합");
    let merge = run_at_target(target, ["merge", "--no-ff", "-m", &commit_msg, branch_ref])?;
    if merge.ok() {
        return Ok(MergeOutcome {
            ok: true,
            conflicted: false,
            conflicted_files: vec![],
            message: merge.stdout.trim().to_string(),
        });
    }

    let files = remaining_conflicts(target)?;
    let has_conflict_marker = merge.stderr.contains("CONFLICT");
    if has_conflict_marker || !files.is_empty() {
        // Keep MERGING state — the resolver UI drives it from here.
        return Ok(MergeOutcome {
            ok: false,
            conflicted: true,
            conflicted_files: files,
            message: merge.stderr.trim().to_string(),
        });
    }

    // Anything else: abort so the tree isn't left in a broken state.
    let _ = run_at_target(target, ["merge", "--abort"]);
    Err(AppError::Git(format!(
        "merge failed: {}",
        merge.stderr.trim()
    )))
}

/// Inspect a single conflicted file. Text only; binary/large files fall back
/// to a side-only picker on the frontend.
pub fn conflict_detail(target: &Target, path: &str) -> AppResult<ConflictDetail> {
    let ours_raw = run_at_target(target, ["show", &format!(":2:{path}")])?;
    let theirs_raw = run_at_target(target, ["show", &format!(":3:{path}")])?;
    let base_raw = run_at_target(target, ["show", &format!(":1:{path}")]);

    let ours_bytes = ours_raw.stdout.as_bytes();
    let theirs_bytes = theirs_raw.stdout.as_bytes();

    // GitOutput.stdout 은 lossy UTF-8 변환을 거친 String 이라 from_utf8 검사는
    // 항상 통과한다 — 그걸로는 바이너리를 절대 못 잡는다. git 자신의 휴리스틱
    // (앞부분에 NUL 바이트가 있으면 바이너리)을 쓴다. NUL 은 유효한 UTF-8 이라
    // lossy 변환에서도 살아남으므로 신뢰할 수 있다.
    let has_nul = |b: &[u8]| b.iter().take(8000).any(|&c| c == 0);
    let is_binary = has_nul(ours_bytes) || has_nul(theirs_bytes);
    let too_large = ours_bytes.len() > MAX_TEXT_BYTES || theirs_bytes.len() > MAX_TEXT_BYTES;

    let (ours, theirs) = if is_binary || too_large {
        (String::new(), String::new())
    } else {
        (ours_raw.stdout.clone(), theirs_raw.stdout.clone())
    };

    let base = match base_raw {
        Ok(o) if o.ok() => Some(o.stdout),
        _ => None,
    };

    let working = read_working_file(target, path)?;

    Ok(ConflictDetail {
        path: path.to_string(),
        is_binary,
        too_large,
        base,
        ours,
        theirs,
        working,
    })
}

fn read_working_file(target: &Target, path: &str) -> AppResult<String> {
    let bytes = crate::git::read_file_at_target(target, path).unwrap_or_default();
    if bytes.is_empty() {
        // No working copy yet (binary, deleted, unreachable) — caller still
        // gets a valid empty string and renders side-only controls.
        return Ok(String::new());
    }
    if bytes.len() > MAX_TEXT_BYTES {
        return Ok(String::new());
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Apply the user's choice to a single file and stage it. Returns the list of
/// remaining unmerged paths after the operation.
pub fn resolve_conflict(target: &Target, path: &str, r: &Resolution) -> AppResult<Vec<String>> {
    match r {
        Resolution::Ours => {
            let out = run_at_target(target, ["checkout", "--ours", "--", path])?;
            if !out.ok() {
                // modify/delete 충돌: 고른 쪽 스테이지가 없다 = 그 쪽에서는
                // 파일이 삭제된 상태다. "그쪽을 쓴다"의 뜻은 삭제 반영이다.
                if stage_missing(&out.stderr) {
                    return remove_and_report(target, path);
                }
                return Err(AppError::Git(format!(
                    "ours 해결 실패: {}",
                    out.stderr.trim()
                )));
            }
        }
        Resolution::Theirs => {
            let out = run_at_target(target, ["checkout", "--theirs", "--", path])?;
            if !out.ok() {
                if stage_missing(&out.stderr) {
                    return remove_and_report(target, path);
                }
                return Err(AppError::Git(format!(
                    "theirs 해결 실패: {}",
                    out.stderr.trim()
                )));
            }
        }
        Resolution::Manual { content } => {
            crate::git::write_file_at_target(target, path, content.as_bytes())?;
        }
    }
    let add = run_at_target(target, ["add", "--", path])?;
    if !add.ok() {
        return Err(AppError::Git(format!(
            "staging 실패: {}",
            add.stderr.trim()
        )));
    }
    remaining_conflicts(target)
}

/// `git checkout --ours/--theirs` 가 "does not have our/their version" 으로
/// 실패했는가 — modify/delete 충돌에서 삭제된 쪽을 고른 경우다.
fn stage_missing(stderr: &str) -> bool {
    let e = stderr.to_lowercase();
    e.contains("does not have our version") || e.contains("does not have their version")
}

fn remove_and_report(target: &Target, path: &str) -> AppResult<Vec<String>> {
    let rm = run_at_target(target, ["rm", "-f", "--", path])?;
    if !rm.ok() {
        return Err(AppError::Git(format!(
            "파일 삭제 실패: {}",
            rm.stderr.trim()
        )));
    }
    remaining_conflicts(target)
}

pub fn remaining_conflicts(target: &Target) -> AppResult<Vec<String>> {
    let out = run_at_target(target, ["diff", "--name-only", "--diff-filter=U"])?;
    Ok(out
        .stdout
        .lines()
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .collect())
}

pub fn merge_in_progress(target: &Target) -> AppResult<bool> {
    let out: GitOutput = run_at_target(target, ["rev-parse", "-q", "--verify", "MERGE_HEAD"])?;
    Ok(out.ok())
}

/// Commit a successful merge. `None` → use git's prepared MERGE_MSG; `Some(m)`
/// overrides the message (the UI passes "feature/x 브랜치 병합").
pub fn complete_merge(target: &Target, message: Option<&str>) -> AppResult<MergeOutcome> {
    let args: Vec<String> = if let Some(m) = message {
        vec!["commit".into(), "-m".into(), m.to_string()]
    } else {
        vec!["commit".into(), "--no-edit".into()]
    };
    let out = run_at_target(target, args.iter().map(|s| s.as_str()))?;
    if out.ok() {
        Ok(MergeOutcome {
            ok: true,
            conflicted: false,
            conflicted_files: vec![],
            message: out.stdout.trim().to_string(),
        })
    } else {
        // Likely "no changes added to commit" because every conflict was resolved
        // to ours/theirs with no further diff — surface verbatim.
        Err(AppError::Git(out.stderr.trim().to_string()))
    }
}

/// `git merge --abort`. Tolerates "not in merging state" errors by ignoring
/// non-zero exit codes — the UI calls this from the cancel banner.
pub fn abort_merge(target: &Target) -> AppResult<()> {
    let _ = run_at_target(target, ["merge", "--abort"]);
    Ok(())
}

fn has_tracked_changes(target: &Target) -> AppResult<bool> {
    let out = run_at_target(target, ["status", "--porcelain=v2", "--untracked-files=no"])?;
    Ok(!out.stdout.trim().is_empty())
}
