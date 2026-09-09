// 가상 팀원 "박준호"가 지금 자기 브랜치를 push 하는 상황을 만든다.
//
//   pnpm demo:push                  # feature/<자동 이름> 브랜치에 커밋 1개 → push → 알림
//   pnpm demo:push -- --branch fix/typo --message "fix: 오타"
//
// 하는 일 (모두 실제 git · 실제 팀 서버):
//   1. ~/gc-demo/teammate-clone 이 없으면 origin 을 clone 한다.
//   2. origin/main 에서 새 브랜치를 만들어 파일 하나를 고치고 커밋·push 한다.
//   3. 이어서 병합 요청까지 보낸다 — refs/gc-mr/main/<브랜치> ref 를 origin 에
//      올린다 (앱의 "병합 요청 보내기"와 같은 상태). push만으로는 관리자의
//      승인 대기열에 오르지 않는다.
//   4. 팀원 기기(~/gc-demo/teammate_token)로 팀 서버에 branch_push + merge_request
//      이벤트를 보낸다 — 데스크톱 앱에서는 pre-push hook 과 병합 요청 커맨드가 하는 일이다.
// 브라우저(또는 앱)를 보고 있으면 5초 안에 우측 하단 알림과 사이드바 배지가 뜬다.
// 먼저 `pnpm seed:demo` 로 팀·기기가 준비돼 있어야 한다.
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const DEMO_ROOT = join(homedir(), "gc-demo");
const ORIGIN = join(DEMO_ROOT, "origin.git");
const CLONE = join(DEMO_ROOT, "teammate-clone");
const TOKEN_FILE = join(DEMO_ROOT, "teammate_token");
const CONFIG = join(homedir(), ".config", "com.gitcompanion.app", "config.json");
const REPO_PROJECTS = join(homedir(), ".config", "com.gitcompanion.app", "repo_projects.json");
const DEMO_PROJECT_NAME = "demo-app 팀";

const argv = process.argv.slice(2);
function opt(name, fallback) {
  const i = argv.indexOf(`--${name}`);
  return i >= 0 && argv[i + 1] ? argv[i + 1] : fallback;
}

function git(cwd, ...a) {
  return execFileSync("git", a, {
    cwd,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
    env: { ...process.env, GIT_TERMINAL_PROMPT: "0" },
  }).trim();
}

if (!existsSync(ORIGIN) || !existsSync(TOKEN_FILE)) {
  console.error("데모가 준비되지 않았습니다. 먼저 실행하세요: pnpm seed:demo");
  process.exit(1);
}

let cfg;
try {
  cfg = JSON.parse(readFileSync(CONFIG, "utf8"));
} catch {
  cfg = {};
}
const backend = (cfg.peer?.backend_url || process.env.GC_BACKEND_URL || "http://127.0.0.1:8000").replace(/\/+$/, "");

// 1. clone
if (!existsSync(CLONE)) {
  mkdirSync(DEMO_ROOT, { recursive: true });
  git(DEMO_ROOT, "clone", "-q", ORIGIN, CLONE);
}
git(CLONE, "config", "user.name", "박준호");
git(CLONE, "config", "user.email", "junho@example.com");
git(CLONE, "config", "commit.gpgsign", "false");
git(CLONE, "fetch", "-q", "--prune", "origin");

// 2. 브랜치 + 커밋 + push
const stamp = new Date();
const auto = `feature/junho-${stamp.getMonth() + 1}${String(stamp.getDate()).padStart(2, "0")}-${String(stamp.getHours()).padStart(2, "0")}${String(stamp.getMinutes()).padStart(2, "0")}${String(stamp.getSeconds()).padStart(2, "0")}`;
const branch = opt("branch", auto);
const message = opt("message", `feat: ${branch.split("/").pop()} 작업`);
git(CLONE, "checkout", "-q", "-B", branch, "origin/main");
const file = join(CLONE, "src", "work", `${branch.replace(/[^A-Za-z0-9._-]+/g, "-")}.ts`);
mkdirSync(join(file, ".."), { recursive: true });
writeFileSync(file, `// ${message}\nexport const createdAt = "${stamp.toISOString()}";\n`);
git(CLONE, "add", "-A");
git(CLONE, "commit", "-qm", message);
git(CLONE, "push", "-q", "-u", "origin", branch);
const sha = git(CLONE, "rev-parse", "HEAD");
console.log(`push 완료: ${branch} (${sha.slice(0, 7)}) — ${message}`);

