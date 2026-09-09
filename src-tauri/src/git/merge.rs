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
    match target {
        // 로컬은 프로세스 spawn 이 싸니까 기존 경로 그대로.
        Target::Local(_) => list_pending_local(target, remote, base),
        // SSH 는 git 명령 한 번이 곧 SSH 연결 한 번이다. 브랜치마다 5~6개씩
        // 연결을 맺으면(merge-base ×2, rev-parse, rev-list, diff) 브랜치
        // 몇 개만 돼도 수십 초가 걸린다 — 스크립트 하나로 전부 계산해서
        // 연결은 **한 번**만 맺는다.
        Target::Ssh { .. } => list_pending_ssh(target, remote, base),
    }
}

/// 로컬 대상용 기존 구현 — 명령 하나당 프로세스 하나, 비용이 거의 없다.
fn list_pending_local(
    target: &Target,
    remote: &str,
    base: &str,
) -> AppResult<Vec<PendingBranch>> {
    // %(symref): refs/remotes/origin/HEAD 같은 심볼릭 ref에서만 비어 있지
    // 않다. %(refname:short)는 origin/HEAD를 "origin"으로 줄여 버려서
    // 이름 비교("origin/HEAD")로는 절대 거를 수 없다 — 원격 HEAD가 base가
    // 아닌 브랜치를 가리키면 "origin"이라는 유령 카드가 생기던 원인.
    let fmt =
        "%(refname:short)%09%(objectname)%09%(authorname)%09%(committerdate:unix)%09%(symref)%09%(subject)";
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
        let mut parts = line.splitn(6, '\t');
        let name = parts.next().unwrap_or("").to_string();
        let sha = parts.next().unwrap_or("").to_string();
        let author = parts.next().unwrap_or("").to_string();
        let unix_str = parts.next().unwrap_or("0");
        let symref = parts.next().unwrap_or("");
        let subject = parts.next().unwrap_or("").to_string();
        if !symref.is_empty() {
            continue; // origin/HEAD 포인터 — 브랜치가 아니다.
        }
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
        let mut parts = line.splitn(6, '\t');
        let name = parts.next().unwrap_or("").to_string();
        let sha = parts.next().unwrap_or("").to_string();
        let author = parts.next().unwrap_or("").to_string();
        let unix_str = parts.next().unwrap_or("0");
        let symref = parts.next().unwrap_or("");
        let subject = parts.next().unwrap_or("").to_string();
        if !symref.is_empty() {
            continue;
        }
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
                changed_files.push(ChangedPath {
                    path: crate::git::unquote_git_path(&path),
                    kind,
                });
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

/// SSH 대상 병합 대기 계산 — 스크립트 한 장을 원격 `sh -s` 로 보내 한 번의
/// 연결로 모든 브랜치를 계산한다. 기존엔 브랜치마다 SSH 연결 5~6개
/// (merge-base ×2, rev-parse, rev-list, diff)를 새로 맺어 브랜치 몇 개만
/// 돼도 수십 초가 걸렸다. 출력 규약:
///   `==\t<R|L>\t<refname>\t<sha>\t<author>\t<unix>\t<symref>\t<subject>`
///     — 후보 브랜치 하나의 시작 (R=원격 추적, L=로컬 브랜치)
///   `A\t<0|1>` — 원격 base(origin/<base>)에 이미 병합됐는가
///   `M\t<로컬 base 존재|1|0>\t<로컬 base에 병합됨|1|0>`
///   `C\t<ahead>\t<behind>` — rev-list --left-right --count 결과
///   `D\t<git diff --name-status 한 줄>` — 이 브랜치의 변경 파일
pub(crate) fn list_pending_ssh(target: &Target, remote: &str, base: &str) -> AppResult<Vec<PendingBranch>> {
    let Target::Ssh { user, host, key, password, port, path } = target else {
        return Err(AppError::Internal(
            "list_pending_ssh: SSH 대상이 아닙니다".into(),
        ));
    };
    let script = pending_probe_script(&path.to_string_lossy(), remote, base);
    let out = crate::git::run_ssh_script(user, host, key, password, *port, &script)?;
    if out.status != 0 {
        let detail = if out.stderr.trim().is_empty() {
            "원격에서 스크립트가 실패했습니다".to_string()
        } else {
            out.stderr.trim().to_string()
        };
        return Err(AppError::Git(format!("병합 대기 조회 실패: {detail}")));
    }
    parse_pending_output(&out.stdout, remote, base)
}

/// [`list_pending_ssh`] 가 원격에서 돌릴 스크립트. POSIX sh — Ubuntu/Debian
/// (dash)·NAS 의 busybox 까지 겨냥했다. `\t` 는 printf 가 해석한다.
pub fn pending_probe_script(path: &str, remote: &str, base: &str) -> String {
    let q_path = crate::git::shell_quote(path);
    let q_remote = crate::git::shell_quote(remote);
    let q_base = crate::git::shell_quote(base);
    format!(
        r#"cd {q_path} || {{ printf 'FATAL\tcd\n' >&2; exit 3; }}
R={q_remote}; B={q_base}; RB="$R/$B"
FMT='%(refname:short)%09%(objectname)%09%(authorname)%09%(committerdate:unix)%09%(symref)%09%(subject)'
probe() {{
  if git merge-base --is-ancestor "$1" "$RB" >/dev/null 2>&1; then
    printf 'A\t1\n'
  else
    printf 'A\t0\n'
  fi
  if git rev-parse -q --verify "refs/heads/$B" >/dev/null 2>&1; then
    if git merge-base --is-ancestor "$1" "refs/heads/$B" >/dev/null 2>&1; then
      printf 'M\t1\t1\n'
    else
      printf 'M\t1\t0\n'
    fi
  else
    printf 'M\t0\t0\n'
  fi
  counts=$(git rev-list --left-right --count "$RB...$1" 2>/dev/null)
  if [ -n "$counts" ]; then
    behind=$(printf '%s\n' "$counts" | cut -f1)
    ahead=$(printf '%s\n' "$counts" | cut -f2)
    printf 'C\t%s\t%s\n' "$ahead" "$behind"
  else
    # 공통 조상이 없는 새 브랜치 — ahead 만 센다 (로컬 경로와 같은 규칙).
    ahead=$(git rev-list --count "$1" 2>/dev/null)
    printf 'C\t%s\t0\n' "$ahead"
  fi
  git -c core.quotepath=off diff --name-status "$RB...$1" 2>/dev/null | while IFS= read -r dl; do
    printf 'D\t%s\n' "$dl"
  done
}}
git for-each-ref "refs/remotes/$R" --format="$FMT" | while IFS= read -r line; do
  printf '==\tR\t%s\n' "$line"
  probe "$(printf '%s' "$line" | cut -f1)"
done
git for-each-ref refs/heads --format="$FMT" | while IFS= read -r line; do
  printf '==\tL\t%s\n' "$line"
  probe "$(printf '%s' "$line" | cut -f1)"
done
"#
    )
}

/// [`pending_probe_script`] 출력을 파싱해 [`PendingBranch`] 목록으로 바꾼다.
/// 순수 함수 — 로컬 경로(list_pending_local)와 같은 필터를 거친다: 심볼릭
/// HEAD·base 자신·base에 이미 병합된 브랜치 제외, 원격/로컬 중복 제거.
pub fn parse_pending_output(stdout: &str, remote: &str, base: &str) -> AppResult<Vec<PendingBranch>> {
    let base_ref = format!("{remote}/{base}");
    struct Cand {
        kind: String,
        name: String,
        sha: String,
        author: String,
        unix_str: String,
        sym: String,
        subject: String,
        ancestor: bool,
        local_base_exists: bool,
        local_base_ancestor: bool,
        ahead: Option<u32>,
        behind: u32,
        files: Vec<ChangedPath>,
    }
    impl Default for Cand {
        fn default() -> Self {
            Self {
                kind: String::new(),
                name: String::new(),
                sha: String::new(),
                author: String::new(),
                unix_str: "0".into(),
                sym: String::new(),
                subject: String::new(),
                ancestor: false,
                local_base_exists: false,
                local_base_ancestor: false,
                ahead: None,
                behind: 0,
                files: Vec::new(),
            }
        }
    }

    let mut out: Vec<PendingBranch> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut cur: Option<Cand> = None;

    // 후보 하나를 확정한다 — 필터를 통과하면 목록에 넣고 seen 에 심는다.
    fn flush(
        cand: Option<Cand>,
        out: &mut Vec<PendingBranch>,
        seen: &mut std::collections::HashSet<String>,
        remote: &str,
        base: &str,
        base_ref: &str,
    ) {
        let Some(c) = cand else { return };
        if !c.sym.is_empty() || c.name.is_empty() || c.sha.is_empty() {
            return; // origin/HEAD 같은 심볼릭 포인터·불완전한 줄
        }
        if c.name == format!("{remote}/HEAD") || c.name.rsplit('/').next() == Some("HEAD") {
            return;
        }
        if c.kind == "R" {
            if c.name == base_ref {
                return; // origin/<base> 자신
            }
        } else if c.name == base || c.name == "HEAD" {
            return; // 로컬 base·HEAD
        }
        if c.ancestor {
            return; // base에 이미 병합됨
        }
        if seen.contains(&c.sha) {
            return; // 같은 팁을 가리키는 로컬/원격 중복
        }
        let Some(ahead) = c.ahead else { return }; // 사라진 ref 등 — 건너뀀다
        let short_name = if c.kind == "L" {
            c.name.clone()
        } else {
            c.name
                .strip_prefix(&format!("{remote}/"))
                .unwrap_or(&c.name)
                .to_string()
        };
        seen.insert(c.sha.clone());
        out.push(PendingBranch {
            name: c.name,
            short_name,
            sha: c.sha,
            author: c.author,
            unix_time: c.unix_str.parse().unwrap_or(0),
            subject: c.subject,
            ahead,
            behind: c.behind,
            changed_files: c.files,
            local: c.kind == "L",
            merged_locally: c.local_base_exists && c.local_base_ancestor,
        });
    }

    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("==\t") {
            flush(cur.take(), &mut out, &mut seen, remote, base, &base_ref);
            let (kind, fields) = match rest.split_once('\t') {
                Some(kv) => kv,
                None => continue,
            };
            let mut parts = fields.splitn(6, '\t');
            cur = Some(Cand {
                kind: kind.to_string(),
                name: parts.next().unwrap_or("").to_string(),
                sha: parts.next().unwrap_or("").to_string(),
                author: parts.next().unwrap_or("").to_string(),
                unix_str: parts.next().unwrap_or("0").to_string(),
                sym: parts.next().unwrap_or("").to_string(),
                subject: parts.next().unwrap_or("").to_string(),
                ..Default::default()
            });
            continue;
        }
        let Some(c) = cur.as_mut() else { continue };
        if let Some(f) = line.strip_prefix("A\t") {
            c.ancestor = f.trim() == "1";
        } else if let Some(f) = line.strip_prefix("M\t") {
            let mut it = f.split('\t');
            c.local_base_exists = it.next().unwrap_or("0").trim() == "1";
            c.local_base_ancestor = it.next().unwrap_or("0").trim() == "1";
        } else if let Some(f) = line.strip_prefix("C\t") {
            let mut it = f.split('\t');
            c.ahead = it.next().and_then(|v| v.trim().parse().ok());
            c.behind = it.next().and_then(|v| v.trim().parse().ok()).unwrap_or(0);
        } else if let Some(f) = line.strip_prefix("D\t") {
            // "<status>\t<path>" (rename 은 "R100\told\tnew") — 로컬 경로와
            // 동일하게 두 번째 필드를 경로로 쓴다.
            let mut fields = f.split('\t');
            let kind = fields.next().unwrap_or("").to_string();
            let path = fields.next().unwrap_or("").to_string();
            if (kind.starts_with('R') || kind.starts_with('C')) && fields.next().is_some() {
                // 새 경로는 버린다 — 기존 동작 유지.
            }
            if !path.is_empty() {
                c.files.push(ChangedPath {
                    path: crate::git::unquote_git_path(&path),
                    kind,
                });
            }
        }
        // 그 외(빈 줄 등)는 무시한다.
    }
    flush(cur, &mut out, &mut seen, remote, base, &base_ref);
    out.sort_by(|a, b| b.unix_time.cmp(&a.unix_time));
    Ok(out)
}

