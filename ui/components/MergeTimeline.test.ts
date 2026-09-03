/**
 * MergeTimeline 순수 로직 테스트 — 레인 배정/클램프(layoutTimeline)와
 * 브릿지 쌍둥이 구현(branchFromSubject, parseBlocks, buildTimeline).
 * GitGraph.test.ts 와 같은 무프레임워크 assert 스타일, `pnpm test:ui` 로 실행.
 */
import { layoutTimeline } from "./MergeTimeline";
import type { MergeTimeline, TimelineMerge, TimelineOpenBranch } from "../lib/ipc";
import {
  branchFromSubject,
  buildTimeline,
  parseBlocks,
} from "../../dev/bridge-timeline";

function assert(cond: boolean, msg: string) {
  if (!cond) throw new Error(`ASSERTION FAILED: ${msg}`);
}

const SINCE = "2026-09-01T00:00:00+09:00";
const UNTIL = "2026-09-08T00:00:00+09:00";
const WIDTH = 700; // 7일 창 → 하루 100px

function merge(sha: string, date: string, first: string | null, branch = "b"): TimelineMerge {
  return {
    sha,
    date,
    author: "tester",
    subject: `${branch} 브렌치 병합`,
    branch,
    commits: [],
    files: [],
    first_commit_date: first,
  };
}

function open(name: string, first: string, last: string): TimelineOpenBranch {
  return { name, commits: [], files: [], first_date: first, last_date: last };
}

function data(merges: TimelineMerge[], openBranches: TimelineOpenBranch[] = []): MergeTimeline {
  return { base: "main", since: SINCE, until: UNTIL, merges, direct: [], open: openBranches };
}

// ── layoutTimeline: 좌표와 클램프 ────────────────────────────────────────────
{
  const d = data([
    merge("m1", "2026-09-03T00:00:00+09:00", "2026-09-02T00:00:00+09:00"),
  ]);
  const { items } = layoutTimeline(d, { since: SINCE, until: UNTIL, width: WIDTH });
  assert(items.length === 1, `items=${items.length}, expected 1`);
  assert(Math.abs(items[0].x0 - 100) < 1e-6, `x0=${items[0].x0}, expected 100 (1일째)`);
  assert(Math.abs(items[0].x1 - 200) < 1e-6, `x1=${items[0].x1}, expected 200 (2일째)`);
  console.log("PASS: layout 좌표 (일 단위 비례)");
}

{
  // 창보다 오래된 첫 커밋은 0 으로 잘린다.
  const d = data([merge("m1", "2026-09-02T00:00:00+09:00", "2026-08-20T00:00:00+09:00")]);
  const { items } = layoutTimeline(d, { since: SINCE, until: UNTIL, width: WIDTH });
  assert(items[0].x0 === 0, `x0=${items[0].x0}, expected 0 (창 시작으로 클램프)`);
  console.log("PASS: layout 창 밖 시작 클램프");
}

{
  // 길이 0 구간(first == date, 또는 first 가 null)은 최소 6px.
  const d = data([merge("m1", "2026-09-02T00:00:00+09:00", null)]);
  const { items } = layoutTimeline(d, { since: SINCE, until: UNTIL, width: WIDTH });
  assert(items[0].x1 - items[0].x0 >= 6, `길이=${items[0].x1 - items[0].x0}, expected >= 6`);
  assert(items[0].x0 >= 0, `x0=${items[0].x0}, 음수 금지`);
  console.log("PASS: layout 길이 0 구간 최소 폭");
}

