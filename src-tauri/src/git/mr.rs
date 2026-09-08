//! 병합 요청(merge request) — 푸시와 승인을 분리하는 대기열.
//!
//! 팀원은 자기 브랜치에 자유롭게 push한다(작업 공유). **병합 요청 보내기를
//! 눌렀을 때만** 관리자의 병합 대기열에 오른다. 요청은 별도 서버 없이
//! 원격 저장소의 git ref 로 공유한다:
//!
//! ```text
//! refs/gc-mr/<base>/<브랜치>        ← 병합 대상별/브랜치별로 하나
//! ```
//!
//! `refs/merge-requests`를 쓰지 않은 이유: GitLab이 그 이름공간을 숨기고
//! push를 거부한다. `gc-mr`은 어느 git 호스트에서나 일반 ref로 취급된다.
//!
//! ref가 가리키는 것은 태그 객체고, 요청 메타데이터(제목·요청자·시각)는
//! 태그 message에 JSON으로 넣는다. 요청 시점의 tip(sha)이 **고정**되므로
//! 관리자가 검토한 것과 병합되는 것이 항상 같다 — 요청 뒤 새 push가 있으면
//! 팀원이 다시 요청해 ref를 갱신한다(또는 관리자가 거절한다).
//!
//! 일반 `git fetch`는 refs/heads만 가져오므로, 이 ref는
//! `git fetch origin +refs/gc-mr/*:refs/gc-mr/*`로 따로 받아온다
//! ([`crate::git::fetch::fetch_target`]이 함께 돌린다).
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::git::merge::ChangedPath;
use crate::git::{run_at_target, Target};

/// 병합 요청 ref의 이름공간.
pub const MR_REF_PREFIX: &str = "refs/gc-mr";

/// 닫힌 이유 — 병합으로 닫혔는가, 거절됐는가.
pub const REASON_MERGED: &str = "merged";
pub const REASON_REJECTED: &str = "rejected";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeRequest {
    /// 병합 대상 브랜치 (main, develop …).
    pub base: String,
    /// 요청한 브랜치 짧은 이름 (feature/login).
    pub branch: String,
    /// 전체 ref 경로 (refs/gc-mr/main/feature/login).
    #[serde(default)]
    pub ref_path: String,
    /// 요청 시점의 브랜치 tip — 이 커밋이 곧 병합될 것.
    pub sha: String,
    /// 요청 제목 — 기본은 마지막 커밋 제목.
    pub title: String,
    /// 요청한 사람 (이름·이메일). 로그인해 있으면 계정에서 온다.
    pub author: String,
    pub email: String,
    /// 요청 시각 (unix seconds).
    pub created_at: i64,
    /// 아직 열려 있는가 — 닫힌 요청은 ref가 지워진다. 항상 true로 읽힌다
    /// (닫힌 것은 목록에서 자동으로 치우므로). 와이어 호환을 위해 남겨 둔다.
    #[serde(default = "default_open")]
    pub open: bool,
    /// 원격 공유 실패(오프라인·권한)로 이 컴퓨터에만 저장된 요청.
    #[serde(default)]
    pub local_only: bool,
}

/// 요청 + 병합 센터 카드가 그리는 브랜치 스냅샷(요청 tip 기준).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestedMerge {
    pub request: MergeRequest,
    pub ahead: u32,
    pub behind: u32,
    pub changed_files: Vec<ChangedPath>,
    /// origin/<branch>가 아직 있는가 — 지워져도 요청 tip으로는 병합할 수 있다.
    pub branch_exists: bool,
}

pub fn mr_ref_path(base: &str, branch: &str) -> String {
    format!("{MR_REF_PREFIX}/{base}/{branch}")
}

fn default_open() -> bool {
    true
}

fn ref_branch(base: &str, refname: &str) -> Option<String> {
    refname
        .strip_prefix(&format!("{MR_REF_PREFIX}/{base}/"))
        .map(|s| s.to_string())
}

fn verify_ref(target: &Target, rev: &str) -> AppResult<Option<String>> {
    let out = run_at_target(target, ["rev-parse", "-q", "--verify", rev])?;
    if !out.ok() {
        return Ok(None);
    }
    Ok(Some(out.stdout.trim().to_string()))
}

