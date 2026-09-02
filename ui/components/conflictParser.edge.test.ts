// Edge-case tests for the frontend conflict parser (CONFLICT HELL cluster).
// Bun/node runner — same invocation as conflictParser.test.ts (pnpm test:ui).
//
// 아래 BUG-n 은 conflictParser.ts 에서 수정 완료 — 각 케이스는 수정된 새
// 동작을 고정하는 회귀 테스트다. 번호는 findings 리포트와 같다:
//   BUG-1  diff3 의 `||||||| <label>` base 구간을 인식 — ours 에 안 샌다
//   BUG-3  CRLF 마커 줄("=======\r")을 인식 — 본문의 \r 는 보존
//   BUG-7  짝 없는 `<<<<<<< ` 는 내용 취급 (내부 `=======` 모호성은 한계로 유지)
//   BUG-8  reassemble 의 빈 교체는 블록 삭제 — 빈 줄을 남기지 않음
import { parseConflictBlocks, reassemble } from "./conflictParser";

function assert(cond: boolean, msg: string) {
  if (!cond) throw new Error(`ASSERTION FAILED: ${msg}`);
}

// ── BUG-3: CRLF files ─────────────────────────────────────────────────────────
// git (verified with 2.43) writes the conflict MARKER lines themselves with
// \r\n when the file uses CRLF. 회귀 노트(BUG-3 수정): 마커 정규식이 끝의 \r
// 를 허용하므로 Windows 줄끝 충돌도 정상 파싱되고, 본문 줄의 \r 는 그대로
// 붙어 있어 CRLF 가 바이트 단위로 왕복한다.
{
  const crlf =
    "one\r\n" +
    "<<<<<<< HEAD\r\n" +
    "two-main\r\n" +
    "=======\r\n" +
    "two-side\r\n" +
    ">>>>>>> feature/x\r\n" +
    "three\r\n";
  const blocks = parseConflictBlocks(crlf);
  assert(blocks.length === 1, `CRLF parses to 1 block, got ${blocks.length}`);
  assert(blocks[0]!.ours === "two-main\r", `ours keeps its \\r: ${JSON.stringify(blocks[0]!.ours)}`);
  assert(blocks[0]!.theirs === "two-side\r", `theirs keeps its \\r: ${JSON.stringify(blocks[0]!.theirs)}`);
  assert(blocks[0]!.startLine === 2 && blocks[0]!.endLine === 6, "marker lines located");
  // reassemble 은 손대지 않은 CRLF 줄을 그대로 보존한다.
  const out = reassemble(crlf, blocks, [blocks[0]!.ours]);
  assert(
    out === "one\r\ntwo-main\r\nthree\r\n",
    `CRLF round trip: ${JSON.stringify(out)}`,
  );
  console.log("PASS: BUG-3 fixed — CRLF conflict parses and round-trips");
}

// ── BUG-1: diff3 / zdiff3 conflict style ─────────────────────────────────────
// With merge.conflictstyle=diff3 (set globally on real dev machines — including
// this one) git inserts a `||||||| <label>` base section. 회귀 노트(BUG-1
// 수정): 파서가 base 마커를 인식해 ours 는 base 마커 앞에서 끝나고, base
// 본문/라벨은 ours 에도 theirs 에도 새지 않는다 — "내 것 사용"이 더는
// 쓰레기를 커밋하지 않는다.
{
  const diff3 = [
    "head",
    "<<<<<<< HEAD",
    "ours line",
    "||||||| 1234abc",
    "base line",
    "=======",
    "theirs line",
    ">>>>>>> feature/x",
    "tail",
  ].join("\n");
  const blocks = parseConflictBlocks(diff3);
  assert(blocks.length === 1, `diff3: one block, got ${blocks.length}`);
  assert(
    blocks[0]!.ours === "ours line",
    `diff3 base section dropped from ours: ${JSON.stringify(blocks[0]!.ours)}`,
  );
  assert(blocks[0]!.theirs === "theirs line", "theirs is unaffected");
  assert(blocks[0]!.startLine === 2 && blocks[0]!.endLine === 8, "block spans marker to marker");
  console.log("PASS: BUG-1 fixed — diff3 base section is recognised and dropped");
}

