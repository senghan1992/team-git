//! 병합 탭 상단의 "최근 N일 병합 흐름" 데이터.
//!
//! base 브랜치의 최근 이력에서 (1) 어떤 브랜치가 언제 병합됐고 무엇이
//! 담겼는지, (2) 아직 병합되지 않고 열려 있는 팀원 브랜치는 무엇인지,
//! (3) base 에 직접 쌓인 커밋은 무엇인지를 한 번에 계산한다.
//! UI 는 이 구조를 그대로 타임라인(가로축=시간)으로 그린다.
//!
//! git 호출은 두 번뿐이다 — base 이력 한 번, 미병합 원격 브랜치 한 번.
//! 파싱된 레코드에서 타임라인을 만드는 `build_timeline` 은 순수 함수라
//! git 없이 단위 테스트한다.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::error::AppResult;
use crate::git::{run_at_target, Target};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TimelineCommit {
    pub sha: String,
    pub subject: String,
    pub author: String,
    /// RFC3339 작성일(author date).
    pub date: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineMerge {
    pub sha: String,
    /// RFC3339 — 병합 커밋의 커밋일(committer date). 타임라인의 합류 지점.
    pub date: String,
    pub author: String,
    pub subject: String,
    /// 병합 커밋 제목에서 복원한 브랜치 이름 (컨벤션을 못 읽으면 None).
    pub branch: Option<String>,
    /// 이 병합으로 들어온 커밋들 (최신순).
    pub commits: Vec<TimelineCommit>,
    /// 이 병합으로 바뀐 파일 (정렬·중복 제거).
    pub files: Vec<String>,
    /// 들어온 커밋 중 가장 이른 작성일 — 레인의 시작점.
    pub first_commit_date: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineOpenBranch {
    /// `origin/` 을 뗀 짧은 이름.
    pub name: String,
    pub commits: Vec<TimelineCommit>,
    pub files: Vec<String>,
    pub first_date: String,
    pub last_date: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeTimeline {
    pub base: String,
    pub since: String,
    pub until: String,
    /// base 로 들어온 병합들 (병합일 오름차순).
    pub merges: Vec<TimelineMerge>,
    /// base 에 직접 쌓인(병합 아닌) 커밋들.
    pub direct: Vec<TimelineCommit>,
    /// 아직 base 에 병합되지 않은 원격 브랜치들 (최근 활동 내림차순).
    pub open: Vec<TimelineOpenBranch>,
}

/// `git log` 한 블록( `%x1e` 구분 )을 그대로 옮긴 레코드.
#[derive(Debug, Clone)]
pub struct LogRecord {
    pub sha: String,
    pub parents: Vec<String>,
    pub author: String,
    /// RFC3339 author date (%aI).
    pub author_date: String,
    /// RFC3339 committer date (%cI).
    pub commit_date: String,
    pub subject: String,
    /// `--source` 의 %S — 미병합 브랜치 조회에서만 채워진다.
    pub source: String,
    pub files: Vec<String>,
}

/// `%x1e%H%x1f%P%x1f%an%x1f%aI%x1f%cI%x1f%s(%x1f%S)` + `--name-only` 출력 파싱.
///
/// 제목(%s)에는 0x1f 가 들어 있을 수 있다 — 고정 필드 5개(with_source 면
/// 마지막 %S 까지)를 양끝에서 세고 가운데 전부를 제목으로 되붙인다
/// (log.rs `parse_log` 와 같은 규칙). 깨진 블록은 버리고 계속한다 —
/// 커밋 하나 때문에 타임라인 전체가 죽으면 안 된다.
pub fn parse_blocks(out: &str, with_source: bool) -> Vec<LogRecord> {
    let mut recs = Vec::new();
    for block in out.split('\u{1e}') {
        let block = block.trim_start_matches('\n');
        if block.trim().is_empty() {
            continue;
        }
        let mut lines = block.lines();
        let Some(head) = lines.next() else { continue };
        let parts: Vec<&str> = head.split('\u{1f}').collect();
        let min = if with_source { 7 } else { 6 };
        if parts.len() < min {
            continue;
        }
        let sha = parts[0].trim().to_string();
        if sha.is_empty() {
            continue;
        }
        let (subject, source) = if with_source {
            let n = parts.len();
            (parts[5..n - 1].join("\u{1f}"), parts[n - 1].to_string())
        } else {
            (parts[5..].join("\u{1f}"), String::new())
        };
        recs.push(LogRecord {
            sha,
            parents: parts[1].split_whitespace().map(|s| s.to_string()).collect(),
            author: parts[2].to_string(),
            author_date: parts[3].to_string(),
            commit_date: parts[4].to_string(),
            subject,
            source,
            files: lines
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .map(|s| s.to_string())
                .collect(),
        });
    }
    recs
}

/// 병합 커밋 제목에서 브랜치 이름 복원.
///
/// 이 앱의 컨벤션 `"<branch> 브렌치 병합"`(과거 표기 "브랜치")과 git 기본
/// 문구(`Merge branch 'x'`, `Merge remote-tracking branch 'origin/x'`,
/// `Merge branch 'x' of <url>`)를 안다. 모르는 문구면 None — 제목을 그대로
/// 보여 주는 편이 틀린 이름보다 낫다.
pub fn branch_from_subject(subject: &str) -> Option<String> {
    let s = subject.trim();
    for marker in [" 브렌치 병합", " 브랜치 병합"] {
        if let Some(idx) = s.find(marker) {
            let name = s[..idx].trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    if let Some(rest) = s.strip_prefix("Merge remote-tracking branch '") {
        if let Some(end) = rest.find('\'') {
            let name = rest[..end].strip_prefix("origin/").unwrap_or(&rest[..end]);
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    if let Some(rest) = s.strip_prefix("Merge branch '") {
        if let Some(end) = rest.find('\'') {
            let name = &rest[..end];
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

/// RFC3339 → epoch millis. 못 읽으면 0 — 정렬·비교에서 "아주 옛날" 취급.
fn ts(s: &str) -> i64 {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.timestamp_millis())
        .unwrap_or(0)
}

fn as_commit(r: &LogRecord) -> TimelineCommit {
    TimelineCommit {
        sha: r.sha.clone(),
        subject: r.subject.clone(),
        author: r.author.clone(),
        date: r.author_date.clone(),
    }
}

/// 파싱된 레코드에서 타임라인을 만든다 (순수 — git 없이 테스트 가능).
///
/// `tip`: base 의 tip sha. `git log` 첫 줄은 커밋 시각이 뒤틀린 저장소에서
/// tip 이 아닐 수 있어 명시적으로 받는다. None 이면 첫 레코드를 쓴다.
pub fn build_timeline(
    base: &str,
    since: DateTime<Utc>,
    until: DateTime<Utc>,
    tip: Option<&str>,
    history: &[LogRecord],
    open_records: &[LogRecord],
) -> MergeTimeline {
    use std::collections::{HashMap, HashSet};

    let since_ms = since.timestamp_millis();
    let map: HashMap<&str, &LogRecord> = history.iter().map(|r| (r.sha.as_str(), r)).collect();

    // base 의 first-parent 사슬 — "base 그 자체"의 역사. 병합 커밋의 두 번째
    // 부모 쪽만 브랜치 작업으로 친다.
    let mut chain: Vec<&LogRecord> = Vec::new();
    let mut chain_set: HashSet<&str> = HashSet::new();
    let start = tip
        .and_then(|t| map.get(t).copied())
        .or_else(|| history.first());
    let mut cur = start;
    while let Some(rec) = cur {
        if !chain_set.insert(rec.sha.as_str()) {
            break; // 순환 방어
        }
        chain.push(rec);
        cur = rec.parents.first().and_then(|p| map.get(p.as_str()).copied());
    }

    // 오래된 병합부터 커밋을 귀속시킨다 — 나중 브랜치가 이전 병합의 커밋을
    // 가로채지 않도록. (창 밖의 옛 병합도 귀속만은 수행한다.)
    let mut chain_merges: Vec<&LogRecord> =
        chain.iter().copied().filter(|r| r.parents.len() >= 2).collect();
    chain_merges.sort_by_key(|r| ts(&r.commit_date));

    let mut assigned: HashSet<&str> = HashSet::new();
    let mut merges: Vec<TimelineMerge> = Vec::new();
    for m in chain_merges {
        let mut commits: Vec<TimelineCommit> = Vec::new();
        let mut files: Vec<String> = Vec::new();
        let mut stack: Vec<&str> = m.parents.iter().skip(1).map(|s| s.as_str()).collect();
        while let Some(p) = stack.pop() {
            if chain_set.contains(p) || assigned.contains(p) {
                continue;
            }
            let Some(rec) = map.get(p).copied() else { continue };
            assigned.insert(p);
            commits.push(as_commit(rec));
            files.extend(rec.files.iter().cloned());
            stack.extend(rec.parents.iter().map(|s| s.as_str()));
        }
        if ts(&m.commit_date) < since_ms {
            continue; // 창 밖 — 귀속만 하고 결과에는 넣지 않는다.
        }
        commits.sort_by_key(|c| std::cmp::Reverse(ts(&c.date)));
        files.sort();
        files.dedup();
        let first_commit_date = commits
            .iter()
            .min_by_key(|c| ts(&c.date))
            .map(|c| c.date.clone());
        merges.push(TimelineMerge {
            sha: m.sha.clone(),
            date: m.commit_date.clone(),
            author: m.author.clone(),
            subject: m.subject.clone(),
            branch: branch_from_subject(&m.subject),
            commits,
            files,
            first_commit_date,
        });
    }
    merges.sort_by_key(|m| ts(&m.date));

    let direct: Vec<TimelineCommit> = chain
        .iter()
        .filter(|r| r.parents.len() < 2 && ts(&r.commit_date) >= since_ms)
        .map(|r| as_commit(r))
        .collect();

    // ── 미병합 원격 브랜치 — %S(도달 ref) 로 묶는다 ─────────────────────────
    let mut by_name: HashMap<String, Vec<&LogRecord>> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for r in open_records {
        let name = r
            .source
            .strip_prefix("refs/remotes/")
            .map(|s| s.splitn(2, '/').nth(1).unwrap_or(s))
            .unwrap_or(&r.source)
            .to_string();
        if name.is_empty() || name == "HEAD" || name == base {
            continue;
        }
        if !by_name.contains_key(&name) {
            order.push(name.clone());
        }
        by_name.entry(name).or_default().push(r);
    }
    let mut open: Vec<TimelineOpenBranch> = Vec::new();
    for name in order {
        let recs = &by_name[&name];
        let mut commits: Vec<TimelineCommit> = recs.iter().map(|r| as_commit(r)).collect();
        commits.sort_by_key(|c| std::cmp::Reverse(ts(&c.date)));
        let mut files: Vec<String> = recs.iter().flat_map(|r| r.files.iter().cloned()).collect();
        files.sort();
        files.dedup();
        let first_date = commits
            .iter()
            .min_by_key(|c| ts(&c.date))
            .map(|c| c.date.clone())
            .unwrap_or_default();
        let last_date = commits
            .iter()
            .max_by_key(|c| ts(&c.date))
            .map(|c| c.date.clone())
            .unwrap_or_default();
        open.push(TimelineOpenBranch {
            name,
            commits,
            files,
            first_date,
            last_date,
        });
    }
    open.sort_by_key(|b| std::cmp::Reverse(ts(&b.last_date)));

    MergeTimeline {
        base: base.to_string(),
        since: since.to_rfc3339(),
        until: until.to_rfc3339(),
        merges,
        direct,
        open,
    }
}

fn verify_ref(target: &Target, r: &str) -> bool {
    run_at_target(target, ["rev-parse", "--verify", "--quiet", r])
        .map(|o| o.ok())
        .unwrap_or(false)
}

/// base 최근 `days`일의 병합 흐름. base 를 찾을 수 없으면(빈 저장소·오타)
/// 빈 타임라인 — 병합 탭이 이것 때문에 죽으면 안 된다.
pub fn merge_timeline(
    target: &Target,
    remote: &str,
    base: &str,
    days: u32,
) -> AppResult<MergeTimeline> {
    let until = Utc::now();
    let since = until - Duration::days(days.max(1) as i64);
    let empty = || MergeTimeline {
        base: base.to_string(),
        since: since.to_rfc3339(),
        until: until.to_rfc3339(),
        merges: vec![],
        direct: vec![],
        open: vec![],
    };

    // 원격 추적 ref 우선 — 팀의 진실은 origin 이다. 없으면 로컬 브랜치.
    let remote_ref = format!("refs/remotes/{remote}/{base}");
    let local_ref = format!("refs/heads/{base}");
    let base_ref = if verify_ref(target, &remote_ref) {
        remote_ref
    } else if verify_ref(target, &local_ref) {
        local_ref
    } else {
        return Ok(empty());
    };
    let tip = run_at_target(target, ["rev-parse", &base_ref])
        .ok()
        .filter(|o| o.ok())
        .map(|o| o.stdout.trim().to_string());

    // 창보다 14일 더 읽는다 — 창 안의 병합에 담긴 브랜치 커밋의 작성일이
    // 창보다 오래됐어도 귀속(레인 시작점)할 수 있게.
    let hist_out = run_at_target(
        target,
        [
            "log",
            base_ref.as_str(),
            &format!("--since={}.days", days as u64 + 14),
            "--date=iso-strict",
            "--name-only",
            "--format=%x1e%H%x1f%P%x1f%an%x1f%aI%x1f%cI%x1f%s",
        ],
    )?;
    let history = if hist_out.ok() {
        parse_blocks(&hist_out.stdout, false)
    } else {
        vec![]
    };

    // 아직 base 에 없는 원격 브랜치들. --glob 이 아무것도 못 잡으면 git 이
    // 실패할 수 있다(원격 없음 등) — 그때는 열린 브랜치 없음으로 계속한다.
    let open_out = run_at_target(
        target,
        [
            "log",
            &format!("--glob=refs/remotes/{remote}/*"),
            "--not",
            base_ref.as_str(),
            &format!("--since={}.days", days.max(1)),
            "--source",
            "--date=iso-strict",
            "--name-only",
            "--format=%x1e%H%x1f%P%x1f%an%x1f%aI%x1f%cI%x1f%s%x1f%S",
        ],
    );
    let open_records = match open_out {
        Ok(o) if o.ok() => parse_blocks(&o.stdout, true),
        _ => vec![],
    };

    Ok(build_timeline(
        base,
        since,
        until,
        tip.as_deref(),
        &history,
        &open_records,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(
        sha: &str,
        parents: &[&str],
        author_date: &str,
        commit_date: &str,
        subject: &str,
        source: &str,
        files: &[&str],
    ) -> LogRecord {
        LogRecord {
            sha: sha.to_string(),
            parents: parents.iter().map(|s| s.to_string()).collect(),
            author: "tester".into(),
            author_date: author_date.to_string(),
            commit_date: commit_date.to_string(),
            subject: subject.to_string(),
            source: source.to_string(),
            files: files.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn day(d: u32) -> String {
        format!("2026-09-{d:02}T12:00:00+09:00")
    }

    fn window() -> (DateTime<Utc>, DateTime<Utc>) {
        (
            DateTime::parse_from_rfc3339("2026-09-01T00:00:00+09:00")
                .unwrap()
                .with_timezone(&Utc),
            DateTime::parse_from_rfc3339("2026-09-08T00:00:00+09:00")
                .unwrap()
                .with_timezone(&Utc),
        )
    }

    #[test]
    fn branch_from_subject_knows_the_conventions() {
        assert_eq!(
            branch_from_subject("feature/login 브렌치 병합"),
            Some("feature/login".into())
        );
        assert_eq!(
            branch_from_subject("fix/nav 브랜치 병합"),
            Some("fix/nav".into())
        );
        assert_eq!(
            branch_from_subject("Merge branch 'feature/pay'"),
            Some("feature/pay".into())
        );
        assert_eq!(
            branch_from_subject("Merge branch 'hotfix' of https://x.example/repo"),
            Some("hotfix".into())
        );
        assert_eq!(
            branch_from_subject("Merge remote-tracking branch 'origin/dev-a'"),
            Some("dev-a".into())
        );
        assert_eq!(branch_from_subject("feat: 일반 커밋"), None);
        assert_eq!(branch_from_subject(" 브렌치 병합"), None);
    }

    #[test]
    fn parse_blocks_handles_files_and_separator_in_subject() {
        let out = format!(
            "\u{1e}{sha1}\u{1f}{p}\u{1f}Alice\u{1f}{d}\u{1f}{d}\u{1f}weird\u{1f}subject\n\na.txt\nb/c.txt\n\u{1e}{sha2}\u{1f}\u{1f}Bob\u{1f}{d}\u{1f}{d}\u{1f}initial\n",
            sha1 = "a".repeat(40),
            sha2 = "b".repeat(40),
            p = "b".repeat(40),
            d = "2026-09-02T10:00:00+09:00",
        );
        let recs = parse_blocks(&out, false);
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].subject, "weird\u{1f}subject");
        assert_eq!(recs[0].files, vec!["a.txt", "b/c.txt"]);
        assert_eq!(recs[0].parents.len(), 1);
        assert!(recs[1].parents.is_empty());
        assert!(recs[1].files.is_empty());
    }

    #[test]
    fn parse_blocks_with_source_field() {
        let out = format!(
            "\u{1e}{sha}\u{1f}\u{1f}Bob\u{1f}{d}\u{1f}{d}\u{1f}wip: x\u{1f}refs/remotes/origin/feature/wip\nf.txt\n",
            sha = "c".repeat(40),
            d = "2026-09-05T10:00:00+09:00",
        );
        let recs = parse_blocks(&out, true);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].source, "refs/remotes/origin/feature/wip");
        assert_eq!(recs[0].subject, "wip: x");
    }

    #[test]
    fn build_attributes_merge_commits_and_unions_files() {
        // main: init(1일) ← direct(2일) ← merge(3일, 두 번째 부모 = f2)
        // 브랜치: f1(작성 8/30) ← f2 — 둘 다 병합으로 귀속되어야 한다.
        let (since, until) = window();
        let history = vec![
            rec("m3", &["d2", "f2"], &day(3), &day(3), "feature/login 브렌치 병합", "", &[]),
            rec("f2", &["f1"], &day(2), &day(2), "feat: 2", "", &["src/a.ts", "src/b.ts"]),
            rec("d2", &["i1"], &day(2), &day(2), "chore: direct", "", &["README.md"]),
            rec("f1", &["i1"], "2026-08-30T12:00:00+09:00", &day(1), "feat: 1", "", &["src/a.ts"]),
            rec("i1", &[], "2026-08-20T12:00:00+09:00", "2026-08-20T12:00:00+09:00", "init", "", &["init.txt"]),
        ];
        let tl = build_timeline("main", since, until, Some("m3"), &history, &[]);
        assert_eq!(tl.merges.len(), 1);
        let m = &tl.merges[0];
        assert_eq!(m.branch.as_deref(), Some("feature/login"));
        assert_eq!(m.commits.len(), 2);
        assert_eq!(m.files, vec!["src/a.ts", "src/b.ts"]);
        assert_eq!(m.first_commit_date.as_deref(), Some("2026-08-30T12:00:00+09:00"));
        // direct 에는 창 안의 비병합 사슬 커밋만 — init 은 창 밖.
        assert_eq!(tl.direct.len(), 1);
        assert_eq!(tl.direct[0].sha, "d2");
    }

    #[test]
    fn older_merge_keeps_its_commits_from_newer_ones() {
        // 옛 병합(창 밖)이 먼저 f1 을 귀속시켜, 창 안의 새 병합이 f1 을
        // 가로채지 못한다.
        let (since, until) = window();
        let history = vec![
            rec("m2", &["m1", "g1"], &day(4), &day(4), "b 브렌치 병합", "", &[]),
            rec("g1", &["f1"], &day(3), &day(3), "g", "", &["g.txt"]),
            rec("m1", &["i1", "f1"], "2026-08-25T12:00:00+09:00", "2026-08-25T12:00:00+09:00", "a 브렌치 병합", "", &[]),
            rec("f1", &["i1"], "2026-08-24T12:00:00+09:00", "2026-08-24T12:00:00+09:00", "f", "", &["f.txt"]),
            rec("i1", &[], "2026-08-20T12:00:00+09:00", "2026-08-20T12:00:00+09:00", "init", "", &[]),
        ];
        let tl = build_timeline("main", since, until, Some("m2"), &history, &[]);
        assert_eq!(tl.merges.len(), 1, "창 밖의 옛 병합은 결과에 없다");
        let m = &tl.merges[0];
        assert_eq!(m.commits.iter().map(|c| c.sha.as_str()).collect::<Vec<_>>(), vec!["g1"]);
        assert_eq!(m.files, vec!["g.txt"]);
    }

    #[test]
    fn open_branches_group_by_source_and_skip_head_and_base() {
        let (since, until) = window();
        let open = vec![
            rec("w2", &["w1"], &day(5), &day(5), "wip 2", "refs/remotes/origin/feature/wip", &["w.txt"]),
            rec("w1", &[], &day(4), &day(4), "wip 1", "refs/remotes/origin/feature/wip", &["w.txt", "v.txt"]),
            rec("h1", &[], &day(4), &day(4), "x", "refs/remotes/origin/HEAD", &[]),
            rec("b1", &[], &day(4), &day(4), "y", "refs/remotes/origin/main", &[]),
        ];
        let tl = build_timeline("main", since, until, None, &[], &open);
        assert_eq!(tl.open.len(), 1);
        let b = &tl.open[0];
        assert_eq!(b.name, "feature/wip");
        assert_eq!(b.commits.len(), 2);
        assert_eq!(b.commits[0].sha, "w2", "최신 커밋이 먼저");
        assert_eq!(b.files, vec!["v.txt", "w.txt"]);
        assert_eq!(b.first_date, day(4));
        assert_eq!(b.last_date, day(5));
    }

    #[test]
    fn empty_inputs_produce_empty_timeline() {
        let (since, until) = window();
        let tl = build_timeline("main", since, until, None, &[], &[]);
        assert!(tl.merges.is_empty() && tl.direct.is_empty() && tl.open.is_empty());
        assert_eq!(tl.base, "main");
    }
}
