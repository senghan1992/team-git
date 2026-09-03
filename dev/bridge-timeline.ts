// 미리보기(git-bridge)용 merge_timeline 구현 — src-tauri/src/git/timeline.rs
// 와 같은 알고리즘·같은 JSON 계약. 순수 함수라 node 의존이 없고, git 실행은
// 호출자가 넘긴 `run` 콜백(브릿지의 tgGit)이 담당한다.

export interface TimelineCommit {
  sha: string;
  subject: string;
  author: string;
  /** RFC3339 작성일(author date). */
  date: string;
}

export interface TimelineMerge {
  sha: string;
  /** RFC3339 — 병합 커밋의 커밋일. 타임라인의 합류 지점. */
  date: string;
  author: string;
  subject: string;
  branch: string | null;
  commits: TimelineCommit[];
  files: string[];
  first_commit_date: string | null;
}

export interface TimelineOpenBranch {
  name: string;
  commits: TimelineCommit[];
  files: string[];
  first_date: string;
  last_date: string;
}

export interface MergeTimelineData {
  base: string;
  since: string;
  until: string;
  merges: TimelineMerge[];
  direct: TimelineCommit[];
  open: TimelineOpenBranch[];
}

interface RunResult {
  ok: boolean;
  stdout: string;
  stderr: string;
}

interface LogRecord {
  sha: string;
  parents: string[];
  author: string;
  authorDate: string;
  commitDate: string;
  subject: string;
  source: string;
  files: string[];
}

const RS = "\u001e";
const US = "\u001f";

/** `%x1e` 블록 파싱 — 제목의 0x1f 는 고정 필드를 양끝에서 세어 살린다. */
export function parseBlocks(out: string, withSource: boolean): LogRecord[] {
  const recs: LogRecord[] = [];
  for (const rawBlock of out.split(RS)) {
    const block = rawBlock.replace(/^\n+/, "");
    if (!block.trim()) continue;
    const lines = block.split("\n");
    const parts = lines[0].split(US);
    const min = withSource ? 7 : 6;
    if (parts.length < min) continue;
    const sha = parts[0].trim();
    if (!sha) continue;
    let subject: string;
    let source = "";
    if (withSource) {
      source = parts[parts.length - 1];
      subject = parts.slice(5, parts.length - 1).join(US);
    } else {
      subject = parts.slice(5).join(US);
    }
    recs.push({
      sha,
      parents: parts[1].split(/\s+/).filter(Boolean),
      author: parts[2],
      authorDate: parts[3],
      commitDate: parts[4],
      subject,
      source,
      files: lines.slice(1).map((l) => l.trim()).filter(Boolean),
    });
  }
  return recs;
}

/** 병합 커밋 제목에서 브랜치 이름 복원 (Rust `branch_from_subject` 쌍둥이). */
export function branchFromSubject(subject: string): string | null {
  const s = subject.trim();
  for (const marker of [" 브렌치 병합", " 브랜치 병합"]) {
    const idx = s.indexOf(marker);
    if (idx >= 0) {
      const name = s.slice(0, idx).trim();
      if (name) return name;
    }
  }
  const rt = "Merge remote-tracking branch '";
  if (s.startsWith(rt)) {
    const rest = s.slice(rt.length);
    const end = rest.indexOf("'");
    if (end > 0) {
      const raw = rest.slice(0, end);
      const name = raw.startsWith("origin/") ? raw.slice("origin/".length) : raw;
      if (name) return name;
    }
  }
  const mb = "Merge branch '";
  if (s.startsWith(mb)) {
    const rest = s.slice(mb.length);
    const end = rest.indexOf("'");
    if (end > 0) return rest.slice(0, end);
  }
  return null;
}

/** RFC3339 → epoch ms. 못 읽으면 0 — "아주 옛날" 취급. */
function ts(s: string): number {
  const t = Date.parse(s);
  return Number.isFinite(t) ? t : 0;
}

function asCommit(r: LogRecord): TimelineCommit {
  return { sha: r.sha, subject: r.subject, author: r.author, date: r.authorDate };
}

function sortedUnique(files: string[]): string[] {
  return [...new Set(files)].sort();
}