// ── BUG-7a: legitimate 7-char "=======" inside a conflicted region ──────────
// A markdown setext underline that is EXACTLY 7 '=' characters is
// indistinguishable from the middle marker. 알려진 한계(의도적으로 유지):
// 텍스트만으로는 어느 `=======` 이 진짜 구분선인지 판별할 수 없어, 파서는
// 첫 번째 후보를 구분선으로 삼는다 — ours 가 정당한 밑줄에서 잘리고 진짜
// 구분선은 theirs 에 섞인다. 해소하려면 conflict_detail 의 스테이지 본문과
// 대조하는 상위 레이어 검증이 필요하다.
{
  const input = [
    "<<<<<<< HEAD",
    "Heading",
    "=======", // legit setext underline (7 chars) — part of ours' content
    "more ours",
    "=======", // the REAL divider written by git
    "theirs body",
    ">>>>>>> feature/x",
  ].join("\n");
  const blocks = parseConflictBlocks(input);
  assert(blocks.length === 1, `legit =======: one block, got ${blocks.length}`);
  assert(blocks[0]!.ours === "Heading", `ours cut at the first divider candidate: ${blocks[0]!.ours}`);
  assert(
    blocks[0]!.theirs === "more ours\n=======\ntheirs body",
    `rest lands in theirs (known limitation): ${JSON.stringify(blocks[0]!.theirs)}`,
  );
  console.log("PASS: BUG-7a known limitation — exact-7 ======= ambiguity keeps first-divider rule");
}

// A longer underline ("=========", 9 chars) does NOT match MID_RE and parses
// correctly — the ambiguity is only the exact-7 form.
{
  const input = [
    "<<<<<<< HEAD",
    "Heading",
    "=========",
    "more ours",
    "=======",
    "theirs body",
    ">>>>>>> feature/x",
  ].join("\n");
  const blocks = parseConflictBlocks(input);
  assert(blocks.length === 1, "9-char underline: one block");
  assert(
    blocks[0]!.ours === "Heading\n=========\nmore ours",
    `9-char underline stays in ours: ${JSON.stringify(blocks[0]!.ours)}`,
  );
  assert(blocks[0]!.theirs === "theirs body", "theirs correct");
  console.log("PASS: 9-char setext underline parses correctly");
}

// ── BUG-7b: stray column-0 "<<<<<<< " content BEFORE a real block ────────────
// Documentation that shows what conflict markers look like (fixtures, guides)
// legitimately contains "<<<<<<< yours" at column 0. 회귀 노트(BUG-7b 수정):
// 종료 마커 앞의 마지막 시작 마커가 블록 시작이 되므로, 문서 예시 줄은 내용
// 취급되고 진짜 블록만 단독으로 파싱된다.
{
  const input = [
    "markers look like:",
    "<<<<<<< yours", // legit content (docs sample)
    "(example)",
    "<<<<<<< HEAD", // real conflict starts here
    "real ours",
    "=======",
    "real theirs",
    ">>>>>>> feature/x",
  ].join("\n");
  const blocks = parseConflictBlocks(input);
  assert(blocks.length === 1, `stray start: one real block, got ${blocks.length}`);
  assert(blocks[0]!.startLine === 4, `block starts at the REAL marker: ${blocks[0]!.startLine}`);
  assert(blocks[0]!.ours === "real ours", `ours is only the real body: ${JSON.stringify(blocks[0]!.ours)}`);
  assert(blocks[0]!.theirs === "real theirs", "theirs body correct");
  // 문서 예시 줄은 해결 후에도 살아남는다.
  const out = reassemble(input, blocks, [blocks[0]!.ours]);
  assert(
    out === "markers look like:\n<<<<<<< yours\n(example)\nreal ours",
    `docs sample survives resolution: ${JSON.stringify(out)}`,
  );
  console.log("PASS: BUG-7b fixed — stray column-0 <<<<<<< stays content, real block parses alone");
}