/// 병합 요청을 만든다(또는 같은 브랜치의 기존 요청을 갱신한다).
///
/// 규칙: **푸시된 커밋만 요청할 수 있다.** 로컬 tip과 origin/<branch>가
/// 다르면 푸시를 먼저 요구한다 — 승인 대상이 어느 컴퓨터에서 보든 같아야
/// 하기 때문이다. base에 이미 병합된 브랜치, 새 커밋이 없는 브랜치도 거부한다.
pub fn request_merge(
    target: &Target,
    remote: &str,
    base: &str,
    branch: &str,
    title: Option<&str>,
    author: &str,
    email: &str,
) -> AppResult<MergeRequest> {
    let base_ref = format!("{remote}/{base}");
    let branch_ref = format!("{remote}/{branch}");
    let local_ref = format!("refs/heads/{branch}");

    let local_sha = verify_ref(target, &local_ref)?.ok_or_else(|| {
        AppError::Git(format!(
            "브랜치 {branch}가 이 컴퓨터에 없습니다. 작업 탭에서 브랜치를 확인하세요."
        ))
    })?;
    let remote_sha = verify_ref(target, &branch_ref)?.ok_or_else(|| {
        AppError::Git(format!(
            "origin/{branch}가 없습니다. 병합 요청은 push된 커밋에만 할 수 있습니다 — 먼저 푸시하세요."
        ))
    })?;
    if local_sha != remote_sha {
        return Err(AppError::Git(
            "푸시하지 않은 커밋이 있습니다. 먼저 푸시한 뒤 병합 요청하세요.".into(),
        ));
    }
    verify_ref(target, &base_ref)?.ok_or_else(|| {
        AppError::Git(format!(
            "병합 대상 브랜치 {base}가 원격에 없습니다. 설정 탭에서 병합 대상을 확인하세요."
        ))
    })?;

    let ancestor = run_at_target(target, ["merge-base", "--is-ancestor", &branch_ref, &base_ref])?;
    if ancestor.ok() {
        return Err(AppError::Git(format!(
            "{branch}는 이미 {base}에 병합되어 있습니다. 요청할 필요가 없습니다."
        )));
    }
    let count = run_at_target(
        target,
        ["rev-list", "--count", &format!("{base_ref}..{branch_ref}")],
    )?;
    let ahead: u32 = count.stdout.trim().parse().unwrap_or(0);
    if ahead == 0 {
        return Err(AppError::Git(
            "병합 대상과 다른 커밋이 없습니다. 요청할 변경이 없습니다.".into(),
        ));
    }

    // 제목 기본값 = 마지막 커밋 제목.
    let title = match title.map(str::trim).filter(|s| !s.is_empty()) {
        Some(t) => t.to_string(),
        None => {
            let out = run_at_target(target, ["log", "-1", "--format=%s", &branch_ref])?;
            let s = out.stdout.trim();
            if s.is_empty() {
                format!("{branch} 병합 요청")
            } else {
                s.to_string()
            }
        }
    };
    let author = author.trim();
    let author = if author.is_empty() { "?" } else { author };

    let mut req = MergeRequest {
        base: base.to_string(),
        branch: branch.to_string(),
        ref_path: mr_ref_path(base, branch),
        sha: remote_sha.clone(),
        title,
        author: author.to_string(),
        email: email.trim().to_string(),
        created_at: chrono::Utc::now().timestamp(),
        open: true,
        local_only: false,
    };

    write_request_ref(target, &req)?;
    req.local_only = !push_request_ref(target, remote, &req.ref_path);
    Ok(req)
}

/// 요청 메타데이터를 태그 객체로 만들어 ref에 박는다.
///
/// `git tag -a`는 임시 이름으로 만든 뒤 update-ref로 옮긴다 — 임의 경로의
/// ref는 `git tag`로 직접 만들 수 없고, `hash-object --stdin`은 SSH 대상에서
/// stdin을 못 쓰기 때문이다. `-c user.name/email`로 tagger를 요청자로 고정한다
/// (git 전역 설정이 없는 컴퓨터에서도 실패하지 않게).
fn write_request_ref(target: &Target, req: &MergeRequest) -> AppResult<()> {
    let payload = serde_json::json!({
        "base": req.base,
        "branch": req.branch,
        "ref_path": req.ref_path,
        "sha": req.sha,
        "title": req.title,
        "author": req.author,
        "email": req.email,
        "created_at": req.created_at,
        "open": true,
    })
    .to_string();
    let tmp = format!("gc-mr-tmp-{}", uuid::Uuid::new_v4());
    let email = if req.email.is_empty() {
        "merge-request@gitcompanion.local"
    } else {
        &req.email
    };
    let tag = run_at_target(
        target,
        [
            "-c",
            &format!("user.name={}", req.author),
            "-c",
            &format!("user.email={email}"),
            "tag",
            "-a",
            "-f",
            "-m",
            &payload,
            &tmp,
            &req.sha,
        ],
    )?;
    if !tag.ok() {
        return Err(AppError::Git(format!(
            "병합 요청을 기록하지 못했습니다: {}",
            tag.stderr.trim()
        )));
    }
    let tag_sha = run_at_target(target, ["rev-parse", &format!("refs/tags/{tmp}")])?;
    let _ = run_at_target(target, ["tag", "-d", &tmp]);
    if !tag_sha.ok() {
        return Err(AppError::Git("병합 요청 객체를 찾지 못했습니다.".into()));
    }
    let upd = run_at_target(
        target,
        ["update-ref", &req.ref_path, tag_sha.stdout.trim()],
    )?;
    if !upd.ok() {
        return Err(AppError::Git(format!(
            "병합 요청 ref를 쓰지 못했습니다: {}",
            upd.stderr.trim()
        )));
    }
    Ok(())
}

