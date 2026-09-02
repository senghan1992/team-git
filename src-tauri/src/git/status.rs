use serde::{Deserialize, Serialize};

use crate::error::AppResult;
use crate::git::run;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    Untracked,
    Conflicted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    pub kind: FileChangeKind,
    pub path: String,
    /// True when the index (staging area) has a change — X not in {'.', ' ', '?'}.
    pub staged: bool,
    /// True when the work tree has a change — Y not in {'.', ' '}. Untracked files use false.
    pub unstaged: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkingTreeStatus {
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    /// `origin/<병합 브랜치>`가 현재 브랜치보다 앞선 커밋 수. `behind`가
    /// 내 원격 브랜치 기준인 것과 달리, 이 값이 "동기화" 버튼이 실제로
    /// 가져올 커밋 수다. 네트워크를 타지 않고 마지막 fetch 시점의 원격
    /// 트래킹 ref 로 계산한다.
    #[serde(default)]
    pub behind_base: u32,
    pub files: Vec<FileChange>,
}

pub fn status(repo_path: &std::path::Path) -> AppResult<WorkingTreeStatus> {
    let out = run(
        Some(repo_path),
        [
            "status",
            "--porcelain=v2",
            "--branch",
            "--untracked-files=normal",
        ],
    )?;
    parse_status(&out.stdout)
}

/// Public for unit testing.
pub fn parse_status(output: &str) -> AppResult<WorkingTreeStatus> {
    let mut status = WorkingTreeStatus::default();
    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("# branch.head ") {
            let v = rest.trim();
            status.branch = if v == "(detached)" {
                None
            } else {
                Some(v.to_string())
            };
        } else if let Some(rest) = line.strip_prefix("# branch.upstream ") {
            status.upstream = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("# branch.ab ") {
            let mut ahead = 0u32;
            let mut behind = 0u32;
            for tok in rest.split_whitespace() {
                if let Some(v) = tok.strip_prefix('+') {
                    ahead = v.parse().unwrap_or(0);
                } else if let Some(v) = tok.strip_prefix('-') {
                    behind = v.parse().unwrap_or(0);
                }
            }
            status.ahead = ahead;
            status.behind = behind;
        } else if line.starts_with("1 ") || line.starts_with("2 ") {
            // 1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>                    → 9 필드
            // 2 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <X><score> <path>\t<orig> → 10 필드
            // rename(`2 `) 라인은 <X><score> 필드가 하나 더 있다 — 같은 9로
            // 자르면 경로 앞에 "R100 " 이 섞여 나온다.
            let n = if line.starts_with("2 ") { 10 } else { 9 };
            let mut parts: Vec<&str> = line.splitn(n, ' ').collect();
            let _ver = parts.remove(0);
            let xy = parts.remove(0);
            let path = parts.pop().unwrap_or("");
            let (kind, final_path) = classify(xy, path);
            let (staged, unstaged) = parse_xy(xy);
            status.files.push(FileChange {
                kind,
                path: crate::git::unquote_git_path(&final_path),
                staged,
                unstaged,
            });
        } else if line.starts_with("? ") {
            let path = line.strip_prefix("? ").unwrap_or("").trim().to_string();
            // Untracked files: not in index, not in work tree (staged=F, unstaged=F)
            status.files.push(FileChange {
                kind: FileChangeKind::Untracked,
                path: crate::git::unquote_git_path(&path),
                staged: false,
                unstaged: false,
            });
        } else if line.starts_with("u ") {
            // u <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path> → 11 필드
            // (모드 4개 + 스테이지 해시 3개) — 10으로 자르면 경로 앞에
            // stage-3 SHA 40자가 붙어 충돌 중 파일 목록 전체가 깨진다.
            let mut parts: Vec<&str> = line.splitn(11, ' ').collect();
            let _ver = parts.remove(0);
            let xy = parts.remove(0);
            let path = parts.pop().unwrap_or("");
            let (staged, unstaged) = parse_xy(xy);
            status.files.push(FileChange {
                kind: FileChangeKind::Conflicted,
                path: crate::git::unquote_git_path(path),
                staged,
                unstaged,
            });
        }
    }
    Ok(status)
}

/// Parse the two-character XY field from porcelain v2.
fn parse_xy(xy: &str) -> (bool, bool) {
    let x = xy.chars().next().unwrap_or('.');
    let y = xy.chars().nth(1).unwrap_or('.');
    // '.' is the "unchanged" placeholder in porcelain v2.
    let staged = x != '.' && x != ' ' && x != '?';
    let unstaged = y != '.' && y != ' ';
    (staged, unstaged)
}

fn classify(xy: &str, path: &str) -> (FileChangeKind, String) {
    let x = xy.chars().next().unwrap_or(' ');
    let y = xy.chars().nth(1).unwrap_or(' ');
    let path = path.to_string();
    // 인덱스(X)에 변화가 없으면 워크트리(Y) 열로 분류한다 — 탐색기에서
    // 파일을 지운 흔한 상태(`.D`)가 "수정됨"으로 표시되던 원인.
    let effective = if x == '.' || x == ' ' { y } else { x };
    match effective {
        'R' | 'C' => {
            let p = path.split('\t').next().unwrap_or(&path).to_string();
            (
                if effective == 'R' {
                    FileChangeKind::Renamed
                } else {
                    FileChangeKind::Copied
                },
                p,
            )
        }
        'A' => (FileChangeKind::Added, path),
        'D' => (FileChangeKind::Deleted, path),
        _ => (FileChangeKind::Modified, path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_clean_branch() {
        let s = "# branch.head main\n# branch.upstream origin/main\n# branch.ab +2 -1\n";
        let st = parse_status(s).unwrap();
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!(st.ahead, 2);
        assert_eq!(st.behind, 1);
        assert!(st.files.is_empty());
    }

    #[test]
    fn parses_modified_and_untracked() {
        let s = "# branch.head main\n1 .M N... 100644 100644 100644 abc abc src/x.rs\n? new.txt\n";
        let st = parse_status(s).unwrap();
        assert_eq!(st.files.len(), 2);
        assert_eq!(st.files[0].path, "src/x.rs");
        // .M → unstaged only
        assert!(!st.files[0].staged);
        assert!(st.files[0].unstaged);
        assert_eq!(st.files[1].kind, FileChangeKind::Untracked);
        assert!(!st.files[1].staged);
        assert!(!st.files[1].unstaged);
    }

    #[test]
    fn parses_staged_modification() {
        // M. = staged modification only
        let s = "# branch.head main\n1 M. N... 100644 100644 abc abc src/x.rs\n";
        let st = parse_status(s).unwrap();
        assert_eq!(st.files.len(), 1);
        assert!(st.files[0].staged);
        assert!(!st.files[0].unstaged);
    }

    #[test]
    fn parses_both_staged_and_unstaged() {
        // MM = both staged and unstaged
        let s = "# branch.head main\n1 MM N... 100644 100644 abc abc src/x.rs\n";
        let st = parse_status(s).unwrap();
        assert_eq!(st.files.len(), 1);
        assert!(st.files[0].staged);
        assert!(st.files[0].unstaged);
    }
}
