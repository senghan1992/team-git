//! One-click automatic merge resolution ("AI 자동 병합").
//!
//! End-to-end flow for a merge that crashed into conflicts (or that was
//! started from the Merge Center): for every conflicted file, ask an
//! AI resolver (or fall back to a deterministic rule), write the result,
//! verify no conflict markers remain, stage it, then — when every file is
//! clean — commit the merge with a descriptive message.
//!
//! Safety guarantees (see also merge.rs):
//! - Nothing is ever committed while conflict markers remain in a file.
//! - A local backup of every conflicted working file is written to the app
//!   config dir *before* resolution; `restore_backup` can bring it back.
//! - If resolution or the final commit fails, `MERGE_HEAD` and the staged
//!   states are left untouched so the user can finish in the Merge Center.

use serde::Serialize;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::config_store;
use crate::error::{AppError, AppResult};
use crate::git::merge::{self, ConflictDetail};
use crate::git::{run_at_target, Target};

// 충돌 마커 판정은 merge::has_unresolved_markers 로 통일한다 — 줄 첫머리의
// 시작/종료/베이스 마커만 본다. (`=======` 단독 줄은 마크다운 setext 제목
// 밑줄 같은 정당한 내용일 수 있어, substring 검사는 그런 파일의 AI 병합을
// 영구히 막았다. git 마커는 항상 <<<<<<< / >>>>>>> 와 함께 나타난다.)

// ── Types ───────────────────────────────────────────────────────────────────

/// Deterministic side selection for files the AI cannot (or must not) handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideChoice {
    Ours,
    Theirs,
}

impl SideChoice {
    pub fn as_str(&self) -> &'static str {
        match self {
            SideChoice::Ours => "ours",
            SideChoice::Theirs => "theirs",
        }
    }
}

