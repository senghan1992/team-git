// UI 단위 테스트 러너.
//
// `ui/**/*.test.ts`는 프레임워크 없이 자체 `assert()`로 검증하고 실패하면
// throw 한다. 별도 테스트 러너를 의존성에 넣지 않기 위해, 이미 vite가 들고
// 있는 esbuild로 각 파일을 번들해 node로 실행한다.
//
//   pnpm test:ui
import { execFileSync } from "node:child_process";
import { mkdtempSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, relative } from "node:path";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const esbuild = require("esbuild");

const root = new URL("..", import.meta.url).pathname;

/** ui/ 아래의 모든 *.test.ts 를 찾는다. */
function findTests(dir) {
  const out = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) out.push(...findTests(full));
    else if (entry.name.endsWith(".test.ts")) out.push(full);
  }
  return out;
}

const tests = findTests(join(root, "ui")).sort();
if (tests.length === 0) {
  console.error("테스트 파일을 찾지 못했습니다 (ui/**/*.test.ts).");
  process.exit(1);
}

const work = mkdtempSync(join(tmpdir(), "gc-ui-test-"));
let failed = 0;

try {
  for (const file of tests) {
    const name = relative(root, file);
    const bundle = join(work, name.replace(/[\\/]/g, "_") + ".mjs");
    try {
      esbuild.buildSync({
        entryPoints: [file],
        bundle: true,
        format: "esm",
        platform: "node",
        outfile: bundle,
        logLevel: "error",
      });
      const out = execFileSync(process.execPath, [bundle], { encoding: "utf8" });
      process.stdout.write(out);
      console.log(`✓ ${name}`);
    } catch (e) {
      failed += 1;
      console.log(`✗ ${name}`);
      const detail = e.stdout || "";
      if (detail) process.stdout.write(detail);
      process.stderr.write(String(e.stderr || e.message) + "\n");
    }
  }
} finally {
  rmSync(work, { recursive: true, force: true });
}

console.log(
  failed === 0
    ? `\n모든 UI 테스트 통과 (${tests.length}개 파일)`
    : `\n${failed}/${tests.length}개 파일 실패`,
);
process.exit(failed === 0 ? 0 : 1);
