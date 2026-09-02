use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::git::run;

/// Fields per line are separated by ASCII Unit Separator (0x1f) and end with a newline.
/// Format: `%H%x1f%s%x1f%an%x1f%aI%x1f%P`
/// e.g. `abcdef...<US>commit message<US>Author Name<US>2024-01-02T03:04:05+00:00<US>parent1 parent2\n`
const SEP: char = '\x1f';

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Commit {
    pub sha: String,
    pub message: String,
    pub author: String,
    pub date: DateTime<Utc>,
    pub parents: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitsPage {
    pub commits: Vec<Commit>,
    pub total: u32,
    pub page: u32,
    pub per_page: u32,
}

pub fn log(
    repo_path: &std::path::Path,
    branch: &str,
    page: u32,
    per_page: u32,
) -> AppResult<CommitsPage> {
    let skip = page.saturating_sub(1) * per_page;
    let out = run(
        Some(repo_path),
        [
            "log",
            "--date=iso-strict",
            &format!("--pretty=format:%H{SEP}%s{SEP}%an{SEP}%aI{SEP}%P%n"),
            "--skip",
            &skip.to_string(),
            "-n",
            &per_page.to_string(),
            branch,
        ],
    )?;
    let commits = parse_log(&out.stdout)?;
    let total = total_count(repo_path, branch)?;
    Ok(CommitsPage {
        commits,
        total,
        page,
        per_page,
    })
}

fn total_count(repo_path: &std::path::Path, branch: &str) -> AppResult<u32> {
    let out = run(Some(repo_path), ["rev-list", "--count", branch])?;
    let n: u32 = out
        .stdout
        .trim()
        .parse()
        .map_err(|e| AppError::Git(format!("invalid rev-list count: {e}")))?;
    Ok(n)
}

/// Parse `git log --pretty=format:...` output into a `Vec<Commit>`.
pub fn parse_log(output: &str) -> AppResult<Vec<Commit>> {
    let mut commits = Vec::new();
    for line in output.split('\n') {
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split(SEP).collect();
        if parts.len() != 5 {
            return Err(AppError::Git(format!("malformed log line: {line:?}")));
        }
        let sha = parts[0].trim().to_string();
        let message = parts[1].to_string();
        let author = parts[2].to_string();
        let date_str = parts[3];
        let date = DateTime::parse_from_rfc3339(date_str)
            .map_err(|e| AppError::Git(format!("invalid date {date_str}: {e}")))?
            .with_timezone(&Utc);
        let parents: Vec<String> = if parts[4].trim().is_empty() {
            vec![]
        } else {
            parts[4].split_whitespace().map(|s| s.to_string()).collect()
        };
        if sha.is_empty() {
            continue;
        }
        commits.push(Commit {
            sha,
            message,
            author,
            date,
            parents,
        });
    }
    Ok(commits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_commit() {
        let s = format!(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa{SEP}initial{SEP}Alice{SEP}2024-01-02T03:04:05+00:00{SEP}\n"
        );
        let c = parse_log(&s).unwrap();
        assert_eq!(c.len(), 1);
        assert!(c[0].sha.starts_with("aaaa"));
        assert_eq!(c[0].message, "initial");
    }

    #[test]
    fn parses_merge_commit_with_two_parents() {
        let s = format!(
            "1111111111111111111111111111111111111111{SEP}merge feat{SEP}Bob{SEP}2024-01-02T03:04:05+00:00{SEP}2222222222222222222222222222222222222222 3333333333333333333333333333333333333333\n"
        );
        let c = parse_log(&s).unwrap();
        assert_eq!(c[0].parents.len(), 2);
    }

    #[test]
    fn rejects_malformed_line() {
        let s = "garbage\n";
        assert!(parse_log(s).is_err());
    }
}
