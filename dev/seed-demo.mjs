// 브라우저 미리보기용 데모 환경을 만든다.
//
//   node dev/seed-demo.mjs            # 데모 저장소 + 앱 설정 생성
//   node dev/seed-demo.mjs --reset    # 지우고 처음부터 다시
//   node dev/seed-demo.mjs --clean    # 데모만 제거 (설정에서 등록 해제)
//
// 만드는 것:
//   ~/gc-demo/origin.git   원격 역할을 하는 bare 저장소
//   ~/gc-demo/demo-app     작업 클론 — main + 팀원 3명의 브랜치가 push된 상태
//                          (그중 2개는 같은 파일을 고쳐서 병합 시 충돌한다)
//   ~/.config/com.gitcompanion.app/config.json 에 저장소 등록 + AI 자동 병합 켜기
//   + 팀 서버 주소 설정, 서버가 떠 있으면 데모 계정까지 가입
//
// 이렇게 해 두면 앱을 열자마자 "다음 할 일: 3건 병합하기" → 병합 탭의 변경
// 지도 → 충돌 → AI 자동 해결까지 전부 실제 git 위에서 눌러 볼 수 있다.
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { randomUUID } from "node:crypto";

const DEMO_ROOT = join(homedir(), "gc-demo");
const REPO = join(DEMO_ROOT, "demo-app");
const ORIGIN = join(DEMO_ROOT, "origin.git");
const CONFIG_DIR = join(homedir(), ".config", "com.gitcompanion.app");
const CONFIG = join(CONFIG_DIR, "config.json");
/** 팀 서버 주소. 계정·알림이 여기 저장된다. */
const BACKEND_URL = process.env.GC_BACKEND_URL || "http://127.0.0.1:8000";

const args = new Set(process.argv.slice(2));
const reset = args.has("--reset");
const clean = args.has("--clean");

function git(cwd, ...a) {
  return execFileSync("git", a, {
    cwd,
    encoding: "utf8",
    // 빈 bare 저장소를 clone 할 때 git 이 내는 경고를 삼킨다 — 정상 동작인데
    // 출력에 섞이면 오류처럼 보인다.
    stdio: ["ignore", "pipe", "ignore"],
    env: { ...process.env, GIT_TERMINAL_PROMPT: "0" },
  });
}

function write(rel, body) {
  const full = join(REPO, rel);
  mkdirSync(join(full, ".."), { recursive: true });
  writeFileSync(full, body);
}

function loadConfig() {
  if (!existsSync(CONFIG)) return { schema_version: 8, repositories: [] };
  try {
    return JSON.parse(readFileSync(CONFIG, "utf8"));
  } catch {
    return { schema_version: 8, repositories: [] };
  }
}

function saveConfig(cfg) {
  mkdirSync(CONFIG_DIR, { recursive: true });
  writeFileSync(CONFIG, JSON.stringify(cfg, null, 2));
}

// ── --clean: 데모 제거 ───────────────────────────────────────────────────────
if (clean) {
  const cfg = loadConfig();
  const before = cfg.repositories.length;
  cfg.repositories = cfg.repositories.filter((r) => r.path !== REPO);
  saveConfig(cfg);
  rmSync(DEMO_ROOT, { recursive: true, force: true });
  console.log(`데모 제거 완료 (저장소 등록 ${before - cfg.repositories.length}건 해제, ${DEMO_ROOT} 삭제)`);
  process.exit(0);
}

if (reset) rmSync(DEMO_ROOT, { recursive: true, force: true });