/// 원격 브랜치 삭제 결과. `auth_required` 는 HTTPS 원격(mod.lge.com 같은
/// Git 호스트)에 자격증명이 없거나 거부된 경우 — UI 는 이 플래그를 보고
/// 아이디/비밀번호 모달을 띄운 뒤 같은 브랜치로 재시도한다. 푸시
/// (`ops::PushOutcome`)와 같은 계약이라 두 흐름이 같은 로그인 모달을
/// 공유할 수 있다.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteBranchOutcome {
    pub ok: bool,
    pub message: String,
    /// HTTPS 원격 + 자격증명 부재/거부 → 로그인 모달이 필요하다.
    #[serde(default)]
    pub auth_required: bool,
    /// 원격 삭제 후 이 저장소에서 함께 정리한 로컬 상태
    /// (예: `["로컬 브랜치 feature/login"]`).
    #[serde(default)]
    pub cleaned_locally: Vec<String>,
    /// 커밋 유실 위험 등으로 지우지 않고 남긴 로컬 상태 — UI 가 사용자에게
    /// 직접 정리하도록 안내한다.
    #[serde(default)]
    pub kept_locally: Vec<String>,
}

/// 병합이 끝나 base에 완전히 포함된 원격 브랜치 — origin에 쌓인 죽은
/// feature 브랜치를 정리할 후보 목록이다. `mine` 은 이 저장소 사용자가
/// 만든(작성자) 브랜치인가 — UI 는 이 플래그를 보고 삭제 버튼을 활성화한다.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergedRemoteBranch {
    /// 원격 트래킹 이름 (예: "origin/feature/login").
    pub name: String,
    /// 브랜치 이름 (예: "feature/login") — 삭제 시 이 이름을 쓴다.
    pub short_name: String,
    /// 작성자 이름 — 브랜치 tip 커밋의 author (예: "홍길동").
    pub author: String,
    /// 작성자 이메일 — 브랜치 tip 커밋의 author email.
    pub author_email: String,
    pub unix_time: i64,
    /// 현재 사용자가 만든 브랜치인가 — 본인 브랜치만 삭제할 수 있다.
    #[serde(default)]
    pub mine: bool,
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
    let fmt = "%(refname:short)%09%(objectname)%09%(authorname)%09%(committerdate:unix)%09%(authoremail)";
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
        let mut parts = line.splitn(5, '\t');
        let name = parts.next().unwrap_or("").to_string();
        let sha = parts.next().unwrap_or("");
        let author = parts.next().unwrap_or("").to_string();
        let unix_time = parts.next().unwrap_or("0").parse::<i64>().unwrap_or(0);
        let author_email = parts.next().unwrap_or("").trim().to_string();
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
            author_email,
            unix_time,
            mine: false,
        });
    }
    // 오래된 것부터 — 제일 먼저 정리해도 되는 것.
    out.sort_by(|a, b| a.unix_time.cmp(&b.unix_time));
    Ok(out)
}

