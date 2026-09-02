// 변경 지도 뒤집기 검증 — 파일 → 브랜치. `pnpm test:ui`로 실행.
import { buildChangeMap } from "./ChangeMap";
import type { PendingBranch } from "../lib/ipc";

function assert(cond: boolean, msg: string) {
  if (!cond) throw new Error(`ASSERTION FAILED: ${msg}`);
  console.log(`PASS: ${msg}`);
}

function branch(
  short: string,
  author: string,
  files: [string, string][],
): PendingBranch {
  return {
    name: `origin/${short}`,
    short_name: short,
    sha: "abc1234",
    author,
    unix_time: 1_700_000_000,
    subject: "wip",
    ahead: 1,
    behind: 0,
    changed_files: files.map(([path, kind]) => ({ path, kind })),
  };
}

{
  assert(buildChangeMap([]).length === 0, "브랜치가 없으면 빈 지도");
}

{
  const rows = buildChangeMap([
    branch("feature/login", "김민지", [
      ["src/api/user.ts", "M"],
      ["src/auth/token.ts", "A"],
    ]),
    branch("feature/pay", "박준호", [
      ["src/api/user.ts", "M"],
      ["src/pay/index.ts", "A"],
    ]),
    branch("fix/nav", "이도윤", [["src/api/user.ts", "D"]]),
  ]);

  assert(rows.length === 3, `파일 3개로 합쳐진다 (got ${rows.length})`);
  // 가장 많이 겹치는 파일이 맨 위 — 병합 관리자가 먼저 봐야 하는 것.
  assert(
    rows[0]!.path === "src/api/user.ts",
    `겹치는 파일이 최상단 (got ${rows[0]!.path})`,
  );
  assert(rows[0]!.touches.length === 3, "3개 브랜치가 같은 파일을 건드린다");
  assert(
    rows[0]!.touches.map((t) => t.branch).join(",") ===
      "feature/login,feature/pay,fix/nav",
    "브랜치 순서는 입력 순서를 유지한다",
  );
  assert(
    rows[0]!.touches.map((t) => t.kind).join(",") === "M,M,D",
    "브랜치별 변경 종류를 따로 유지한다 (한쪽이 삭제해도 보인다)",
  );
  assert(
    rows[0]!.touches.map((t) => t.author).join(",") === "김민지,박준호,이도윤",
    "누가 고치는지 함께 담긴다",
  );

  // 겹치지 않는 나머지는 경로 순으로 안정 정렬 — 새로고침마다 순서가 흔들리면
  // 목록을 읽을 수 없다.
  const rest = rows.slice(1).map((r) => r.path);
  assert(
    rest.join(",") === "src/auth/token.ts,src/pay/index.ts",
    `단독 수정 파일은 경로 순 (got ${rest.join(",")})`,
  );
  assert(
    rows.slice(1).every((r) => r.touches.length === 1),
    "단독 수정 파일은 브랜치 1개",
  );
}

{
  // 한 브랜치만 있으면 겹침이 없어야 한다 — 잘못된 경고를 띄우면 안 된다.
  const rows = buildChangeMap([
    branch("solo", "혼자", [["a.ts", "M"], ["b.ts", "M"]]),
  ]);
  assert(rows.length === 2, "파일 2개");
  assert(rows.every((r) => r.touches.length === 1), "겹침 없음");
}

console.log("\n✓ ChangeMap 전체 통과");