if (existsSync(REPO)) {
  console.log(`이미 있습니다: ${REPO}`);
  console.log("처음부터 다시 만들려면: node dev/seed-demo.mjs --reset");
} else {
  // ── 저장소 만들기 ────────────────────────────────────────────────────────
  mkdirSync(DEMO_ROOT, { recursive: true });
  git(DEMO_ROOT, "init", "-q", "--bare", "-b", "main", ORIGIN);
  git(DEMO_ROOT, "clone", "-q", ORIGIN, REPO);

  git(REPO, "config", "user.email", "minji@example.com");
  git(REPO, "config", "user.name", "김민지");
  git(REPO, "config", "commit.gpgsign", "false");
  git(REPO, "checkout", "-q", "-B", "main");

  write(
    "src/api/user.ts",
    `export interface User {
  id: string;
  name: string;
}

export async function fetchUser(id: string): Promise<User> {
  return { id, name: "unknown" };
}
`,
  );
  write(
    "ui/views/LoginView.ts",
    `export function renderLogin(): string {
  return "login";
}
`,
  );
  write("README.md", "# demo-app\n\nGit Companion 미리보기용 데모 저장소입니다.\n");

  // 팀 규칙: main 으로만 병합하고, 병합 관리자는 데모 계정 minji.
  // 계정 자체는 팀 서버(`backend/`)의 users 테이블이 소유한다 — 아래에서
  // 서버가 떠 있으면 가입까지 해 준다.
  write(
    ".gpconfig",
    JSON.stringify(
      {
        gpconfig_version: 2,
        default_base_branch: "main",
        members: [
          { id: "u-me", name: "김민지", email: "minji@example.com", role: "admin" },
          { id: "u2", name: "박준호", email: "junho@example.com", role: "member" },
          { id: "u3", name: "이도윤", email: "doyoon@example.com", role: "member" },
        ],
        merge_managers: { main: "minji@example.com" },
        merge_targets: ["main"],
        notify_recipients: [],
        notify: { on_branch_ready: true, on_merge_complete: true },
      },
      null,
      2,
    ) + "\n",
  );

  git(REPO, "add", "-A");
  git(REPO, "commit", "-qm", "chore: 초기 구조와 프로젝트 설정");
  git(REPO, "push", "-q", "-u", "origin", "main");

  // ── 팀원 3명이 각자 브랜치에서 작업하고 push ─────────────────────────────
  // feature/login 과 feature/payment 는 같은 파일(src/api/user.ts)을 고친다 →
  // 변경 지도에 겹침 경고가 뜨고, 두 번째 병합에서 실제로 충돌한다.
  const branches = [
    {
      name: "feature/login",
      author: "김민지",
      email: "minji@example.com",
      message: "feat: 로그인 토큰 갱신 로직 추가",
      files: {
        "src/auth/token.ts": `export function refreshToken(token: string): string {
  return token + "-refreshed";
}
`,
        "src/api/user.ts": `export interface User {
  id: string;
  name: string;
  token?: string;
}

export async function fetchUser(id: string): Promise<User> {
  return { id, name: "unknown", token: "t" };
}
`,
      },
    },
    {
      name: "feature/payment",
      author: "박준호",
      email: "junho@example.com",
      message: "feat: 결제 실패 재시도",
      files: {
        "src/pay/retry.ts": `export function retry(attempts: number): number {
  return attempts - 1;
}
`,
        "src/api/user.ts": `export interface User {
  id: string;
  name: string;
  plan?: string;
}

export async function fetchUser(id: string): Promise<User> {
  return { id, name: "unknown", plan: "free" };
}
`,
      },
    },
    {
      name: "fix/nav",
      author: "이도윤",
      email: "doyoon@example.com",
      message: "fix: 사이드바 활성 상태 수정",
      files: {
        "ui/views/LoginView.ts": `export function renderLogin(): string {
  return "login-v2";
}
`,
      },
    },
  ];

  for (const b of branches) {
    git(REPO, "checkout", "-q", "main");
    git(REPO, "checkout", "-q", "-b", b.name);
    git(REPO, "config", "user.name", b.author);
    git(REPO, "config", "user.email", b.email);
    for (const [path, body] of Object.entries(b.files)) write(path, body);
    git(REPO, "add", "-A");
    git(REPO, "commit", "-qm", b.message);
    git(REPO, "push", "-q", "-u", "origin", b.name);
  }

  // 병합 관리자 시점으로 돌려 둔다.
  git(REPO, "checkout", "-q", "main");
  git(REPO, "config", "user.name", "김민지");
  git(REPO, "config", "user.email", "minji@example.com");

  console.log(`데모 저장소 생성: ${REPO}`);
  console.log(`  원격: ${ORIGIN}`);
  console.log(`  병합 대기 브랜치: ${branches.map((b) => b.name).join(", ")}`);
}

// ── 앱 설정에 등록 ──────────────────────────────────────────────────────────
const cfg = loadConfig();
cfg.repositories = cfg.repositories ?? [];
let entry = cfg.repositories.find((r) => r.path === REPO);
if (!entry) {
  entry = {
    id: randomUUID(),
    path: REPO,
    display_name: "demo-app",
    default_branch: "main",
    working_branch: "main",
    ssh_host: "",
    ssh_user: "",
    ssh_key_path: "",
    ssh_password: "",
    ed25519_fingerprint: "",
    ssh_port: 22,
    remote_url: ORIGIN,
    created_at: new Date().toISOString(),
  };
  cfg.repositories.push(entry);
}