impl std::str::FromStr for SideChoice {
    type Err = AppError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ours" => Ok(SideChoice::Ours),
            "theirs" => Ok(SideChoice::Theirs),
            _ => Err(AppError::Config(format!(
                "알 수 없는 선택: {s} (ours 또는 theirs)"
            ))),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AutoResolveOptions {
    /// Side used for binary / oversized files, where no text merge exists
    /// (default: theirs).
    pub binary_strategy: SideChoice,
    /// What to do with a **text** file when the AI produces nothing usable and
    /// *both* sides changed it relative to base.
    ///
    /// `None` (the safe default when AI is enabled): leave the file conflicted
    /// so a person resolves it. Picking a whole side there silently throws away
    /// a teammate's committed work and then pushes it — worse than stopping.
    ///
    /// `Some(side)`: pick that side. Used when the user is knowingly running
    /// the rule-based mode (AI turned off) and asked for exactly this.
    pub text_fallback: Option<SideChoice>,
}

impl Default for AutoResolveOptions {
    fn default() -> Self {
        AutoResolveOptions {
            binary_strategy: SideChoice::Theirs,
            text_fallback: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileResolution {
    pub path: String,
    /// "ai" | "ours" | "theirs", or "skipped" for a `remaining_reasons` entry.
    pub method: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoResolveReport {
    pub resolved: Vec<FileResolution>,
    pub remaining: Vec<String>,
    /// Why each still-conflicted file was not resolved, in `remaining` order.
    /// Without this the user only sees "N개를 해결하지 못했습니다" and has to
    /// guess whether the resolver crashed or deliberately stepped back.
    pub remaining_reasons: Vec<FileResolution>,
    /// True when every conflict was resolved AND the merge was committed.
    pub committed: bool,
    /// Local backup id of the pre-resolution working files (None if empty).
    pub backup_id: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupEntry {
    pub id: String,
    pub created_at: String,
    pub files: Vec<String>,
}

// ── Entry point ─────────────────────────────────────────────────────────────

/// Resolve every currently-conflicted file and, when all are clean, commit
/// the merge. `resolve_with_ai` is called per text file with its conflict
/// context; a disabled/failing/empty AI result falls back to a deterministic
/// side selection.
pub fn auto_resolve_merge<F>(
    target: &Target,
    opts: &AutoResolveOptions,
    resolve_with_ai: F,
) -> AppResult<AutoResolveReport>
where
    F: Fn(&ConflictDetail) -> AppResult<String>,
{
    let remaining = merge::remaining_conflicts(target)?;
    let in_progress = merge::merge_in_progress(target)?;
    if remaining.is_empty() {
        if in_progress {
            return Ok(AutoResolveReport {
                resolved: vec![],
                remaining: vec![],
                remaining_reasons: vec![],
                committed: false,
                backup_id: None,
                message: "해결할 충돌 파일이 없습니다. ‘병합 완료’를 눌러 병합을 마무리하세요."
                    .into(),
            });
        }
        return Err(AppError::Git(
            "진행 중인 병합이 없습니다. 병합 센터에서 먼저 병합을 시작하세요.".into(),
        ));
    }

    // Safety net step 1: back up the conflicted working files locally before
    // touching anything. Empty files are skipped (nothing to preserve).
    let backup_dir = backup_root(target)?.join(local_backup_id());
    let mut backed_up: Vec<String> = Vec::new();
    for path in &remaining {
        let Ok(bytes) = crate::git::read_file_at_target(target, path) else {
            continue;
        };
        if bytes.is_empty() {
            continue;
        }
        let dst = backup_dir.join(path);
        std::fs::create_dir_all(dst.parent().unwrap_or(&backup_dir))
            .map_err(|e| AppError::Io(format!("백업 폴더 생성 실패 ({path}): {e}")))?;
        std::fs::write(&dst, &bytes)
            .map_err(|e| AppError::Io(format!("백업 실패 ({path}): {e}")))?;
        backed_up.push(path.clone());
    }
    let backup_id = (!backed_up.is_empty()).then(|| {
        backup_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    });

    // Resolve file by file. Failures never abort the whole run — the file is
    // just skipped so it stays in `remaining` for the Merge Center.
    let mut resolved: Vec<FileResolution> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut notes: Vec<(String, String)> = Vec::new();
    for path in &remaining {
        let detail = match merge::conflict_detail(target, path) {
            Ok(d) => d,
            Err(e) => {
                notes.push((path.clone(), format!("충돌 정보 조회 실패: {e}")));
                skipped.push(path.clone());
                continue;
            }
        };
        if detail.is_binary || detail.too_large {
            // No text merge possible — pick a side. Skipped (never fatal) on error.
            match resolve_side(target, path, opts.binary_strategy) {
                Ok(()) => resolved.push(FileResolution {
                    path: path.clone(),
                    method: opts.binary_strategy.as_str().to_string(),
                    note: Some(
                        "바이너리/대용량 파일 — 내용 확인 후 병합 탭에서 검토하세요.".to_string(),
                    ),
                }),
                Err(e) => {
                    notes.push((path.clone(), format!("측면 선택 실패: {e}")));
                    skipped.push(path.clone());
                }
            }
            continue;
        }
        // Text file: try AI first, then the deterministic fallback.
        match resolve_with_ai(&detail) {
            Ok(text) if valid_ai_body(&text) => {
                let clean = strip_code_fence(&text);
                match write_and_stage(target, path, clean.as_bytes()) {
                    Ok(()) => resolved.push(FileResolution {
                        path: path.clone(),
                        method: "ai".to_string(),
                        note: None,
                    }),
                    Err(e) => {
                        notes.push((path.clone(), format!("AI 결과 저장 실패: {e}")));
                        skipped.push(path.clone());
                    }
                }
            }
            Ok(_) | Err(_) => {
                // AI를 못 썼다. 한쪽만 base에서 바뀐 파일은 그쪽을 고르는 게
                // 곧 정답이므로 그대로 해결한다. 양쪽이 모두 바뀐 파일은
                // 통째로 한쪽을 고르면 팀원의 커밋이 조용히 사라지므로,
                // `text_fallback`이 명시되지 않았으면 사람에게 넘긴다.
                let side = one_sided_change(&detail).or(opts.text_fallback);
                let Some(side) = side else {
                    notes.push((
                        path.clone(),
                        "양쪽에서 모두 수정된 파일입니다. AI 결과를 쓸 수 없어 자동으로 한쪽을 고르지 않았습니다 — 병합 탭에서 직접 확인하세요."
                            .to_string(),
                    ));
                    skipped.push(path.clone());
                    continue;
                };
                match resolve_side(target, path, side) {
                    Ok(()) => resolved.push(FileResolution {
                        path: path.clone(),
                        method: side.as_str().to_string(),
                        note: Some(
                            "AI 결과를 사용할 수 없어 규칙 기반으로 선택했습니다.".to_string(),
                        ),
                    }),
                    Err(e) => {
                        notes.push((path.clone(), format!("측면 선택 실패: {e}")));
                        skipped.push(path.clone());
                    }
                }
            }
        }
    }

    // Attach notes to the last resolved entry of the same path when present
    // (keeps per-file info without a separate field).
    for res in resolved.iter_mut() {
        if let Some((_, n)) = notes.iter().find(|(p, _)| *p == res.path) {
            let prev = res.note.as_deref().unwrap_or("");
            res.note = Some(format!("{prev} ({n})").trim().to_string());
        }
    }

    // Commit only when every conflicted file is clean. On commit failure the
    // stages + MERGE_HEAD remain — the user finishes via the Merge Center.
    let remaining_after = merge::remaining_conflicts(target)?;
    let mut committed = false;
    let mut message;
    let total = resolved.len() + skipped.len();
    if remaining_after.is_empty() {
        if merge::merge_in_progress(target)? {
            let branch = merge_head_branch(target);
            let branch = branch.unwrap_or_else(|_| "(병합 대상)".to_string());
            // 팀 컨벤션 커밋 메시지 — "<브랜치> 브렌치 병합" (aos-git과 동일한 문구).
            let short = branch.strip_prefix("origin/").unwrap_or(&branch);
            let commit_msg = format!("{short} 브렌치 병합");
            match merge::complete_merge(target, Some(&commit_msg)) {
                Ok(out) if out.ok => {
                    committed = true;
                    message = format!(
                        "충돌 {total}개를 자동 해결하고 ‘{}’로 커밋했습니다.",
                        commit_msg
                    );
                }
                Ok(out) => {
                    message = format!(
                        "모든 충돌은 해결됐지만 커밋에 실패했습니다: {}. 파일은 스테이징되어 있으니 병합 센터의 ‘병합 완료’로 마무리하세요.",
                        out.message.trim()
                    );
                }
                Err(e) => {
                    message = format!(
                        "모든 충돌은 해결됐지만 커밋에 실패했습니다: {e}. 충돌 전 상태는 백업에 보존되어 있으니 병합 센터의 ‘병합 완료’로 마무리하세요."
                    );
                }
            }
        } else {
            // Merge finished between our pass and the commit check (e.g. the
            // user hit 완료 elsewhere) — nothing lost, just inform.
            message = format!("충돌 {total}개를 해결했습니다. 병합 커밋은 이미 완료된 상태입니다.");
        }
    } else {
        message = format!(
            "충돌 {total}개 중 {skipped}개를 해결하지 못했습니다. 남은 파일은 병합 센터에서 처리하세요.",
            total = total,
            skipped = skipped.len()
        );
    }
    if skipped.is_empty() && !remaining_after.is_empty() {
        // Shouldn't happen (skips mirror remaining), but keep the message honest.
        message = format!("{message} 남은 파일: {}", remaining_after.join(", "));
    }

    // Pair every still-conflicted file with the reason it was left alone.
    let remaining_reasons: Vec<FileResolution> = remaining_after
        .iter()
        .map(|path| FileResolution {
            path: path.clone(),
            method: "skipped".to_string(),
            note: notes
                .iter()
                .find(|(p, _)| p == path)
                .map(|(_, n)| n.clone()),
        })
        .collect();

    Ok(AutoResolveReport {
        resolved,
        remaining: remaining_after,
        remaining_reasons,
        committed,
        backup_id,
        message,
    })
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// 한쪽만 base에서 바뀐 충돌이면 그 바뀐 쪽을 돌려준다.
///
/// 이 경우 "한쪽 선택"은 버리는 것이 아니라 **올바른 병합 결과**다 — 다른 쪽은
/// base와 같으므로 잃는 변경이 없다. (git이 여기서 충돌을 내는 것은 보통 줄
/// 끝/모드 차이나 rename 같은 이유다.)
///
/// 양쪽이 모두 바뀌었으면 `None` — 자동으로 고를 수 있는 정답이 없다.
fn one_sided_change(detail: &ConflictDetail) -> Option<SideChoice> {
    let base = detail.base.as_deref()?;
    if base == detail.theirs.as_str() {
        // 상대는 손대지 않았다 → 내 변경이 결과물.
        Some(SideChoice::Ours)
    } else if base == detail.ours.as_str() {
        // 나는 손대지 않았다 → 상대 변경이 결과물.
        Some(SideChoice::Theirs)
    } else {
        None
    }
}

fn resolve_side(target: &Target, path: &str, side: SideChoice) -> AppResult<()> {
    let flag = format!("--{}", side.as_str());
    let out = run_at_target(target, ["checkout", flag.as_str(), "--", path])?;
    if !out.ok() {
        return Err(AppError::Git(format!(
            "{} 해결 실패: {}",
            side.as_str(),
            out.stderr.trim()
        )));
    }
    stage(target, path)
}

fn write_and_stage(target: &Target, path: &str, content: &[u8]) -> AppResult<()> {
    crate::git::write_file_at_target(target, path, content)?;
    stage(target, path)
}

fn stage(target: &Target, path: &str) -> AppResult<()> {
    let out = run_at_target(target, ["add", "--", path])?;
    if !out.ok() {
        return Err(AppError::Git(format!(
            "staging 실패: {}",
            out.stderr.trim()
        )));
    }
    Ok(())
}

/// Accept AI output only when it is non-empty and free of conflict markers —
/// never auto-commit while markers remain.
fn valid_ai_body(text: &str) -> bool {
    if text.trim().is_empty() {
        return false;
    }
    // 마커 검사는 원문 기준 — trim 하면 첫 줄의 들여쓰기가 벗겨져
    // "    <<<<<<< 예시" 같은 문서 내용이 0열 마커로 오인된다.
    !crate::git::merge::has_unresolved_markers(text)
}

/// Strip a ``` fence the model may have wrapped its answer in.
fn strip_code_fence(text: &str) -> String {
    let mut s = text.trim().to_string();
    if s.starts_with("```") {
        if let Some(pos) = s.find('\n') {
            s = s[pos + 1..].to_string();
        } else {
            s.clear();
        }
    }
    if s.ends_with("```") {
        if let Some(pos) = s.rfind("```") {
            s = s[..pos].to_string();
        }
    }
    s.trim_end().to_string()
}

fn merge_head_branch(target: &Target) -> AppResult<String> {
    // rev-parse --abbrev-ref MERGE_HEAD just echoes the pseudo-ref name, so
    // resolve the tip sha to an actual branch instead.
    let sha = run_at_target(target, ["rev-parse", "MERGE_HEAD"])?;
    let mut name = String::new();
    if sha.ok() {
        let out = run_at_target(
            target,
            [
                "for-each-ref",
                "--points-at",
                sha.stdout.trim(),
                "--format=%(refname:short)",
            ],
        )?;
        if out.ok() {
            for line in out.stdout.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                // prefer the remote branch (our merges target origin/*); else
                // fall back to the first ref (e.g. the local branch).
                if line.starts_with("origin/") {
                    name = line.to_string();
                    break;
                }
                if name.is_empty() {
                    name = line.to_string();
                }
            }
        }
    }
    if name.is_empty() {
        name = "(병합 대상)".into();
    }
    Ok(name)
}

/// `<millis>_<uuid8>` — sortable, collision-free backup ids.
fn local_backup_id() -> String {
    format!(
        "{}_{}",
        chrono::Utc::now().timestamp_millis(),
        &Uuid::new_v4().simple().to_string()[..8]
    )
}

/// Backups live in the app config dir (never inside the repo, so they are
/// never committed; SSH git dirs are remote and cannot be written to).
/// Keyed by a slug of the repo path so different repos stay separate.
/// `GC_BACKUP_DIR` overrides the base dir (used by integration tests).
fn backup_root(target: &Target) -> AppResult<PathBuf> {
    let base = match std::env::var_os("GC_BACKUP_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => config_store::config_dir()?.join("backups"),
    };
    let slug: String = target
        .path()
        .to_string_lossy()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    Ok(base.join(slug))
}

// ── Backup listing / restore ────────────────────────────────────────────────

pub fn list_backups(target: &Target) -> AppResult<Vec<BackupEntry>> {
    let root = backup_root(target)?;
    let mut out = Vec::new();
    if !root.is_dir() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().into_owned();
        let mut files = Vec::new();
        collect_files(&entry.path(), "", &mut files);
        let created_at = id
            .split('_')
            .next()
            .and_then(|m| m.parse::<i64>().ok())
            .and_then(chrono::DateTime::from_timestamp_millis)
            .map(|t| t.to_rfc3339())
            .unwrap_or_else(|| id.clone());
        out.push(BackupEntry {
            id,
            created_at,
            files,
        });
    }
    out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(out)
}

/// Restore a backup: rewrites the stored pre-resolution bytes back into the
/// working tree (unstaged — files appear as modified, never re-conflicted).
pub fn restore_backup(target: &Target, backup_id: &str) -> AppResult<usize> {
    let dir = backup_root(target)?.join(backup_id);
    if !dir.is_dir() {
        return Err(AppError::Config(format!(
            "백업을 찾을 수 없습니다: {backup_id}"
        )));
    }
    let mut files = Vec::new();
    collect_files(&dir, "", &mut files);
    for f in &files {
        let src = dir.join(f);
        let bytes =
            std::fs::read(&src).map_err(|e| AppError::Io(format!("백업 읽기 실패 ({f}): {e}")))?;
        crate::git::write_file_at_target(target, f, &bytes)?;
    }
    Ok(files.len())
}

fn collect_files(dir: &Path, prefix: &str, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            collect_files(&e.path(), &rel, out);
        } else {
            out.push(rel);
        }
    }
}

// ── Unit tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_and_marked_ai_bodies() {
        assert!(!valid_ai_body(""));
        assert!(!valid_ai_body("   \n  "));
        assert!(!valid_ai_body(
            "<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> branch"
        ));
        assert!(!valid_ai_body("prefix\n||||||| base\nmiddle"));
        assert!(valid_ai_body("let a = 1;\nreturn a;"));
        // `=======` 단독/유사 줄은 정당한 내용일 수 있다 (마크다운 setext
        // 제목 밑줄 등). git 마커는 항상 <<<<<<< / >>>>>>> 짝과 함께 오므로
        // 그 짝 없이는 유효한 본문으로 받아들인다 — 예전 substring 검사는
        // 이런 문서의 AI 병합을 영구히 막았다.
        assert!(valid_ai_body("제목\n=======\n본문"));
        assert!(valid_ai_body("=========="));
        assert!(valid_ai_body("got =======  marker"));
        // 들여쓰인 유사 마커도 내용이다 (git 마커는 항상 0열).
        assert!(valid_ai_body("    <<<<<<< sample in docs"));
    }

