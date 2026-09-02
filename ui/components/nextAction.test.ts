// 다음 할 일 우선순위 검증. `pnpm test:ui`로 실행.
import { computeNextAction, isMergeManagerFor, type NextActionInput } from "./nextAction";
import type { FileChange, ProjectConfigResult, WorkingTreeStatus } from "../lib/ipc";

function assert(cond: boolean, msg: string) {
  if (!cond) throw new Error(`ASSERTION FAILED: ${msg}`);
  console.log(`PASS: ${msg}`);
}

function status(
  over: Partial<WorkingTreeStatus> & { kinds?: FileChange["kind"][] } = {},
): WorkingTreeStatus {
  const kinds = over.kinds ?? [];
  return {
    branch: "feature/login",
    upstream: "origin/feature/login",
    ahead: over.ahead ?? 0,
    behind: over.behind ?? 0,
    behind_base: over.behind_base ?? 0,
    files: kinds.map((kind, i) => ({ kind, path: `f${i}.ts` }) as FileChange),
  };
}

function input(over: Partial<NextActionInput> = {}): NextActionInput {
  return {
    status: status(),
    pendingCount: null,
    isMergeManager: false,
    baseBranch: "main",
    ...over,
  };
}

// ── 우선순위: 충돌 > 병합 대기 > 커밋 > 푸시 > 동기화 > 없음 ──────────────
{
  // 충돌은 다른 모든 신호를 이긴다 — 풀기 전엔 아무것도 진행되지 않는다.
  const a = computeNextAction(
    input({
      status: status({ kinds: ["conflicted", "modified"], ahead: 3, behind: 2 }),
      isMergeManager: true,
      pendingCount: 5,
    }),
  );
  assert(a.kind === "resolve", `충돌이 최우선 (got ${a.kind})`);
  assert(a.tab === "merge", "충돌은 병합 탭으로 보낸다");
  assert(a.urgent, "충돌은 긴급");
}

{
  // 관리자의 병합 대기는 내 커밋보다 앞선다 — 팀 전체가 막혀 있기 때문.
  const a = computeNextAction(
    input({
      status: status({ kinds: ["modified"], ahead: 1 }),
      isMergeManager: true,
      pendingCount: 2,
    }),
  );
  assert(a.kind === "merge", `관리자 병합 대기가 커밋보다 우선 (got ${a.kind})`);
  assert(a.label.includes("2건"), `대기 건수가 문구에 들어간다 (got ${a.label})`);
}

{
  // 관리자가 아니면 병합 대기는 무시하고 내 일을 시킨다.
  const a = computeNextAction(
    input({
      status: status({ kinds: ["modified"] }),
      isMergeManager: false,
      pendingCount: 9,
    }),
  );
  assert(a.kind === "commit", `관리자가 아니면 커밋이 다음 할 일 (got ${a.kind})`);
}

{
  // pendingCount 를 모를 때(조회 실패)는 그 규칙을 건너뛴다.
  const a = computeNextAction(
    input({ status: status({ ahead: 2 }), isMergeManager: true, pendingCount: null }),
  );
  assert(a.kind === "push", `대기 수를 모르면 push 로 넘어간다 (got ${a.kind})`);
  assert(a.urgent, "미푸시는 관리자를 기다리게 하므로 긴급");
}

{
  const a = computeNextAction(input({ status: status({ behind: 4 }) }));
  assert(a.kind === "sync", `뒤처지면 동기화 (got ${a.kind})`);
  assert(!a.urgent, "동기화는 급하지 않다");
}

{
  // ahead + behind 가 동시에 있으면 푸시가 아니라 동기화 먼저 —
  // 지금 푸시하면 non-fast-forward 로 거절당한다.
  const a = computeNextAction(input({ status: status({ ahead: 2, behind: 3 }) }));
  assert(a.kind === "sync", `갈라진 상태는 받은 뒤 푸시 (got ${a.kind})`);
  assert(a.urgent, "갈라진 상태는 긴급 — 푸시가 막혀 있다");
}

{
  // 병합 브랜치(origin/main)가 앞서 있으면 upstream behind 가 0 이어도
  // 동기화를 제안한다 — 관리자의 병합이 push 된 직후의 상태다.
  const a = computeNextAction(input({ status: status({ behind_base: 5 }) }));
  assert(a.kind === "sync", `병합 브랜치가 앞서면 동기화 (got ${a.kind})`);
  assert(a.label.includes("5개"), `가져올 커밋 수가 문구에 들어간다 (got ${a.label})`);
}

{
  // upstream 이 없어도 (한 번도 push 안 한 브랜치) behind_base 는 동작한다.
  const a = computeNextAction(
    input({ status: { ...status({ behind_base: 2 }), upstream: null } }),
  );
  assert(a.kind === "sync", `upstream 없이도 병합 브랜치 동기화 제안 (got ${a.kind})`);
}

{
  const a = computeNextAction(input({ status: status() }));
  assert(a.kind === "clean", `깨끗하면 할 일 없음 (got ${a.kind})`);
}

{
  // 상태를 못 읽어도 죽지 않고 clean 으로 떨어진다.
  const a = computeNextAction(input({ status: null }));
  assert(a.kind === "clean", `상태 없음이면 clean (got ${a.kind})`);
}

{
  // untracked 도 커밋해야 하는 변경으로 센다.
  const a = computeNextAction(input({ status: status({ kinds: ["untracked"] }) }));
  assert(a.kind === "commit", `untracked 도 커밋 대상 (got ${a.kind})`);
  assert(a.label.includes("1개"), `개수가 문구에 들어간다 (got ${a.label})`);
}

// ── 병합 관리자 판정 ──────────────────────────────────────────────────────
function cfg(
  managers: Record<string, string>,
  members: { email: string; role: string }[] = [],
): ProjectConfigResult {
  return {
    exists: true,
    config: {
      gpconfig_version: 2,
      default_base_branch: "main",
      members: members.map((m, i) => ({ id: `u${i}`, name: `u${i}`, ...m })),
      merge_managers: managers,
      merge_targets: ["main"],
      notify_recipients: [],
      notify: { on_branch_ready: false, on_merge_complete: false },
    },
  };
}

assert(
  isMergeManagerFor(null, "a@x.com", "main"),
  "설정이 아직 없으면 누구나 병합할 수 있다",
);
assert(
  isMergeManagerFor(cfg({}), "a@x.com", "main"),
  "관리자 미지정이면 누구나 병합할 수 있다",
);
assert(
  isMergeManagerFor(cfg({ main: "LEAD@x.com" }), "lead@x.com", "main"),
  "지정된 관리자는 대소문자 무시하고 인정된다",
);
assert(
  !isMergeManagerFor(cfg({ main: "lead@x.com" }), "other@x.com", "main"),
  "지정된 관리자가 아니면 병합 대기를 재촉하지 않는다",
);
assert(
  isMergeManagerFor(
    cfg({ main: "lead@x.com" }, [{ email: "boss@x.com", role: "admin" }]),
    "boss@x.com",
    "main",
  ),
  "admin 은 모든 브랜치를 병합할 수 있다",
);
assert(
  !isMergeManagerFor(cfg({ main: "lead@x.com" }), null, "main"),
  "미로그인이면 관리자로 보지 않는다",
);
assert(
  isMergeManagerFor(cfg({ "release/1.0": "lead@x.com" }), "other@x.com", "main"),
  "다른 브랜치에만 관리자가 있으면 main 은 자유롭다",
);

console.log("\n✓ nextAction 전체 통과");