// 4. 병합 요청 — 작업 탭의 "병합 요청 보내기"와 같은 상태를 실제 git 으로 만든다.
//      요청 메타데이터를 태그 객체 message 의 JSON 으로 넣고 refs/gc-mr/<base>/<branch>
//      ref 를 origin 에 올린다 (Rust git::mr · dev/git-bridge.ts 와 같은 규칙).
const BASE = "main";
const refPath = `refs/gc-mr/${BASE}/${branch}`;
const mrPayload = JSON.stringify({
  base: BASE,
  branch,
  ref_path: refPath,
  sha,
  title: message,
  author: "박준호",
  email: "junho@example.com",
  created_at: Math.floor(Date.now() / 1000),
  open: true,
});
const tmpTag = `gc-mr-tmp-${Date.now()}`;
git(CLONE, "-c", "user.name=박준호", "-c", "user.email=junho@example.com",
  "tag", "-a", "-f", "-m", mrPayload, tmpTag, sha);
const tagSha = git(CLONE, "rev-parse", `refs/tags/${tmpTag}`);
git(CLONE, "tag", "-d", tmpTag);
git(CLONE, "update-ref", refPath, tagSha);
git(CLONE, "push", "-q", "origin", `+${refPath}:${refPath}`);
console.log(`병합 요청 완료: ${branch} → ${BASE} 승인 대기열 (${refPath})`);

// 5. 알림 (pre-push hook 의 `git-companion hook emit` + 병합 요청 커맨드에 해당)
const token = readFileSync(TOKEN_FILE, "utf8").trim();
let projectIds = [];
try {
  const links = JSON.parse(readFileSync(REPO_PROJECTS, "utf8"));
  projectIds = links[join(DEMO_ROOT, "demo-app")] ?? [];
} catch {
  projectIds = [];
}
if (projectIds.length === 0) {
  // 연결 파일이 없으면 서버에서 팀을 찾는다.
  const r = await fetch(`${backend}/projects`, { headers: { authorization: `Bearer ${token}` } });
  const j = r.ok ? await r.json() : { projects: [] };
  projectIds = (j.projects ?? []).filter((p) => p.display_name === DEMO_PROJECT_NAME).map((p) => p.id);
}
if (projectIds.length === 0) {
  console.error("demo-app 팀 프로젝트를 찾지 못했습니다. 다시 실행하세요: pnpm seed:demo");
  process.exit(1);
}
const url = ORIGIN.endsWith(".git") ? ORIGIN.slice(0, -4) : ORIGIN;
const baseData = { repo_name: "demo-app", url };
const events = [
  // push 알림 (pre-push hook 이 보내는 것과 같은 이벤트) — 관리자에게는 참고용.
  {
    event_kind: "branch_push",
    payload: JSON.stringify({
      kind: "branch_push",
      data: { author: "박준호", message, sha, ...baseData, branch },
    }),
  },
  // 병합 요청 알림 (앱의 "병합 요청 보내기"가 보내는 것과 같은 이벤트) —
  // 관리자에게만 "검토하기" 할 일 알림이 간다.
  {
    event_kind: "merge_request",
    payload: JSON.stringify({
      kind: "merge_request",
      data: { author: "박준호", message, sha, ...baseData, branch, base: BASE },
    }),
  },
];
let sent = 0;
for (const projectId of projectIds) {
  for (const ev of events) {
    const r = await fetch(`${backend}/events`, {
      method: "POST",
      headers: { authorization: `Bearer ${token}`, "content-type": "application/json" },
      body: JSON.stringify({ project_id: projectId, event_kind: ev.event_kind, repo_name: "demo-app", payload: ev.payload }),
    });
    if (r.ok) sent += 1;
    else console.error(`알림 전송 실패 (${r.status}) — 팀 서버(${backend})가 떠 있는지 확인하세요.`);
  }
}
if (sent > 0) console.log(`알림 전송 완료 (${sent}건) — 관리자 화면에 "새 push"·"병합 요청 도착" 알림이 뜨고, ${branch} 가 승인 대기열에 올라갑니다.`);