    #[test]
    fn strips_code_fences() {
        assert_eq!(strip_code_fence("```\ncode\n```"), "code");
        assert_eq!(
            strip_code_fence("```rust\nfn main() {}\n```"),
            "fn main() {}"
        );
        assert_eq!(strip_code_fence("plain text"), "plain text");
        assert_eq!(strip_code_fence("  ```\n  x\n  ```\n"), "  x");
        assert_eq!(strip_code_fence("```"), "");
    }

    fn detail(base: Option<&str>, ours: &str, theirs: &str) -> ConflictDetail {
        ConflictDetail {
            path: "a.txt".into(),
            is_binary: false,
            too_large: false,
            base: base.map(|s| s.to_string()),
            ours: ours.into(),
            working: String::new(),
            theirs: theirs.into(),
        }
    }

    #[test]
    fn one_sided_change_picks_the_side_that_actually_changed() {
        // 상대가 손대지 않았다 → 내 변경이 결과물 (잃는 것 없음).
        assert_eq!(
            one_sided_change(&detail(Some("shared\n"), "new\n", "shared\n")),
            Some(SideChoice::Ours)
        );
        // 내가 손대지 않았다 → 상대 변경이 결과물.
        assert_eq!(
            one_sided_change(&detail(Some("shared\n"), "shared\n", "changed\n")),
            Some(SideChoice::Theirs)
        );
    }