/// 브랜치 작성자가 현재 사용자(내 브랜치)인가.
///
/// 작성자 이름이 git 신원(user.name)과 같으면 내 브랜치, 아니면 이메일
/// (user.email)이 같아도 내 브랜치다 — 이름은 팀원끼리 겹칠 수 있지만
/// 이메일은 팀 단위로 유일하기 때문(gpconfig 가 팀원을 이메일로 매칭하는
/// 이유와 같다). 공백·대소문자는 무시한다. 신원 자체를 모르면(None) 어떤
/// 브랜치도 "내 것"이 아니다 — 삭제를 허락하는 것보다 거부하는 쪽이 안전하다.
pub fn identity_matches(
    author_name: &str,
    author_email: &str,
    identity_name: Option<&str>,
    identity_email: Option<&str>,
) -> bool {
    let an = author_name.trim();
    let ae = author_email.trim().to_lowercase();
    if !an.is_empty()
        && identity_name
            .map(|n| n.trim().eq_ignore_ascii_case(an))
            .unwrap_or(false)
    {
        return true;
    }
    if !ae.is_empty()
        && identity_email
            .map(|e| e.trim().to_lowercase() == ae)
            .unwrap_or(false)
    {
        return true;
    }
    false
}

/// 현재 사용자의 git 신원 (user.name, user.email).
///
/// - 로컬 대상: 이 컴퓨터의 저장소 git 설정을 읽는다 — 브랜치 author 도 git
///   이 기록한 값이라 **같은 출처**끼리 비교해 오탐이 없다. 설정이 비어 있으면
///   로그인 계정(name/email)으로 대체한다.
/// - SSH 대상: 원격 저장소의 git config 는 서버 주인을 말해 주지 않으므로
///   로그인 계정을 우선 쓴다.
pub fn current_git_identity(
    target: &Target,
    session: Option<(&str, &str)>,
) -> (Option<String>, Option<String>) {
    if matches!(target, Target::Ssh { .. }) {
        return session
            .map(|(n, e)| (Some(n.to_string()), Some(e.to_string())))
            .unwrap_or((None, None));
    }
    let query = |key: &str| {
        run_at_target(target, ["config", key])
            .ok()
            .filter(|o| o.ok())
            .map(|o| o.stdout.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let name = query("user.name");
    let email = query("user.email");
    if name.is_none() && email.is_none() {
        return session
            .map(|(n, e)| (Some(n.to_string()), Some(e.to_string())))
            .unwrap_or((None, None));
    }
    // 한쪽만 설정된 경우 빈 쪽을 세션 값으로 채운다.
    let (sn, se) = session.unwrap_or(("", ""));
    (
        name.or_else(|| (!sn.is_empty()).then(|| sn.to_string())),
        email.or_else(|| (!se.is_empty()).then(|| se.to_string())),
    )
}

/// 병합이 끝난 원격 브랜치를 origin에서 삭제한다 (`push <remote> --delete`).
///
/// 안전장치: base 자신은 거부하고, 브랜치가 `<remote>/<base>`의 조상인지
/// (=커밋이 전부 base에 들어갔는지) 삭제 직전에 다시 확인한다 — 목록을 본
/// 뒤 팀원이 새 커밋을 push했다면 여기서 멈춘다. 성공하면 `fetch --prune`으로
/// 로컬 트래킹 ref도 정리한다.
///
/// HTTPS 원격(Git 호스트)은 `push --delete` 도 로그인이 필요하다 — 푸시와
/// 같은 [`ops::PushCredential`] 을 받아 Basic 인증 헤더를 실어 보내고,
/// 자격증명이 없으면 git 이 터미널 프롬프트에 매달리는 대신(앱은 stdin 이
/// 닫혀 있어 "could not read Username … No such device or address" 로
/// 죽는다) `auth_required: true` 를 돌려줘 UI 가 로그인 모달을 띄우게 한다.
pub fn delete_remote_branch(
    target: &Target,
    remote: &str,
    base: &str,
    branch: &str,
    credentials: Option<&crate::config_store::PushCredential>,
) -> AppResult<DeleteBranchOutcome> {
    if branch == base {
        return Err(AppError::Git(format!(
            "병합 브랜치({base})는 삭제할 수 없습니다."
        )));
    }
    // 심층 방어: .gpconfig의 병합 대상 브랜치(develop, release/1.0 …)는
    // 어떤 호출 경로로도 지우지 않는다 — 커맨드 계층의 필터에만 의존하면
    // merge 계층을 직접 쓰는 코드가 팀의 합류 지점을 지울 수 있다.
    if let Ok((cfg, exists)) = crate::gpconfig::read_config_effective(target, base, remote) {
        if exists
            && (cfg.merge_targets.iter().any(|t| t == branch)
                || cfg.default_base_branch == branch)
        {
            return Err(AppError::Git(format!(
                "{branch}은(는) 병합 대상 브랜치라 삭제할 수 없습니다."
            )));
        }
    }
    // HTTPS 원격인가 — 자격증명이 필요한지 결정한다. URL 에 이미
    // `http://user:pass@host/…` 처럼 자격증명이 박혀 있으면 git 이 그걸
    // 쓰므로(프롬프트 없음) 평범한 push 로 충분하다.
    let https = crate::git::ops::remote_is_https(target, remote);
    if https && credentials.is_none() {
        // 푸시와 같은 정책: 자격증명 없이 HTTPS push 는 아예 시도하지 않는다
        // — 터미널 프롬프트를 쓸 수 없는 이 앱에서는 반드시 실패한다.
        return Ok(DeleteBranchOutcome {
            ok: false,
            message: "Git 호스트 로그인이 필요합니다. 삭제할 때 아이디/비밀번호를 입력하세요."
                .to_string(),
            auth_required: true,
            cleaned_locally: vec![],
            kept_locally: vec![],
        });
    }
    // 낡은 트래킹 ref 로 검사하면 마지막 fetch **이후**에 팀원이 push한
    // 커밋이 보이지 않아 가드가 뚫린다 — 삭제 직전에 그 브랜치를 다시
    // 받아 실제 tip 기준으로 확인한다. (fetch 실패는 관용: 오프라인이면
    // 아래 push --delete 도 어차피 실패한다. 자격증명이 있으면 fetch 도
    // 같은 헤더로 보낸다.)
    if https {
        let cred = credentials.unwrap();
        let _ = crate::git::ops::run_http_with_credentials(
            target,
            cred,
            &["fetch", remote, branch],
        );
    } else {
        let _ = run_at_target(target, ["fetch", remote, branch]);
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
    // 여기까지 왔으면 삭제해도 안전하다 — 이제 진짜 삭제.
    let out = if https {
        crate::git::ops::run_http_with_credentials(
            target,
            credentials.unwrap(),
            &["push", remote, "--delete", branch],
        )?
    } else {
        run_at_target(target, ["push", remote, "--delete", branch])?
    };
    if !out.ok() {
        let auth_required = https && crate::git::ops::is_auth_failure(&out.stderr);
        let message = if auth_required {
            // 실제 stderr 를 함께 보여 준다 — "could not read Username" 같은
            // 원인이 그대로 보여야 저장된 자격증명이 잘못됐음을 알 수 있다.
            format!(
                "Git 호스트 로그인 실패: {}",
                crate::git::ops::friendly_git_error(&out.stderr)
            )
        } else {
            format!(
                "원격 브랜치 삭제 실패: {}",
                crate::git::ops::friendly_git_error(&out.stderr)
            )
        };
        return Ok(DeleteBranchOutcome {
            ok: false,
            message,
            auth_required,
            cleaned_locally: vec![],
            kept_locally: vec![],
        });
    }
    // 성공 — 로컬 트래킹 ref 정리. best-effort(오프라인이어도 삭제는 됐다).
    if https {
        let _ = crate::git::ops::run_http_with_credentials(
            target,
            credentials.unwrap(),
            &["fetch", "--prune", remote],
        );
    } else {
        let _ = run_at_target(target, ["fetch", "--prune", remote]);
    }
    // 원격 브랜치는 지워졌다 — 이제 이 저장소에 남은 같은 이름의 **로컬**
    // 브랜치를 함께 정리한다. 안 지우면 "병합 탭에서 지웠는데 터미널의
    // git branch 에 그대로 보인다"가 된다 (원격 ref 와 로컬 ref 는 별개다).
    // 안전 규칙: 현재 체크아웃된 브랜치는 건드리지 않고, 커밋이 전부 로컬
    // base 에 들어간 경우에만 지운다. 팀원의 다른 작업 폴더에 있는 로컬
    // 브랜치는 우리가 지울 수 없으므로, 그쪽은 각자 fetch --prune 후
    // 정리하도록 안내만 한다.
    let (cleaned_locally, kept_locally) = cleanup_local_branch(target, base, branch);
    Ok(DeleteBranchOutcome {
        ok: true,
        message: format!("{remote}/{branch} 브랜치를 삭제했습니다."),
        auth_required: false,
        cleaned_locally,
        kept_locally,
    })
}

/// [`delete_remote_branch`] 성공 후, 같은 이름의 로컬 브랜치를 안전하게
/// 정리한다. 반환값은 (지운 것들, 유지한 것들) — 각 항목은 한국어 설명
/// 문자열이라 UI 토스트에 그대로 붙일 수 있다.
///
/// 판단 순서:
/// 1. 로컬 브랜치가 없으면 할 일이 없다.
/// 2. 현재 체크아웃된 브랜치면 지울 수 없다 — 유지 안내.
/// 3. 로컬 브랜치 tip 이 로컬 base 의 조상이면(=커밋이 전부 base 에
///    들어갔다) 확실히 안전하므로 `-D` 로 지운다.
/// 4. 아니면 `git branch -d` 의 자체 판단(HEAD/업스트림 병합 검사)에
///    맡기고, 그것도 거부되면(아직 base 에 없는 커밋이 남음) 유지한다.
fn cleanup_local_branch(target: &Target, base: &str, branch: &str) -> (Vec<String>, Vec<String>) {
    let local_ref = format!("refs/heads/{branch}");
    let exists = run_at_target(target, ["rev-parse", "-q", "--verify", &local_ref])
        .map(|o| o.ok())
        .unwrap_or(false);
    if !exists {
        return (vec![], vec![]);
    }

    let current = run_at_target(target, ["symbolic-ref", "-q", "--short", "HEAD"])
        .map(|o| o.stdout.trim().to_string())
        .unwrap_or_default();
    if current == branch {
        return (vec![], vec![format!("로컬 브랜치 {branch} — 지금 작업 중인 브랜치라 남겼습니다")]);
    }

    // 커밋이 전부 로컬 base 에 들어갔는가 — 맞으면 강제로 지워도 잃을 게 없다.
    let all_in_base = {
        let base_ref = format!("refs/heads/{base}");
        run_at_target(target, ["merge-base", "--is-ancestor", &local_ref, &base_ref])
            .map(|o| o.ok())
            .unwrap_or(false)
    };
    let out = if all_in_base {
        run_at_target(target, ["branch", "-D", branch])
    } else {
        run_at_target(target, ["branch", "-d", branch])
    };
    let ok = out.as_ref().map(|o| o.ok()).unwrap_or(false);
    if ok {
        (vec![format!("로컬 브랜치 {branch}")], vec![])
    } else if all_in_base {
        // base 에 다 들어갔는데도 실패 — 일반적이지 않지만(ref 잠금 등)
        // 실패 사유를 남겨 사용자가 직접 지울 수 있게 한다.
        let detail = out
            .map(|o| o.stderr)
            .unwrap_or_else(|e| e.to_string());
        (
            vec![],
            vec![format!(
                "로컬 브랜치 {branch} 정리 실패: {}",
                crate::git::ops::friendly_git_error(&detail)
            )],
        )
    } else {
        (
            vec![],
            vec![format!(
                "로컬 브랜치 {branch} — 아직 base 에 들어가지 않은 커밋이 있어 남겼습니다 (확인 후 직접 지우세요: git branch -D {branch})"
            )],
        )
    }
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
    expected_sha: Option<&str>,
) -> AppResult<MergeOutcome> {
    // 이미 병합이 진행 중이면 새 병합을 시작하지 않는다. 특히 충돌을 전부
    // ours로 해결해 둔 상태는 인덱스가 HEAD와 같아 아래 dirty-tree 가드를
    // 통과하고, 이어지는 `checkout <base>`가 "Already on <base>"이면서도
    // MERGE_HEAD를 지워 버린다 — 사용자가 풀어 둔 병합이 소리 없이 증발한다.
    if merge_in_progress(target)? {
        return Err(AppError::Git(
            "이미 진행 중인 병합이 있습니다. 병합 탭에서 먼저 마무리하거나 중단하세요.".into(),
        ));
    }
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

    // fetch --prune 이후의 실제 tip 확인 — 관리자가 화면에서 검토한 것과
    // 지금 병합될 것이 같은지 검증한다. 병합 요청 ref(refs/gc-mr/*)는 태그
    // 객체를 가리키므로 ^{commit} 으로 커밋으로 벗겨 비교한다 — 어느 ref를
    // 넘겨도 (브랜치든 요청이든) 실제 병합될 커밋 기준으로 판정한다.
    let tip = run_at_target(
        target,
        ["rev-parse", "-q", "--verify", &format!("{branch_ref}^{{commit}}")],
    )?;
    if !tip.ok() {
        return Err(AppError::Git(format!(
            "{branch_ref} 브랜치를 찾을 수 없습니다 — 방금 원격에서 삭제되었을 수 있습니다. 목록을 새로고침하세요."
        )));
    }
    if let Some(expected) = expected_sha {
        let actual = tip.stdout.trim();
        if !expected.is_empty() && actual != expected && !actual.starts_with(expected) {
            return Err(AppError::Git(format!(
                "검토한 뒤 이 브랜치에 새 push가 있었습니다(또는 히스토리가 바뀌었습니다). 목록을 새로고침해 최신 내용을 확인한 뒤 다시 병합하세요. (검토: {} → 현재: {})",
                &expected[..expected.len().min(7)],
                &actual[..actual.len().min(7)],
            )));
        }
    }

    let checkout = run_at_target(target, ["checkout", base])?;
    if !checkout.ok() {
        return Err(AppError::Git(format!(
            "checkout {base} 실패: {}",
            crate::git::ops::friendly_git_error(&checkout.stderr)
        )));
    }

    // 병합 커밋 문구는 사람이 읽는다 — ref 경로(refs/gc-mr/…)가 아니라
    // 브랜치 이름이 남게 한다. 요청 ref는 refs/gc-mr/<base>/<branch> 꼴이다.
    let short = if let Some(rest) = branch_ref.strip_prefix("refs/gc-mr/") {
        rest.split_once('/')
            .map(|(_, branch)| branch.to_string())
            .unwrap_or_else(|| rest.to_string())
    } else {
        branch_ref
            .strip_prefix(&format!("{remote}/"))
            .unwrap_or(branch_ref)
            .to_string()
    };
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
        "병합 실패: {}",
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
                    crate::git::ops::friendly_git_error(&out.stderr)
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
                    crate::git::ops::friendly_git_error(&out.stderr)
                )));
            }
        }
        Resolution::Manual { content } => {
            // 충돌 마커가 남은 본문이 그대로 스테이징·커밋되면 팀 전체에
            // 배포된다 — 자동 경로(valid_ai_body)와 같은 규칙으로 거부한다.
            // 단, 파일의 **원문**(스테이지 :1/:2/:3)에 원래 있던 마커-닮은
            // 줄(문서의 git 예시 등)은 정당한 내용이다 — 그것 때문에 수동
            // 병합이 영영 막히면 사용자는 파일 통째 선택으로 내몰린다.
            if has_novel_markers(target, path, content) {
                return Err(AppError::Git(
                    "충돌 표시(<<<<<<< 또는 >>>>>>>)가 아직 남아 있습니다. 모든 블록을 해결한 뒤 저장하세요."
                        .into(),
                ));
            }
            // 아래 staging(add)이 실패하면(잠금 등) Err 를 돌려주면서도
            // 워크트리는 이미 새 내용으로 바뀌어 있던 문제 — 실패 시 원래
            // 바이트로 되돌려 "실패 = 상태 불변"을 지킨다.
            let before = crate::git::read_file_at_target(target, path).ok();
            crate::git::write_file_at_target(target, path, content.as_bytes())?;
            let add = run_at_target(target, ["add", "--", path])?;
            if !add.ok() {
                if let Some(orig) = before {
                    let _ = crate::git::write_file_at_target(target, path, &orig);
                }
                return Err(AppError::Git(format!(
                    "staging 실패: {}",
                    crate::git::ops::friendly_git_error(&add.stderr)
                )));
            }
            return remaining_conflicts(target);
        }
    }
    let add = run_at_target(target, ["add", "--", path])?;
    if !add.ok() {
        return Err(AppError::Git(format!(
            "staging 실패: {}",
            crate::git::ops::friendly_git_error(&add.stderr)
        )));
    }
    remaining_conflicts(target)
}