/** 파싱된 레코드에서 타임라인 구성 (Rust `build_timeline` 쌍둥이). */
export function buildTimeline(
  base: string,
  since: Date,
  until: Date,
  tip: string | null,
  history: LogRecord[],
  openRecords: LogRecord[],
): MergeTimelineData {
  const sinceMs = since.getTime();
  const map = new Map(history.map((r) => [r.sha, r]));

  // base 의 first-parent 사슬 — 병합 커밋의 두 번째 부모 쪽만 브랜치 작업.
  const chainSet = new Set<string>();
  const chain: LogRecord[] = [];
  let cur = (tip && map.get(tip)) || history[0];
  while (cur && !chainSet.has(cur.sha)) {
    chainSet.add(cur.sha);
    chain.push(cur);
    cur = cur.parents.length > 0 ? map.get(cur.parents[0])! : undefined!;
  }

  // 오래된 병합부터 귀속 — 나중 브랜치가 이전 병합의 커밋을 가로채지 않게.
  const chainMerges = chain
    .filter((r) => r.parents.length >= 2)
    .sort((a, b) => ts(a.commitDate) - ts(b.commitDate));
  const assigned = new Set<string>();
  const merges: TimelineMerge[] = [];
  for (const m of chainMerges) {
    const commits: TimelineCommit[] = [];
    let files: string[] = [];
    const stack = m.parents.slice(1);
    while (stack.length > 0) {
      const p = stack.pop()!;
      if (chainSet.has(p) || assigned.has(p)) continue;
      const rec = map.get(p);
      if (!rec) continue;
      assigned.add(p);
      commits.push(asCommit(rec));
      files = files.concat(rec.files);
      stack.push(...rec.parents);
    }
    if (ts(m.commitDate) < sinceMs) continue; // 창 밖 — 귀속만.
    commits.sort((a, b) => ts(b.date) - ts(a.date));
    const first = commits.reduce<string | null>(
      (acc, c) => (acc === null || ts(c.date) < ts(acc) ? c.date : acc),
      null,
    );
    merges.push({
      sha: m.sha,
      date: m.commitDate,
      author: m.author,
      subject: m.subject,
      branch: branchFromSubject(m.subject),
      commits,
      files: sortedUnique(files),
      first_commit_date: first,
    });
  }
  merges.sort((a, b) => ts(a.date) - ts(b.date));

  const direct = chain
    .filter((r) => r.parents.length < 2 && ts(r.commitDate) >= sinceMs)
    .map(asCommit);

  // ── 미병합 원격 브랜치 — %S(도달 ref)로 묶는다 ──────────────────────────
  const byName = new Map<string, LogRecord[]>();
  for (const r of openRecords) {
    let name = r.source;
    if (name.startsWith("refs/remotes/")) {
      const rest = name.slice("refs/remotes/".length);
      const slash = rest.indexOf("/");
      name = slash >= 0 ? rest.slice(slash + 1) : rest;
    }
    if (!name || name === "HEAD" || name === base) continue;
    const list = byName.get(name) ?? [];
    list.push(r);
    byName.set(name, list);
  }
  const open: TimelineOpenBranch[] = [];
  for (const [name, recs] of byName) {
    const commits = recs.map(asCommit).sort((a, b) => ts(b.date) - ts(a.date));
    open.push({
      name,
      commits,
      files: sortedUnique(recs.flatMap((r) => r.files)),
      first_date: commits[commits.length - 1]?.date ?? "",
      last_date: commits[0]?.date ?? "",
    });
  }
  open.sort((a, b) => ts(b.last_date) - ts(a.last_date));

  return {
    base,
    since: since.toISOString(),
    until: until.toISOString(),
    merges,
    direct,
    open,
  };
}

/** 브릿지의 `merge_timeline` 케이스 본체 — Rust `merge_timeline` 쌍둥이. */
export function mergeTimeline(
  run: (args: string[]) => RunResult,
  remote: string,
  base: string,
  days: number,
): MergeTimelineData {
  const until = new Date();
  const d = Math.max(1, Math.floor(days) || 1);
  const since = new Date(until.getTime() - d * 86400_000);
  const empty: MergeTimelineData = {
    base,
    since: since.toISOString(),
    until: until.toISOString(),
    merges: [],
    direct: [],
    open: [],
  };

  // 원격 추적 ref 우선 — 팀의 진실은 origin. 없으면 로컬 브랜치.
  const verify = (ref: string) => run(["rev-parse", "--verify", "--quiet", ref]).ok;
  const remoteRef = `refs/remotes/${remote}/${base}`;
  const localRef = `refs/heads/${base}`;
  const baseRef = verify(remoteRef) ? remoteRef : verify(localRef) ? localRef : null;
  if (!baseRef) return empty;
  const tipOut = run(["rev-parse", baseRef]);
  const tip = tipOut.ok ? tipOut.stdout.trim() : null;

  // 창보다 14일 더 읽는다 — 창 안의 병합에 담긴 옛 브랜치 커밋도 귀속되게.
  const hist = run([
    "log",
    baseRef,
    `--since=${d + 14}.days`,
    "--date=iso-strict",
    "--name-only",
    `--format=%x1e%H%x1f%P%x1f%an%x1f%aI%x1f%cI%x1f%s`,
  ]);
  const history = hist.ok ? parseBlocks(hist.stdout, false) : [];

  const openOut = run([
    "log",
    `--glob=refs/remotes/${remote}/*`,
    "--not",
    baseRef,
    `--since=${d}.days`,
    "--source",
    "--date=iso-strict",
    "--name-only",
    `--format=%x1e%H%x1f%P%x1f%an%x1f%aI%x1f%cI%x1f%s%x1f%S`,
  ]);
  const openRecords = openOut.ok ? parseBlocks(openOut.stdout, true) : [];

  return buildTimeline(base, since, until, tip, history, openRecords);
}