// A stray "<<<<<<< " with NO real block after it: 짝이 되는 `>>>>>>>` 가
// 없으면 그냥 내용이다 — 블록을 지어내지 않는다 (BUG-7 수정으로 보장).
{
  const input = ["docs:", "<<<<<<< sample", "no real conflict here"].join("\n");
  assert(parseConflictBlocks(input).length === 0, "unterminated lookalike yields no blocks");
  console.log("PASS: unterminated marker lookalike is plain content");
}

// ── reassemble round-trips ───────────────────────────────────────────────────
// Untouched head/tail must be byte-identical, including trailing-newline
// state and odd whitespace.
{
  const input = [
    "head  ", // trailing spaces preserved
    "\tindented\t",
    "<<<<<<< HEAD",
    "ours",
    "=======",
    "theirs",
    ">>>>>>> feature/x",
    "tail",
    "", // trailing newline
  ].join("\n");
  const blocks = parseConflictBlocks(input);
  const out = reassemble(input, blocks, [blocks[0]!.ours]);
  assert(
    out === "head  \n\tindented\t\nours\ntail\n",
    `round trip with trailing newline: ${JSON.stringify(out)}`,
  );
  console.log("PASS: reassemble preserves untouched bytes and trailing newline");
}

// File WITHOUT trailing newline, block at EOF.
{
  const input = "head\n<<<<<<< HEAD\no\n=======\nt\n>>>>>>> x"; // no trailing \n
  const blocks = parseConflictBlocks(input);
  assert(blocks.length === 1, "EOF block parsed");
  const out = reassemble(input, blocks, ["o"]);
  assert(out === "head\no", `no spurious trailing newline: ${JSON.stringify(out)}`);
  console.log("PASS: reassemble keeps missing trailing newline");
}

// Multi-line replacement (manual merge of both sides) splices exactly.
{
  const input = "a\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> x\nz\n";
  const blocks = parseConflictBlocks(input);
  const out = reassemble(input, blocks, ["ours\ntheirs"]);
  assert(out === "a\nours\ntheirs\nz\n", `multi-line replacement: ${JSON.stringify(out)}`);
  console.log("PASS: multi-line manual replacement splices exactly");
}

// CRLF content lines inside an LF-marker conflict (mixed-ending file, e.g.
// only SOME lines were saved by a Windows editor): parses, and the \r stays
// attached to the body so a Manual write round-trips the bytes.
{
  const input = "a\n<<<<<<< HEAD\nours-crlf\r\n=======\ntheirs\n>>>>>>> x\nz\n";
  const blocks = parseConflictBlocks(input);
  assert(blocks.length === 1, "mixed-ending block parsed");
  assert(blocks[0]!.ours === "ours-crlf\r", `body keeps its \\r: ${JSON.stringify(blocks[0]!.ours)}`);
  const out = reassemble(input, blocks, [blocks[0]!.ours]);
  assert(out === "a\nours-crlf\r\nz\n", `mixed-ending round trip: ${JSON.stringify(out)}`);
  console.log("PASS: CRLF content lines survive when markers are LF");
}

// ── BUG-8: empty replacement deletes the block ───────────────────────────────
// 회귀 노트(BUG-8 수정): 빈 문자열 교체는 "블록을 통째로 지운다"는 뜻 —
// 빈 줄을 남기지 않으므로 "양쪽 모두 이 hunk 를 삭제"가 표현 가능하다.
{
  const input = "a\n<<<<<<< HEAD\no\n=======\nt\n>>>>>>> x\nb\n";
  const blocks = parseConflictBlocks(input);
  const out = reassemble(input, blocks, [""]);
  assert(out === "a\nb\n", `empty replacement deletes the block: ${JSON.stringify(out)}`);
  console.log("PASS: BUG-8 fixed — empty replacement deletes the block, no blank line");
}

// replacements/blocks mismatch must throw, not corrupt.
{
  const input = "a\n<<<<<<< HEAD\no\n=======\nt\n>>>>>>> x\nb\n";
  const blocks = parseConflictBlocks(input);
  let threw = false;
  try {
    reassemble(input, blocks, []);
  } catch {
    threw = true;
  }
  assert(threw, "mismatched replacement count throws");
  console.log("PASS: replacement count mismatch throws");
}