/// 줄 첫머리 기준의 충돌 마커 줄 판정. `=======` 단독은 정당한 내용일 수
/// 있어 제외하고, git이 항상 함께 쓰는 시작(`<<<<<<< `)·종료(`>>>>>>> `)·
/// 베이스(`|||||||`) 마커만 본다.
pub(crate) fn is_conflict_marker_line(l: &str) -> bool {
    l.starts_with("<<<<<<< ")
        || l.starts_with(">>>>>>> ")
        || l.starts_with("|||||||")
        || l == "<<<<<<<"
        || l == ">>>>>>>"
}

pub(crate) fn has_unresolved_markers(content: &str) -> bool {
    content.lines().any(is_conflict_marker_line)
}

/// 본문에 남은 마커 줄 중, 파일 원문(충돌 스테이지 :1/:2/:3)에는 **없던**
/// 것이 있는가. 원문에 이미 있던 마커-닮은 줄은 내용이고, 새로 생긴 마커는
/// 해결되지 않은 블록이다.
fn has_novel_markers(target: &Target, path: &str, content: &str) -> bool {
    let mut markers = content.lines().filter(|l| is_conflict_marker_line(l)).peekable();
    if markers.peek().is_none() {
        return false;
    }
    let mut stage_lines: std::collections::HashSet<String> = std::collections::HashSet::new();
    for st in [":1:", ":2:", ":3:"] {
        if let Ok(out) = run_at_target(target, ["show", &format!("{st}{path}")]) {
            if out.ok() {
                for l in out.stdout.lines().filter(|l| is_conflict_marker_line(l)) {
                    stage_lines.insert(l.to_string());
                }
            }
        }
    }
    markers.any(|l| !stage_lines.contains(l))
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
        .filter(|s| !s.is_empty())
        .map(crate::git::unquote_git_path)
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

/// `git merge --abort`. Tolerates "not in merging state" (배너에서 병합이
/// 없어도 호출된다) — 하지만 병합이 **아직 남아 있는데** 실패한 경우
/// (index.lock 경합 등)를 성공으로 보고하면 UI 가 거짓 상태로 넘어간다.
pub fn abort_merge(target: &Target) -> AppResult<()> {
    let out = run_at_target(target, ["merge", "--abort"])?;
    if merge_in_progress(target)? {
        return Err(AppError::Git(format!(
            "병합 중단 실패: {}",
            crate::git::ops::friendly_git_error(&out.stderr)
        )));
    }
    Ok(())
}

fn has_tracked_changes(target: &Target) -> AppResult<bool> {
    let out = run_at_target(target, ["status", "--porcelain=v2", "--untracked-files=no"])?;
    Ok(!out.stdout.trim().is_empty())
}
