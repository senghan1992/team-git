// tauri build 가 시작되기 전에 남아 있는 앱 프로세스를 정리한다.
//
// Windows 는 **실행 중인 exe 파일을 지우거나 덮어쓸 수 없다** — 앱(또는
// 백그라운드 리스너 gc-peer-listener)이 켜져 있는 채로 빌드하면
//
//   error: failed to remove file `...\target\release\gc-peer-listener.exe`
//   Caused by: 액세스가 거부되었습니다. (os error 5)
//
// 로 빌드가 죽는다. 예전 버전은 앱이 꺼져도 리스너가 남았으므로 특히 자주
// 그랬다. 그래서 beforeBuildCommand 에서 이 스크립트를 먼저 돌려 싹 정리하고
// 빌드한다 (tauri.conf.json 의 beforeBuildCommand 참고).
//
// 사용법: node dev/tauri-pre.mjs build   — 정리 후 `pnpm build` 실행 (빌드 전용)
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const isWindows = process.platform === "win32";
const repoRoot = fileURLToPath(new URL("..", import.meta.url));

// 실행 중이면 파일을 잠그는 프로세스들 — 전부 강제 종료 (없으면 그냥 지나간다).
const kills = isWindows
  ? [
      ["taskkill", ["/F", "/IM", "gc-peer-listener.exe"]],
      ["taskkill", ["/F", "/IM", "git-companion.exe"]],
      ["taskkill", ["/F", "/IM", "Git Companion.exe"]],
    ]
  : [["pkill", ["-f", "gc-peer-listener"]]];

for (const [cmd, args] of kills) {
  try {
    spawnSync(cmd, args, { stdio: "ignore" });
  } catch {
    // 도구가 없거나 프로세스가 없으면 그대로 진행 — 정리는 최선노력이다.
  }
}

// 실제 프론트엔드 빌드. (vite 결과물이 src-tauri 에 번들로 들어간다.)
const result = spawnSync("pnpm", ["build"], {
  cwd: repoRoot,
  stdio: "inherit",
  shell: isWindows,
});
process.exit(result.status ?? 1);
