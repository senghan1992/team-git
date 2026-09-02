// 알림 → 저장소 매칭 검증. `pnpm test:ui`로 실행.
import { normalizeRemoteUrl, repoForEvent } from "./repoMatch";
import type { Repo } from "./ipc";

function assert(cond: boolean, msg: string) {
  if (!cond) throw new Error(`ASSERTION FAILED: ${msg}`);
  console.log(`PASS: ${msg}`);
}

function repo(id: string, name: string, remote: string): Repo {
  return {
    id,
    path: `/home/dev/${name}`,
    display_name: name,
    default_branch: "main",
    working_branch: "main",
    ssh_host: "",
    ssh_user: "",
    ssh_key_path: "",
    ssh_password: "",
    ed25519_fingerprint: "",
    remote_url: remote,
    created_at: "2026-01-01T00:00:00Z",
  };
}

function ev(repoName: string, url?: string): { repo_name: string; payload: string } {
  return {
    repo_name: repoName,
    payload: JSON.stringify({ kind: "main_push", data: url ? { url, branch: "main" } : { branch: "main" } }),
  };
}

// ── 정규화: 표기가 달라도 같은 저장소는 같은 열쇠 ──────────────────────────
{
  const expect = "github.com/team/app";
  for (const u of [
    "https://github.com/team/app.git",
    "git@github.com:team/app.git",
    "ssh://git@github.com/team/app.git/",
    "GITHUB.com/team/app",
  ]) {
    assert(normalizeRemoteUrl(u) === expect, `정규화 일치: ${u}`);
  }
}

{
  // 자격증명은 반드시 벗겨진다 — payload에 토큰이 남으면 안 된다.
  const n = normalizeRemoteUrl("http://oauth2:glpat-secret@git.corp.com/hub/app.git");
  assert(n === "git.corp.com/hub/app", `자격증명 제거 (got ${n})`);
  assert(!n.includes("secret"), "토큰이 열쇠에 남지 않는다");
}

{
  // 포트는 유지 — 다른 포트는 다른 서버일 수 있다.
  assert(
    normalizeRemoteUrl("https://host.com:8443/team/app.git") === "host.com:8443/team/app",
    "포트 유지",
  );
}

// ── 매칭: URL 우선, 이름 폴백 ───────────────────────────────────────────────
{
  // 폴더 이름이 달라도 origin이 같으면 찾는다 — 팀원마다 clone 이름이 다른 현실.
  const repos = [repo("r1", "my-app-clone", "git@github.com:team/app.git")];
  const m = repoForEvent(repos, ev("app", "https://github.com/team/app.git"));
  assert(m?.id === "r1", `이름이 달라도 URL로 매칭 (got ${m?.id})`);
}

{
  // 같은 이름의 저장소가 둘이어도 URL이 가른다 — 예전의 막다른 길.
  const repos = [
    repo("r1", "app", "https://github.com/team-a/app.git"),
    repo("r2", "app", "https://github.com/team-b/app.git"),
  ];
  const m = repoForEvent(repos, ev("app", "git@github.com:team-b/app.git"));
  assert(m?.id === "r2", `동명 저장소는 URL이 가른다 (got ${m?.id})`);
}

{
  // URL 없는 구버전 이벤트 — 이름이 유일하면 그걸로.
  const repos = [repo("r1", "app", "https://github.com/team/app.git")];
  assert(repoForEvent(repos, ev("app"))?.id === "r1", "URL 없으면 유일한 이름으로 폴백");
}

{
  // URL도 없고 이름도 겹치면 못 찾는 게 맞다 — 엉뚱한 저장소에 동기화하면 안 된다.
  const repos = [
    repo("r1", "app", "https://github.com/team-a/app.git"),
    repo("r2", "app", "https://github.com/team-b/app.git"),
  ];
  assert(repoForEvent(repos, ev("app")) === null, "모호하면 null");
}

{
  // 이벤트 URL이 어느 등록 저장소와도 다르면 이름 폴백으로 넘어간다.
  const repos = [repo("r1", "app", "https://github.com/team/app.git")];
  const m = repoForEvent(repos, ev("app", "https://github.com/other/thing.git"));
  assert(m?.id === "r1", `URL 불일치 시 이름 폴백 (got ${m?.id})`);
}

console.log("\n✓ repoMatch 전체 통과");