// AI 자동 병합을 켜 둔다. 브릿지는 실제 LLM 을 호출하지 않으므로(미리보기에
// API 키가 없다) "AI 를 못 쓴 상태"와 같게 동작한다 — 즉 양쪽이 모두 고친
// 파일은 자동으로 한쪽을 고르지 않고 사람에게 넘기는 안전 규칙을 볼 수 있다.
// 실제 LLM 으로 시험하려면 설정 화면에서 Base URL·모델·API 키를 채우면 된다.
// 로그인 화면에서 서버 주소를 다시 입력하지 않도록 미리 넣어 둔다.
cfg.peer = { ...(cfg.peer ?? {}), backend_url: BACKEND_URL };

cfg.ai = {
  enabled: true,
  base_url: "",
  api_key: "",
  model: "",
  system_prompt: "",
  auto_resolve: true,
  binary_strategy: "theirs",
  ...(cfg.ai ?? {}),
};
saveConfig(cfg);

console.log(`앱 설정 등록: ${CONFIG}`);

// ── 데모 계정 ───────────────────────────────────────────────────────────────
//
// 계정은 팀 서버의 users 테이블(SQLite)이 소유한다. 서버가 떠 있으면 바로
// 가입시켜서 미리보기에서 로그인만 하면 되게 하고, 안 떠 있으면 서버를 띄우는
// 방법을 알려 준다 (앱은 로그인 없이는 쓸 수 없다).
const DEMO_USERS = [
  { name: "김민지", email: "minji@example.com", username: "minji", password: "minji-demo-pw" },
  { name: "박준호", email: "junho@example.com", username: "junho", password: "junho-demo-pw" },
];

const usable = [];
const existsWithOtherPassword = [];
let serverUp = false;
try {
  const health = await fetch(`${BACKEND_URL}/healthz`, { signal: AbortSignal.timeout(3000) });
  serverUp = health.ok;
} catch {
  serverUp = false;
}

/** 이 아이디/비밀번호로 실제 로그인되는지 확인. */
async function canLogIn(u) {
  try {
    const r = await fetch(`${BACKEND_URL}/auth/login`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ username: u.username, password: u.password }),
    });
    return r.ok;
  } catch {
    return false;
  }
}

if (serverUp) {
  for (const u of DEMO_USERS) {
    try {
      const r = await fetch(`${BACKEND_URL}/auth/register`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(u),
      });
      if (r.ok) {
        usable.push(u);
      } else if (r.status === 409) {
        // 이미 있는 계정이다. 비밀번호가 데모 값과 같은지 **확인해야** 한다 —
        // 누군가 바꿨을 수 있고, 그때 데모 비밀번호를 안내하면 로그인이 안 되는
        // 이유를 찾느라 시간을 버린다.
        if (await canLogIn(u)) usable.push(u);
        else existsWithOtherPassword.push(u);
      }
    } catch {
      /* 다음 사람으로 */
    }
  }
}

console.log("");
if (serverUp && (usable.length > 0 || existsWithOtherPassword.length > 0)) {
  console.log(`팀 서버: ${BACKEND_URL} (실행 중)`);
  if (usable.length > 0) {
    console.log("로그인:");
    for (const u of usable) {
      const role = u.username === "minji" ? "병합 관리자" : "일반 팀원";
      console.log(`  ${u.username} / ${u.password}   (${u.name}, ${role})`);
    }
  }
  for (const u of existsWithOtherPassword) {
    console.log(
      `  ⚠ ${u.username} 계정은 이미 있지만 비밀번호가 데모 값과 다릅니다 — 아는 비밀번호로 로그인하거나 서버 DB(gc_peer.db)를 지우고 다시 실행하세요.`,
    );
  }
} else {
  console.log(`팀 서버(${BACKEND_URL})가 응답하지 않습니다. 계정은 서버에 저장되므로 먼저 띄우세요:`);
  console.log("  cd backend && python3.11 -m venv .venv && ./.venv/bin/pip install -e '.[dev]'");
  console.log("  ./.venv/bin/uvicorn app.main:app --port 8000");
  console.log("그리고 다시: pnpm seed:demo");
}
console.log("데모를 지우려면: node dev/seed-demo.mjs --clean");