{
  // 겹치는 두 병합은 다른 레인, 떨어져 있으면 레인 재사용.
  const d = data([
    merge("m1", "2026-09-02T12:00:00+09:00", "2026-09-01T12:00:00+09:00", "a"),
    merge("m2", "2026-09-03T00:00:00+09:00", "2026-09-02T00:00:00+09:00", "b"), // m1 과 겹침
    merge("m3", "2026-09-06T00:00:00+09:00", "2026-09-05T00:00:00+09:00", "c"), // 멀리 떨어짐
  ]);
  const { items, laneCount } = layoutTimeline(d, { since: SINCE, until: UNTIL, width: WIDTH });
  const byKey = new Map(items.map((i) => [i.key, i]));
  assert(byKey.get("m1")!.lane !== byKey.get("m2")!.lane, "겹치는 병합은 레인이 달라야 한다");
  assert(byKey.get("m3")!.lane === byKey.get("m1")!.lane, "떨어진 병합은 빈 레인을 재사용");
  assert(laneCount === 2, `laneCount=${laneCount}, expected 2`);
  console.log("PASS: layout 레인 탐욕 배정");
}

{
  // 열린 브랜치는 오른쪽 끝까지 이어진다 → 이후 항목과 레인을 공유하지 못한다.
  const d = data(
    [merge("m1", "2026-09-06T00:00:00+09:00", "2026-09-05T12:00:00+09:00", "late")],
    [open("feature/wip", "2026-09-02T00:00:00+09:00", "2026-09-05T00:00:00+09:00")],
  );
  const { items } = layoutTimeline(d, { since: SINCE, until: UNTIL, width: WIDTH });
  const wip = items.find((i) => i.kind === "open")!;
  assert(wip.x1 === WIDTH, `open x1=${wip.x1}, expected ${WIDTH} (지금까지)`);
  const late = items.find((i) => i.key === "m1")!;
  assert(late.lane !== wip.lane, "열린 브랜치 위로 겹치는 병합은 다른 레인");
  console.log("PASS: layout 열린 브랜치는 끝까지");
}

{
  // 오늘 시작한 짧은 브랜치 셋이 오른쪽 끝에 몰리면 — 선분은 거의 겹치지 않아도
  // 레이블 글자가 겹치므로 서로 다른 레인을 받고, 레이블은 끝 기준 정렬이어야 한다.
  const d = data(
    [],
    [
      open("feature/login", "2026-09-07T22:00:00+09:00", "2026-09-07T23:00:00+09:00"),
      open("feature/payment", "2026-09-07T22:30:00+09:00", "2026-09-07T23:00:00+09:00"),
      open("fix/nav", "2026-09-07T22:40:00+09:00", "2026-09-07T23:00:00+09:00"),
    ],
  );
  const { items, laneCount } = layoutTimeline(d, { since: SINCE, until: UNTIL, width: WIDTH });
  assert(laneCount === 3, `laneCount=${laneCount}, expected 3 (레이블이 겹치므로 각자 레인)`);
  assert(items.every((i) => i.anchorEnd), "가장자리 항목은 끝 기준 정렬");
  assert(items.every((i) => i.label.endsWith("병합 대기")), "열린 브랜치 레이블에 '병합 대기'");
  assert(items.every((i) => i.occ0 < i.x0 && i.occ1 === WIDTH), "점유 구간이 레이블만큼 왼쪽으로 넓다");
  console.log("PASS: layout 오른쪽 가장자리 레이블 겹침 방지");
}

{
  // 멀리 떨어진 두 항목은 레이블 폭을 고려해도 한 레인을 공유한다.
  const d = data([
    merge("m1", "2026-09-02T00:00:00+09:00", "2026-09-01T12:00:00+09:00", "a"),
    merge("m2", "2026-09-07T00:00:00+09:00", "2026-09-06T00:00:00+09:00", "b"),
  ]);
  const { laneCount } = layoutTimeline(d, { since: SINCE, until: UNTIL, width: WIDTH });
  assert(laneCount === 1, `laneCount=${laneCount}, expected 1`);
  console.log("PASS: layout 떨어진 항목은 레이블 포함해도 레인 공유");
}

