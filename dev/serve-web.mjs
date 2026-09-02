// 브라우저에서 보면서 작업하기 위한 개발 서버 런처.
//
//   pnpm dev:web
//
// 이 개발 머신은 code-server(브라우저 VS Code) 뒤에 있어서 localhost 로는
// 볼 수 없다. code-server 에는 `/absproxy/<port>/` 포트 프록시가 내장되어
// 있으므로, vite 를 그 경로를 base 로 띄우고 접속할 URL 을 출력한다.
//
// 필요하면 환경변수로 바꿀 수 있다:
//   GC_PORT=5174 pnpm dev:web       다른 포트로
//   GC_BASE=/ pnpm dev:web          프록시 없이 (SSH 터널로 볼 때)
//   GC_HOST=0.0.0.0 pnpm dev:web    직접 접속 허용 (보안 그룹이 열려 있을 때)
import { spawn, execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const port = process.env.GC_PORT || "5173";
const host = process.env.GC_HOST || "127.0.0.1";
// `/absproxy/<port>/` 는 경로를 그대로 앱에 넘긴다 — 그래서 vite base 와
// 반드시 같아야 한다. (`/proxy/`는 접두사를 떼어내므로 절대경로 자산이 깨진다.)
const base = process.env.GC_BASE ?? `/absproxy/${port}/`;

/** code-server 가 이 머신에서 돌고 있으면 그 포트, 아니면 null. */
function codeServerPort() {
  // ss 의 프로세스 이름은 code-server 가 아니라 "MainThread" 로 나오므로
  // 이름으로 찾을 수 없다. 설정 파일의 bind-addr 를 읽고 실제로 리스닝
  // 중인지 확인한다.
  const cfg = join(homedir(), ".config", "code-server", "config.yaml");
  let port = null;
  if (existsSync(cfg)) {
    const m = readFileSync(cfg, "utf8").match(/^bind-addr:\s*\S*?:(\d+)/m);
    if (m) port = m[1];
  }
  port = port ?? "8080";
  try {
    const out = execFileSync("bash", ["-lc", `ss -tln 2>/dev/null | grep -c ':${port} ' || true`], {
      encoding: "utf8",
    });
    return Number(out.trim()) > 0 ? port : null;
  } catch {
    return null;
  }
}

const demoRepo = join(homedir(), "gc-demo", "demo-app");
const csPort = codeServerPort();

console.log("");
console.log("─".repeat(70));
if (base === "/") {
  console.log(`  개발 서버: http://localhost:${port}/`);
  console.log("");
  console.log("  로컬 PC 에서 보려면 터미널에서 SSH 터널을 여세요:");
  console.log(`    ssh -N -L ${port}:127.0.0.1:${port} <이 서버>`);
  console.log(`  그리고 브라우저에서 http://localhost:${port}/ 를 엽니다.`);
} else {
  console.log("  브라우저에서 열 주소 — 지금 code-server 를 보고 있는 탭의");
  console.log("  주소창에서 도메인만 남기고 아래 경로를 붙이세요:");
  console.log("");
  console.log(`      https://<code-server 도메인>/absproxy/${port}/`);
  console.log("");
  console.log("  (예: https://d1234abcd.cloudfront.net/absproxy/5173/)");
  if (!csPort) {
    console.log("");
    console.log("  ⚠ code-server 를 찾지 못했습니다. 프록시 없이 보려면:");
    console.log(`      GC_BASE=/ pnpm dev:web`);
  }
}
console.log("");
if (existsSync(demoRepo)) {
  console.log(`  데모 저장소: ${demoRepo} (병합 대기 3건)`);
} else {
  console.log("  데모 데이터가 없습니다. 먼저 실행하세요:  pnpm seed:demo");
}
console.log("  로그인: minji / minji-demo-pw (병합 관리자)  ·  junho / junho-demo-pw (팀원)");
console.log("  계정은 팀 서버에 저장됩니다 — pnpm seed:demo 가 서버에 가입까지 해 줍니다.");
console.log("─".repeat(70));
console.log("");

// HMR 소켓은 프록시를 거치므로 브라우저가 붙을 포트/프로토콜을 알려 준다.
// CloudFront → nginx → code-server 경로는 https 이므로 443/wss 가 기본이다.
const env = { ...process.env, GC_BASE: base, GC_PORT: port, GC_HOST: host };
if (base !== "/" && !process.env.GC_HMR_CLIENT_PORT) {
  env.GC_HMR_CLIENT_PORT = "443";
  env.GC_HMR_PROTOCOL = "wss";
}

// Vite 5.4+ 의 Host 헤더 검사. 프록시를 거치면 Host 가 프록시 도메인이 되어
// 그대로는 "Blocked request" 가 뜬다. 도메인은 배포마다 다르고 미리 알 수
// 없으므로, 루프백에 바인드된 상태(= 이 머신 안의 프록시만 접속 가능)에서는
// 전부 허용한다. 0.0.0.0 으로 열었다면 함부로 열지 않고 사용자가 직접
// GC_ALLOWED_HOSTS 를 지정하게 한다.
if (!process.env.GC_ALLOWED_HOSTS) {
  const loopback = host === "127.0.0.1" || host === "localhost" || host === "::1";
  if (loopback) env.GC_ALLOWED_HOSTS = "all";
}

const vite = spawn("npx", ["vite"], { stdio: "inherit", env });
vite.on("exit", (code) => process.exit(code ?? 0));