/// 요청 ref를 원격에 올린다(또는 갱신한다). 실패해도 요청은 로컬에 남고
/// 호출부가 `local_only`로 표시한다 — 오프라인에서 흐름이 막히지 않게.
fn push_request_ref(target: &Target, remote: &str, ref_path: &str) -> bool {
    let spec = format!("+{ref_path}:{ref_path}");
    let out = run_at_target(target, ["push", remote, &spec]);
    out.map(|o| o.ok()).unwrap_or(false)
}

/// 요청 ref 하나를 닫는다: 로컬에서 지우고 원격에서도 지운다.
///
/// 로컬 삭제는 항상 먼저 한다 — 원격 삭제가 실패해도(오프라인) 이 컴퓨터의
/// 대기열은 비운다. 원격 실패는 호출부로 그대로 올려 화면이 알린다.
pub fn close_request(
    target: &Target,
    remote: &str,
    base: &str,
    branch: &str,
) -> AppResult<()> {
    let ref_path = mr_ref_path(base, branch);
    let _ = run_at_target(target, ["update-ref", "-d", &ref_path]);
    // `:<ref>` 형태가 임의 경로의 ref 삭제에 가장 확실하다.
    let spec = format!(":{ref_path}");
    let out = run_at_target(target, ["push", remote, &spec]);
    match out {
        Ok(o) if o.ok() => Ok(()),
        Ok(o) => {
            // 이미 지워진 ref(다른 관리자가 먼저 닫음)는 성공과 같다.
            let err = o.stderr.to_lowercase();
            if err.contains("does not exist") {
                Ok(())
            } else {
                Err(AppError::Git(format!(
                    "원격 병합 요청 정리 실패: {}",
                    crate::git::ops::friendly_git_error(&o.stderr)
                )))
            }
        }
        Err(e) => Err(e),
    }
}

/// 요청 ref의 태그 message에서 메타데이터 JSON을 읽는다.
fn read_request(target: &Target, _refname: &str, object: &str) -> Option<MergeRequest> {
    let out = run_at_target(target, ["cat-file", "tag", object]).ok()?;
    if !out.ok() {
        return None;
    }
    let body = out.stdout.split("\n\n").nth(1)?.trim();
    let mut req: MergeRequest = serde_json::from_str(body).ok()?;
    req.open = true;
    req.local_only = false;
    Some(req)
}

