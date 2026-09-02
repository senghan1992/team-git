//! Per-repo collaboration config stored as `.gpconfig` at the repository root.
//!
//! Because the file lives **inside** the repo, any collaborator who connects
//! the same project through Git Companion sees the same merge-manager
//! assignments, member list and notification recipients — no backend needed.
//! Members are matched across devices by **email**.
use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::git::{read_file_at_target, run_at_target, write_file_at_target, Target};

pub const GPCONFIG_FILE: &str = ".gpconfig";
pub const GPCONFIG_VERSION: u32 = 2;
const GP_COMMIT_MESSAGE: &str = "chore: update project config (.gpconfig)";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GpMember {
    /// Stable id (uuid) chosen by the app that registered the person.
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub email: String,
    #[serde(default = "default_member_role")]
    pub role: String, // "admin" | "member"
}

fn default_member_role() -> String {
    "member".into()
}

fn default_gp_version() -> u32 {
    GPCONFIG_VERSION
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GpNotifySettings {
    #[serde(default)]
    pub on_branch_ready: bool,
    #[serde(default)]
    pub on_merge_complete: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectConfig {
    #[serde(default = "default_gp_version")]
    pub gpconfig_version: u32,
    #[serde(default)]
    pub default_base_branch: String,
    /// People collaborating on this project.
    #[serde(default)]
    pub members: Vec<GpMember>,
    /// branch name → member email. The listed person is the merge manager for
    /// that branch (commits/pushes on it are their responsibility).
    #[serde(default)]
    pub merge_managers: HashMap<String, String>,
    /// 병합 대상 브랜치 목록 — 이 브랜치들로만 병합할 수 있다.
    /// main 외에도 자유롭게 지정할 수 있고 언제든 바꿔서 다시 커밋할 수 있다.
    /// 비어 있으면 `default_base_branch`만 대상으로 취급한다.
    #[serde(default)]
    pub merge_targets: Vec<String>,
    /// Member emails that should be notified about this project.
    #[serde(default)]
    pub notify_recipients: Vec<String>,
    #[serde(default)]
    pub notify: GpNotifySettings,
}

/// Read `.gpconfig` from the repo. `exists=false` when the file is absent.
pub fn read_config(target: &Target) -> AppResult<(ProjectConfig, bool)> {
    let bytes = read_file_at_target(target, GPCONFIG_FILE);
    let mut data = match bytes {
        Ok(b) if !b.is_empty() => b,
        _ => return Ok((ProjectConfig::default(), false)),
    };
    // Trim trailing whitespace that `cat` may have added.
    while matches!(data.last(), Some(b'\n') | Some(b'\r')) {
        data.pop();
    }
    let cfg: ProjectConfig = serde_json::from_slice(&data)
        .map_err(|e| AppError::Config(format!(".gpconfig 파싱 실패: {e}")))?;
    Ok((normalize(cfg), true))
}

/// Read the **team's** `.gpconfig`, not just whatever the checked-out branch
/// happens to contain.
///
/// `.gpconfig` lives inside the repo, so a member working on their own branch
/// often has no copy of it at all — the branch was cut before the config was
/// committed, or simply hasn't been synced. Reading only the working tree there
/// yields "no config", which the UI reads as "no merge manager assigned, anyone
/// may merge" and shows a team member the manager's job. So when the working
/// tree has no copy, fall back to the merge branch's committed copy
/// (`<remote>/<base>` first, then a local `<base>`).
///
/// The working tree still wins when present, so edits in the 설정 tab stay
/// visible before they are committed.
pub fn read_config_effective(
    target: &Target,
    base_branch: &str,
    remote: &str,
) -> AppResult<(ProjectConfig, bool)> {
    let (cfg, exists) = read_config(target)?;
    if exists {
        return Ok((cfg, true));
    }
    let base = base_branch.trim();
    if base.is_empty() {
        return Ok((cfg, false));
    }
    for rev in [format!("{remote}/{base}"), base.to_string()] {
        let spec = format!("{rev}:{GPCONFIG_FILE}");
        let out = run_at_target(target, ["show", &spec])?;
        if !out.ok() || out.stdout.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<ProjectConfig>(out.stdout.trim()) {
            Ok(parsed) => return Ok((normalize(parsed), true)),
            // 커밋된 설정이 깨져 있으면 조용히 무시하고 다음 후보를 본다 —
            // 설정 파일 하나 때문에 앱 전체가 막히면 안 된다.
            Err(_) => continue,
        }
    }
    Ok((cfg, false))
}

/// Write `.gpconfig` (pretty JSON) into the repo working tree.
pub fn save_config(target: &Target, cfg: &ProjectConfig) -> AppResult<ProjectConfig> {
    let cfg = normalize(cfg.clone());
    let json = serde_json::to_vec_pretty(&cfg)
        .map_err(|e| AppError::Config(format!(".gpconfig 직렬화 실패: {e}")))?;
    write_file_at_target(target, GPCONFIG_FILE, &json)?;
    Ok(cfg)
}

/// `git add .gpconfig && git commit` — makes the config visible to everyone
/// who pulls the repo.
pub fn commit_config(target: &Target) -> AppResult<CommitOutcome> {
    if !run_at_target(target, ["add", "--", GPCONFIG_FILE])?.ok() {
        return Err(AppError::Git("git add .gpconfig 실패".into()));
    }
    let out = run_at_target(target, ["commit", "-m", GP_COMMIT_MESSAGE])?;
    if out.ok() {
        Ok(CommitOutcome {
            ok: true,
            message: out.stdout.trim().to_string(),
        })
    } else {
        let msg = out.stderr.trim();
        if msg.contains("nothing to commit") || msg.contains("no changes added") {
            Ok(CommitOutcome {
                ok: true,
                message: "변경 사항 없음".into(),
            })
        } else {
            Err(AppError::Git(format!("커밋 실패: {msg}")))
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CommitOutcome {
    pub ok: bool,
    pub message: String,
}

/// Make the config consistent:
/// - members deduped by email (first wins)
/// - non-member emails dropped from `merge_managers` and `notify_recipients`
/// - version pinned to the current schema
fn normalize(mut cfg: ProjectConfig) -> ProjectConfig {
    cfg.gpconfig_version = GPCONFIG_VERSION;
    let mut seen: Vec<String> = Vec::new();
    cfg.members.retain(|m| {
        let email = m.email.trim().to_lowercase();
        if email.is_empty() || seen.contains(&email) {
            false
        } else {
            seen.push(email.clone());
            true
        }
    });
    cfg.merge_managers.retain(|_, email| seen.contains(&email));
    cfg.notify_recipients
        .retain(|email| seen.contains(&email.trim().to_lowercase()));
    cfg.notify_recipients.dedup();
    // 병합 대상: 공백 제거 + 순서 유지 중복 제거.
    let mut seen_targets: Vec<String> = Vec::new();
    let mut targets_out: Vec<String> = Vec::new();
    for b in cfg.merge_targets.drain(..) {
        let b = b.trim().to_string();
        if b.is_empty() || seen_targets.contains(&b) {
            continue;
        }
        seen_targets.push(b.clone());
        targets_out.push(b);
    }
    cfg.merge_targets = targets_out;
    cfg
}

/// 병합이 허용되는 대상 브랜치 목록을 돌려준다.
/// `merge_targets`가 비어 있으면 `fallback`(default_base_branch)만 대상이다.
pub fn merge_targets_of(cfg: &ProjectConfig, fallback: &str) -> Vec<String> {
    if !cfg.merge_targets.is_empty() {
        cfg.merge_targets.clone()
    } else {
        vec![fallback.trim().to_string()]
    }
}

/// `branch`가 이 프로젝트의 병합 대상 브랜치인지 판정한다 (순수 함수).
///
/// 우선순위는 `merge_targets` → `default_base_branch` → `registered_default`.
/// `exists`는 `.gpconfig` 파일이 실제로 있는지 — 없으면 `default_base_branch`가
/// 기본값(빈 문자열)이므로 앱에 등록된 기본 브랜치로 판정해야 한다.
///
/// pre-push hook이 "이 푸시가 팀원 전체에게 동기화 알림을 보내야 하는
/// 푸시인가"를 정하는 데 쓴다. 병합 브랜치가 main이 아닌 팀(develop,
/// release/1.0 …)에서도 알림이 정확히 가야 하기 때문에 하드코딩하지 않는다.
pub fn is_merge_target(
    cfg: &ProjectConfig,
    exists: bool,
    registered_default: &str,
    branch: &str,
) -> bool {
    let branch = branch.trim();
    if branch.is_empty() {
        return false;
    }
    let fallback = if exists && !cfg.default_base_branch.trim().is_empty() {
        cfg.default_base_branch.trim().to_string()
    } else {
        registered_default.trim().to_string()
    };
    merge_targets_of(cfg, &fallback).iter().any(|t| t == branch)
}

/// Member lookup helper for the UI layer.
pub fn member_by_email<'a>(cfg: &'a ProjectConfig, email: &str) -> Option<&'a GpMember> {
    let email = email.trim().to_lowercase();
    cfg.members
        .iter()
        .find(|m| m.email.trim().to_lowercase() == email)
}

/// Build a fresh member record for the given account.
pub fn member_from_account(id: &str, name: &str, email: &str, role: &str) -> GpMember {
    GpMember {
        id: if id.is_empty() {
            Uuid::new_v4().to_string()
        } else {
            id.to_string()
        },
        name: name.to_string(),
        email: email.trim().to_lowercase(),
        role: normalize_role(role),
    }
}

pub fn normalize_role(role: &str) -> String {
    if role == "admin" {
        "admin".into()
    } else {
        "member".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_dedupes_members_and_drops_unknown_managers() {
        let cfg = ProjectConfig {
            gpconfig_version: 999,
            default_base_branch: "main".into(),
            members: vec![
                GpMember {
                    id: "a".into(),
                    name: "Alice".into(),
                    email: "a@x".into(),
                    role: "admin".into(),
                },
                GpMember {
                    id: "b".into(),
                    name: "Bob".into(),
                    email: "A@X".into(),
                    role: "member".into(),
                },
                GpMember {
                    id: "c".into(),
                    name: "Carol".into(),
                    email: " ".into(),
                    role: "member".into(),
                },
            ],
            merge_managers: HashMap::from([
                ("feature/x".into(), "a@x".into()),
                ("feature/y".into(), "ghost@x".into()),
            ]),
            notify_recipients: vec!["A@X".into(), "A@X".into(), "ghost@x".into()],
            merge_targets: vec![
                "main".into(),
                "main".into(),
                " release/1.0 ".into(),
                " ".into(),
            ],
            notify: GpNotifySettings::default(),
        };
        let n = normalize(cfg);
        assert_eq!(n.gpconfig_version, GPCONFIG_VERSION);
        assert_eq!(n.members.len(), 1);
        assert_eq!(n.members[0].name, "Alice");
        assert!(n.merge_managers.contains_key("feature/x"));
        assert!(!n.merge_managers.contains_key("feature/y"));
        assert_eq!(n.notify_recipients, vec!["A@X"]);
        assert_eq!(n.merge_targets, vec!["main", "release/1.0"]);
    }

    #[test]
    fn merge_targets_fallback_to_default_base_when_empty() {
        let cfg = ProjectConfig {
            gpconfig_version: 1,
            default_base_branch: "develop".into(),
            members: vec![],
            merge_managers: HashMap::new(),
            merge_targets: vec![],
            notify_recipients: vec![],
            notify: GpNotifySettings::default(),
        };
        assert_eq!(merge_targets_of(&cfg, "develop"), vec!["develop"]);
        let with_targets = ProjectConfig {
            merge_targets: vec!["main".into(), "release/2.0".into()],
            ..cfg.clone()
        };
        assert_eq!(
            merge_targets_of(&with_targets, "develop"),
            vec!["main", "release/2.0"]
        );
    }

    #[test]
    fn is_merge_target_uses_config_then_registered_default() {
        let empty = ProjectConfig::default();
        // .gpconfig가 없으면 앱에 등록된 기본 브랜치만 병합 브랜치다.
        assert!(is_merge_target(&empty, false, "develop", "develop"));
        assert!(!is_merge_target(&empty, false, "develop", "main"));
        assert!(!is_merge_target(&empty, false, "develop", "feature/x"));

        // merge_targets가 있으면 그 목록이 전부다 — main이 없어도 된다.
        let cfg = ProjectConfig {
            default_base_branch: "develop".into(),
            merge_targets: vec!["develop".into(), "release/1.0".into()],
            ..ProjectConfig::default()
        };
        assert!(is_merge_target(&cfg, true, "main", "develop"));
        assert!(is_merge_target(&cfg, true, "main", "release/1.0"));
        assert!(
            !is_merge_target(&cfg, true, "main", "main"),
            "목록에 없는 브랜치는 병합 브랜치가 아니다"
        );

        // merge_targets가 비어 있으면 default_base_branch가 대상.
        let only_base = ProjectConfig {
            default_base_branch: "develop".into(),
            ..ProjectConfig::default()
        };
        assert!(is_merge_target(&only_base, true, "main", "develop"));
        assert!(!is_merge_target(&only_base, true, "main", "main"));

        // 빈 브랜치 이름은 절대 병합 브랜치로 보지 않는다.
        assert!(!is_merge_target(&cfg, true, "main", "  "));
    }

    /// 팀원이 자기 브랜치에 있으면 `.gpconfig` 사본이 없을 수 있다. 그때도
    /// 병합 브랜치에 커밋된 팀 규칙을 읽어야 한다 — 못 읽으면 "관리자 미지정"
    /// 으로 오인되어 팀원에게 관리자 화면이 뜬다.
    #[test]
    fn read_config_effective_falls_back_to_the_merge_branch_copy() {
        fn git(dir: &std::path::Path, args: &[&str]) {
            let out = std::process::Command::new("git")
                .current_dir(dir)
                .env("LC_ALL", "C.UTF-8")
                .args(args)
                .output()
                .expect("git");
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }

        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["config", "user.email", "t@example.com"]);
        git(&repo, &["config", "user.name", "T"]);
        git(&repo, &["config", "commit.gpgsign", "false"]);

        // main 에만 .gpconfig 를 커밋한다.
        std::fs::write(
            repo.join(GPCONFIG_FILE),
            br#"{"default_base_branch":"main","members":[{"id":"1","name":"Lead","email":"lead@x.com","role":"admin"}],"merge_managers":{"main":"lead@x.com"},"merge_targets":["main"]}"#,
        )
        .unwrap();
        std::fs::write(repo.join("README.md"), b"x").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-m", "init"]);

        let target = Target::Local(repo.clone());
        // main 위: 워킹 트리에 파일이 있으므로 그대로 읽힌다.
        let (cfg, exists) = read_config_effective(&target, "main", "origin").unwrap();
        assert!(exists);
        assert_eq!(
            cfg.merge_managers.get("main").map(String::as_str),
            Some("lead@x.com")
        );

        // .gpconfig 가 생기기 전에 갈라진 브랜치를 흉내낸다: 파일을 지운 커밋.
        git(&repo, &["checkout", "-b", "feature/mine"]);
        std::fs::remove_file(repo.join(GPCONFIG_FILE)).unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-m", "no config here"]);

        // 워킹 트리에는 없다 — read_config 는 못 찾는다.
        let (_, plain_exists) = read_config(&target).unwrap();
        assert!(!plain_exists, "워킹 트리에는 .gpconfig 가 없어야 한다");

        // 그래도 main 에 커밋된 규칙은 읽혀야 한다.
        let (cfg, exists) = read_config_effective(&target, "main", "origin").unwrap();
        assert!(exists, "병합 브랜치의 커밋된 설정을 찾아야 한다");
        assert_eq!(
            cfg.merge_managers.get("main").map(String::as_str),
            Some("lead@x.com"),
            "팀원도 병합 관리자가 누구인지 알 수 있어야 한다"
        );
        assert!(is_merge_target(&cfg, exists, "main", "main"));

        // 어디에도 없으면 exists=false (기존 동작 유지).
        let (_, none) = read_config_effective(&target, "does-not-exist", "origin").unwrap();
        assert!(!none);
    }

    #[test]
    fn member_from_account_normalizes_role_and_email() {
        let m = member_from_account("", "홍길동", " HONG@X.COM ", "OWNER");
        assert_eq!(m.email, "hong@x.com");
        assert_eq!(m.role, "member");
        assert!(!m.id.is_empty());
    }
}