// ── branchFromSubject (브릿지 쌍둥이) ────────────────────────────────────────
{
  assert(branchFromSubject("feature/login 브렌치 병합") === "feature/login", "팀 컨벤션");
  assert(branchFromSubject("fix/nav 브랜치 병합") === "fix/nav", "브랜치 표기");
  assert(branchFromSubject("Merge branch 'feature/pay'") === "feature/pay", "git 기본");
  assert(branchFromSubject("Merge branch 'hotfix' of https://x/y") === "hotfix", "of URL");
  assert(
    branchFromSubject("Merge remote-tracking branch 'origin/dev-a'") === "dev-a",
    "remote-tracking",
  );
  assert(branchFromSubject("feat: 일반 커밋") === null, "모르는 문구는 null");
  console.log("PASS: branchFromSubject");
}

// ── parseBlocks + buildTimeline (브릿지 쌍둥이) ──────────────────────────────
{
  const US = "\u001f";
  const RS = "\u001e";
  const d = (day: number) => `2026-09-0${day}T12:00:00+09:00`;
  const hist =
    `${RS}m3${US}d2 f2${US}민지${US}${d(3)}${US}${d(3)}${US}feature/login 브렌치 병합\n` +
    `${RS}f2${US}f1${US}준호${US}${d(2)}${US}${d(2)}${US}feat: 2\n\nsrc/a.ts\nsrc/b.ts\n` +
    `${RS}d2${US}i1${US}민지${US}${d(2)}${US}${d(2)}${US}chore: direct\n\nREADME.md\n` +
    `${RS}f1${US}i1${US}준호${US}2026-08-30T12:00:00+09:00${US}${d(1)}${US}feat: 1\n\nsrc/a.ts\n` +
    `${RS}i1${US}${US}민지${US}2026-08-20T12:00:00+09:00${US}2026-08-20T12:00:00+09:00${US}init\n\ninit.txt\n`;
  const history = parseBlocks(hist, false);
  assert(history.length === 5, `history=${history.length}, expected 5`);

  const openOut = `${RS}w1${US}${US}도윤${US}${d(5)}${US}${d(5)}${US}wip${US}refs/remotes/origin/feature/wip\n\nw.txt\n`;
  const openRecords = parseBlocks(openOut, true);

  const tl = buildTimeline(
    "main",
    new Date("2026-09-01T00:00:00+09:00"),
    new Date("2026-09-08T00:00:00+09:00"),
    "m3",
    history,
    openRecords,
  );
  assert(tl.merges.length === 1, `merges=${tl.merges.length}, expected 1`);
  assert(tl.merges[0].branch === "feature/login", `branch=${tl.merges[0].branch}`);
  assert(tl.merges[0].commits.length === 2, `commits=${tl.merges[0].commits.length}, expected 2`);
  assert(
    JSON.stringify(tl.merges[0].files) === JSON.stringify(["src/a.ts", "src/b.ts"]),
    `files=${tl.merges[0].files}`,
  );
  assert(
    tl.merges[0].first_commit_date === "2026-08-30T12:00:00+09:00",
    `first=${tl.merges[0].first_commit_date}`,
  );
  assert(tl.direct.length === 1 && tl.direct[0].sha === "d2", `direct=${JSON.stringify(tl.direct)}`);
  assert(tl.open.length === 1 && tl.open[0].name === "feature/wip", `open=${JSON.stringify(tl.open)}`);
  console.log("PASS: buildTimeline 병합 귀속/직접 커밋/열린 브랜치");
}

{
  // 제목에 0x1f 가 들어 있어도 블록이 죽지 않는다.
  const US = "\u001f";
  const RS = "\u001e";
  const out = `${RS}aa${US}${US}A${US}2026-09-02T10:00:00+09:00${US}2026-09-02T10:00:00+09:00${US}weird${US}subject\n`;
  const recs = parseBlocks(out, false);
  assert(recs.length === 1 && recs[0].subject === `weird${US}subject`, "0x1f in subject");
  console.log("PASS: parseBlocks 제목의 0x1f");
}

console.log("MergeTimeline tests: all passed");
