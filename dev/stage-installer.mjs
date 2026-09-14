// 빌드된 설치 파일을 `release/` 폴더로 스테이징한다 — git 으로 팀원에게
// 설치 파일을 공유하기 위한 첫 단계다.
//
//   사용법 (Windows):
//     pnpm tauri build
//     node dev/stage-installer.mjs
//
// 이 스크립트는 `src-tauri/target/release/bundle/` 아래의 설치 파일을
// `release/` 폴더로 버전 번호가 들어간 이름으로 복사하고, 그 다음에 실행할
// git 명령 두 줄을 안내한다. 실행 중인 앱은 `tauri build` 가 이미 정리하므로
// (dev/tauri-pre.mjs) 별도로 끌 필요가 없다.
import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readdirSync, statSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join } from "node:path";

const repoRoot = fileURLToPath(new URL("..", import.meta.url));
const version = JSON.parse(readFileSync(join(repoRoot, "package.json"), "utf8")).version;

// 번들 형식 → 대상 폴더와 파일 이름 규칙.
// NSIS 가 만드는 원래 파일명은 `Git Companion_0.1.6_x64-setup.exe` 처럼
// 띄어쓰기·밑줄이 섞여 있어 git 저장소에서 다루기 불편하다 — 여기서
// `Git-Companion-0.1.6-setup.exe` 로 정리해 복사한다.
const BUNDLES = [
  {
    dir: ["bundle", "nsis"],
    match: /-setup\.exe$/i,
    name: `Git-Companion-${version}-setup.exe`,
  },
  {
    dir: ["bundle", "msi"],
    match: /\.msi$/i,
    name: `Git-Companion-${version}.msi`,
  },
  {
    dir: ["bundle", "deb"],
    match: /\.deb$/i,
    name: `git-companion_${version}_amd64.deb`,
  },
  {
    dir: ["bundle", "appimage"],
    match: /\.AppImage$/i,
    name: `Git-Companion-${version}.AppImage`,
  },
  {
    dir: ["bundle", "macos"],
    match: /\.app$/i,
    name: `Git-Companion-${version}.app`,
  },
];

// 빌드 출력물 위치는 환경마다 다르다:
//   Windows 네이티브  : src-tauri/target/release
//   Linux 크로스빌드  : target/<triple>/release  (워크스페이스 루트 target/)
//   그 외             : src-tauri/target/<triple>/release
// 실제로 존재하는 디렉터리를 후보 순서대로 찾아 사용한다.
const candidates = [
  join(repoRoot, "src-tauri", "target", "release"),
  join(repoRoot, "target", "release"),
  join(repoRoot, "target", "x86_64-pc-windows-gnu", "release"),
  join(repoRoot, "src-tauri", "target", "x86_64-pc-windows-gnu", "release"),
  join(repoRoot, "target", "aarch64-pc-windows-msvc", "release"),
  join(repoRoot, "src-tauri", "target", "aarch64-pc-windows-msvc", "release"),
];
const bundleRoot = candidates.find((dir) => existsSync(join(dir, "bundle")));
const releaseDir = join(repoRoot, "release");
mkdirSync(releaseDir, { recursive: true });

let copied = 0;
for (const spec of BUNDLES) {
  const srcDir = join(bundleRoot, ...spec.dir);
  if (!existsSync(srcDir)) continue;
  // 같은 종류가 여러 개면(예: 이전 버전 잔재) 가장 최신 파일만 고른다.
  const matches = readdirSync(srcDir)
    .map((entry) => {
      const src = join(srcDir, entry);
      return { entry, src, mtime: statSync(src).mtimeMs };
    })
    .filter((f) => statSync(f.src).isFile() && spec.match.test(f.entry))
    .sort((a, b) => b.mtime - a.mtime);
  if (matches.length === 0) continue;
  const { src } = matches[0];
  const dst = join(releaseDir, spec.name);
  copyFileSync(src, dst);
  console.log(`  ✓ ${spec.name}  (${(statSync(dst).size / 1024 / 1024).toFixed(1)} MB)`);
  copied += 1;
}

if (copied === 0) {
  console.error(
    `[스테이징 실패] 빌드 출력물(bundle/ 디렉터리)을 찾지 못했습니다.\n` +
      "  먼저 `pnpm tauri build` 를 실행해 설치 파일을 만든 뒤 다시 실행하세요.",
  );
  process.exit(1);
}

console.log(`\n설치 파일 ${copied}개를 release/ 폴더에 모았습니다 (v${version}).`);
console.log("이제 git 으로 공유하려면 아래 명령을 그대로 실행하세요:\n");
console.log(`  git add release/`);
console.log(`  git commit -m "release: v${version} 설치 파일"`);
console.log(`  git push`);
console.log("\n(팀원은 `git pull` 후 release/ 폴더의 설치 파일을 실행하면 됩니다.)");