    /// 데이터 손실 방지의 핵심: 양쪽이 모두 고친 파일은 자동으로 한쪽을
    /// 고르지 않는다. 그렇게 하면 팀원의 커밋이 조용히 사라진 채 push된다.
    #[test]
    fn one_sided_change_refuses_to_pick_when_both_sides_changed() {
        assert_eq!(
            one_sided_change(&detail(Some("shared\n"), "mine\n", "theirs\n")),
            None
        );
        // base를 알 수 없으면(add/add 충돌) 역시 고를 수 없다.
        assert_eq!(one_sided_change(&detail(None, "mine\n", "theirs\n")), None);
    }

    #[test]
    fn side_choice_from_str_roundtrip() {
        assert_eq!(SideChoice::Ours.as_str(), "ours");
        assert_eq!(SideChoice::Theirs.as_str(), "theirs");
        assert_eq!("ours".parse::<SideChoice>().unwrap(), SideChoice::Ours);
        assert_eq!("theirs".parse::<SideChoice>().unwrap(), SideChoice::Theirs);
        assert!("bogus".parse::<SideChoice>().is_err());
    }

    #[test]
    fn backup_id_is_sortable() {
        let a = local_backup_id();
        // Ensure a different millisecond timestamp (same-millis ids sort by
        // random suffix, which makes the ordering assertion flaky).
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = local_backup_id();
        assert!(a < b, "millis prefix must sort: {a} vs {b}");
        assert!(a.split('_').count() == 2);
    }

    #[test]
    fn collect_files_walks_nested_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("src")).unwrap();
        std::fs::write(tmp.path().join("a.txt"), "x").unwrap();
        std::fs::write(tmp.path().join("src/b.rs"), "y").unwrap();
        let mut files = Vec::new();
        collect_files(tmp.path(), "", &mut files);
        files.sort();
        assert_eq!(files, vec!["a.txt".to_string(), "src/b.rs".to_string()]);
    }
}
