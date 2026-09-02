import { defineConfig, type Plugin } from "vite";
import tailwindcss from "@tailwindcss/vite";
import { gitBridgePlugin } from "./dev/git-bridge";

// 개발 서버를 리버스 프록시(code-server 의 `/absproxy/<port>/` 등) 아래에서
// 볼 수 있게 하는 설정. 모두 환경변수로만 켜지므로 로컬 `pnpm dev` 동작은
// 그대로다. `pnpm dev:web` 이 이 값들을 채워 준다.
//
//   GC_BASE              vite base path (예: "/absproxy/5173/")
//   GC_HOST              바인드 주소 (기본 127.0.0.1)
//   GC_PORT              포트 (기본 5173)
//   GC_HMR_CLIENT_PORT   브라우저가 HMR 소켓에 접속할 포트 (https 프록시면 443)
//   GC_HMR_PROTOCOL      "ws" | "wss"
//   GC_ALLOWED_HOSTS     "all" 또는 쉼표로 구분한 호스트 목록
const host = process.env.GC_HOST || "127.0.0.1";
const port = Number(process.env.GC_PORT || 5173);
const hmrClientPort = process.env.GC_HMR_CLIENT_PORT
  ? Number(process.env.GC_HMR_CLIENT_PORT)
  : undefined;

// Vite 5.4+ 는 DNS 리바인딩을 막기 위해 Host 헤더를 검사한다. 리버스 프록시를
// 거치면 Host 가 프록시의 도메인(예: *.cloudfront.net)이 되므로 그대로는
// "Blocked request" 가 뜬다. 도메인은 배포마다 다르니 환경변수로 받는다.
//
// 서버를 루프백(127.0.0.1)에 바인드한 채라면 이 머신 안의 프록시만 접속할 수
// 있으므로 "all" 로 열어도 외부에 노출되지 않는다 — `dev:web` 이 그 조건일
// 때만 자동으로 "all" 을 넣는다.
const allowedHosts: true | string[] | undefined = (() => {
  const raw = process.env.GC_ALLOWED_HOSTS?.trim();
  if (!raw) return undefined;
  if (raw === "all") return true;
  return raw
    .split(",")
    .map((h) => h.trim())
    .filter(Boolean);
})();

/**
 * base 를 슬래시 없이 입력해도 열리게 한다.
 *
 * `/absproxy/5173/` 를 base 로 띄우면 vite 는 `/absproxy/5173` (슬래시 없음)에
 * 404 를 준다. 사람이 주소를 손으로 칠 때 끝 슬래시는 거의 빠지므로, 그때마다
 * 404 를 보여 주는 대신 슬래시 붙은 주소로 리다이렉트한다.
 */
function baseRedirectPlugin(base: string): Plugin {
  return {
    name: "gc-base-redirect",
    configureServer(server) {
      if (base === "/") return;
      const withoutSlash = base.replace(/\/$/, "");
      server.middlewares.use((req, res, next) => {
        const [path, query] = (req.url ?? "").split("?");
        if (path !== withoutSlash) {
          next();
          return;
        }
        res.statusCode = 302;
        res.setHeader("Location", base + (query ? `?${query}` : ""));
        res.end();
      });
    },
  };
}

export default defineConfig(({ command }) => ({
  // base 는 개발 서버에서만 적용한다. Tauri 번들은 `tauri://` 로 로드되므로
  // 프록시 경로가 박히면 자산을 못 찾는다 — 셸에 GC_BASE 가 남아 있는 채로
  // `pnpm build` 를 돌려도 안전해야 한다.
  base: command === "serve" ? process.env.GC_BASE || "/" : "/",
  plugins: [
    baseRedirectPlugin(command === "serve" ? process.env.GC_BASE || "/" : "/"),
    tailwindcss(),
    gitBridgePlugin(),
  ],
  clearScreen: false,
  server: {
    host,
    port,
    strictPort: true,
    ...(allowedHosts === undefined ? {} : { allowedHosts }),
    // 프록시를 거치면 브라우저가 보는 호스트/포트가 서버와 다르다. 알려 주지
    // 않으면 HMR 소켓이 localhost:5173 으로 붙으려다 실패한다. 값이 없으면
    // vite 기본 동작(직접 접속)을 그대로 쓴다.
    hmr: hmrClientPort
      ? {
          clientPort: hmrClientPort,
          protocol: process.env.GC_HMR_PROTOCOL === "ws" ? "ws" : "wss",
        }
      : undefined,
  },
  build: {
    target: "esnext",
    outDir: "dist",
    emptyOutDir: true,
  },
}));