/// 이 base의 열린 병합 요청 목록. 오래된 요청부터 정렬한다 — 먼저 기다린
/// 팀원의 요청이 위에 온다.
///
/// 부수 효과(자동 정리): 요청 tip이 이미 base에 들어갔으면 그 요청은 닫는다
/// (로컬 ref 삭제 + 원격 삭제, 원격 실패는 무시). 앱 밖 터미널에서 병합된
/// 브랜치의 요청이 대기열에 영원히 남는 것을 막는다.
pub fn list_requests(target: &Target, remote: &str, base: &str) -> AppResult<Vec<MergeRequest>> {
    let prefix = format!("{MR_REF_PREFIX}/{base}/");
    let list = run_at_target(
        target,
        [
            "for-each-ref",
            &prefix,
            "--format=%(objectname)%09%(refname)",
        ],
    )?;
    if !list.ok() {
        return Err(AppError::Git(format!(
            "병합 요청 목록 조회 실패: {}",
            list.stderr.trim()
        )));
    }
    let base_ref = format!("{remote}/{base}");
    let mut out = Vec::new();
    for line in list.stdout.lines() {
        let Some((object, refname)) = line.split_once('\t') else {
            continue;
        };
        let (object, refname) = (object.trim(), refname.trim());
        if object.is_empty() || refname.is_empty() {
            continue;
        }
        let Some(branch) = ref_branch(base, refname) else {
            continue;
        };
        let Some(mut req) = read_request(target, refname, object) else {
            // 읽을 수 없는 ref(깨진 JSON 등)는 조용히 건너뛴다 — 대기열
            // 전체가 죽지 않게.
            continue;
        };
        req.branch = branch;
        req.ref_path = refname.to_string();
        req.base = base.to_string();

        // 이미 병합됐는가 — 요청 tip 또는 브랜치 tip이 base의 조상이면 닫는다.
        let merged = run_at_target(
            target,
            ["merge-base", "--is-ancestor", &req.sha, &base_ref],
        )?
        .ok();
        let branch_ref = format!("{remote}/{}", req.branch);
        let tip_merged = verify_ref(target, &branch_ref)?
            .map(|tip| run_at_target(target, ["merge-base", "--is-ancestor", &tip, &base_ref]).map(|o| o.ok()).unwrap_or(false))
            .unwrap_or(false);
        if merged || tip_merged {
            let _ = close_request(target, remote, base, &req.branch);
            continue;
        }
        out.push(req);
    }
    out.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.branch.cmp(&b.branch)));
    Ok(out)
}

/// 대기열 카드용 스냅샷 — 요청 tip 기준의 ahead/behind/변경 파일.
fn snapshot(
    target: &Target,
    base_ref: &str,
    sha: &str,
) -> AppResult<(u32, u32, Vec<ChangedPath>)> {
    let count = run_at_target(
        target,
        [
            "rev-list",
            "--left-right",
            "--count",
            &format!("{base_ref}...{sha}"),
        ],
    )?;
    let (behind, ahead) = count
        .stdout
        .trim()
        .split_once(|c: char| c.is_whitespace())
        .map(|(b, a)| (b.trim().parse().unwrap_or(0), a.trim().parse().unwrap_or(0)))
        .unwrap_or((0, 0));

    let diff = run_at_target(target, ["diff", "--name-status", &format!("{base_ref}...{sha}")])?;
    let mut changed_files = Vec::new();
    if diff.ok() {
        for cl in diff.stdout.lines() {
            if cl.is_empty() {
                continue;
            }
            let mut fields = cl.split('\t');
            let kind = fields.next().unwrap_or("").to_string();
            let path = fields.next().unwrap_or("").to_string();
            if (kind.starts_with('R') || kind.starts_with('C')) && fields.next().is_some() {
                // 이름변경/복사 — 세 번째 필드(옛 경로)는 버리고 새 경로만.
            }
            if !path.is_empty() {
                changed_files.push(ChangedPath {
                    path: crate::git::unquote_git_path(&path),
                    kind,
                });
            }
        }
    }
    Ok((ahead, behind, changed_files))
}

/// 병합 탭 대기열 — 열린 요청 각각에 카드 렌더링용 스냅샷을 얹는다.
pub fn list_requested_merges(
    target: &Target,
    remote: &str,
    base: &str,
) -> AppResult<Vec<RequestedMerge>> {
    let requests = list_requests(target, remote, base)?;
    let base_ref = format!("{remote}/{base}");
    let mut out = Vec::new();
    for req in requests {
        let branch_exists = verify_ref(target, &format!("{remote}/{}", req.branch))?.is_some();
        let (ahead, behind, changed_files) = snapshot(target, &base_ref, &req.sha)?;
        out.push(RequestedMerge {
            request: req,
            ahead,
            behind,
            changed_files,
            branch_exists,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ref_path_is_per_base_per_branch() {
        assert_eq!(
            mr_ref_path("main", "feature/login"),
            "refs/gc-mr/main/feature/login"
        );
        // 병합 대상에도 슬래시가 들어갈 수 있다 (release/1.0).
        assert_eq!(
            mr_ref_path("release/1.0", "fix/a"),
            "refs/gc-mr/release/1.0/fix/a"
        );
        assert_eq!(
            ref_branch("main", "refs/gc-mr/main/feature/login"),
            Some("feature/login".into())
        );
        assert_eq!(ref_branch("main", "refs/gc-mr/develop/x"), None);
    }
}
