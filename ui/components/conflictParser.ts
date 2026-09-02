// Pure parser for git conflict markers — column-0 only.
//
// Recognises the canonical markers:
//   <<<<<<< <label>
//   =======
//   >>>>>>> <label>
// Indented lookalikes (`    <<<<<<<`) are *not* treated as markers; they are
// preserved as content. That matches git's own behaviour for files that
// already contained the literal strings.

export interface ConflictBlock {
  /** 1-based start line of `<<<<<<<` (inclusive). */
  startLine: number;
  /** 1-based end line of `>>>>>>>` (inclusive). */
  endLine: number;
  ours: string;
  theirs: string;
}

// CRLF 파일은 마커 줄 자체가 `=======\r` 로 끝난다 — `\r?` 를 허용하지 않으면
// Windows 팀원의 충돌이 통째로 "블록 0개"가 된다.
const START_RE = /^<<<<<<< /;
// diff3/zdiff3 conflictstyle 의 base 구간 마커 (`||||||| <label>`). 이걸
// 모르면 base 본문 전체가 ours 에 섞여 "내 것 선택"이 쓰레기를 커밋한다.
const BASE_RE = /^\|\|\|\|\|\|\|( |\r?$)/;
const MID_RE = /^=======\r?$/;
const END_RE = /^>>>>>>> /;

/**
 * Walk the content line-by-line and yield each conflict block. Lines outside
 * any block are silently dropped — callers are responsible for stitching the
 * untouched segments back together if they need them.
 *
 * 방어 규칙: 시작 마커는 **종료 마커 짝이 확인될 때만** 블록으로 인정한다.
 * 문서 예시로 든 `<<<<<<< sample` 한 줄이 진짜 블록까지 삼키지 않도록,
 * 종료 마커 앞의 마지막 시작 마커를 블록 시작으로 잡는다.
 */
export function parseConflictBlocks(content: string): ConflictBlock[] {
  const lines = content.split("\n");
  const out: ConflictBlock[] = [];
  let i = 0;
  while (i < lines.length) {
    if (!START_RE.test(lines[i] ?? "")) {
      i++;
      continue;
    }
    // 짝이 되는 종료 마커부터 찾는다 — 없으면 이 줄은 그냥 내용이다.
    let k = i + 1;
    while (k < lines.length && !END_RE.test(lines[k] ?? "")) {
      k++;
    }
    if (k >= lines.length) {
      i++;
      continue;
    }
    // 종료 마커 앞의 마지막 시작 마커가 진짜 블록 시작이다.
    let s = i;
    for (let j = i + 1; j < k; j++) {
      if (START_RE.test(lines[j] ?? "")) s = j;
    }
    // diff3 base 구간(있으면)과 ======= 구분선을 찾는다.
    let baseAt = -1;
    let midAt = -1;
    for (let j = s + 1; j < k; j++) {
      if (baseAt < 0 && midAt < 0 && BASE_RE.test(lines[j] ?? "")) {
        baseAt = j;
        continue;
      }
      if (midAt < 0 && MID_RE.test(lines[j] ?? "")) {
        midAt = j;
      }
    }
    if (midAt < 0) {
      // 구분선 없는 시작/종료 짝 — git 이 만드는 형태가 아니므로 내용 취급.
      i = s + 1;
      continue;
    }
    const oursEnd = baseAt >= 0 ? baseAt : midAt;
    out.push({
      startLine: s + 1,
      endLine: k + 1,
      ours: lines.slice(s + 1, oursEnd).join("\n"),
      theirs: lines.slice(midAt + 1, k).join("\n"),
    });
    i = k + 1;
  }
  return out;
}

/**
 * Reassemble a file from head/tail segments and edited block bodies.
 * Caller supplies one replacement per block, in order.
 */
export function reassemble(
  content: string,
  blocks: ConflictBlock[],
  replacements: string[],
): string {
  if (replacements.length !== blocks.length) {
    throw new Error(
      `replacements (${replacements.length}) does not match blocks (${blocks.length})`,
    );
  }
  const lines = content.split("\n");
  let out: string[] = [];
  let cursor = 1; // 1-based
  for (let i = 0; i < blocks.length; i++) {
    const b = blocks[i]!;
    const before = lines.slice(cursor - 1, b.startLine - 1);
    out = out.concat(before);
    // 빈 교체는 "블록을 통째로 지운다"는 뜻 — 빈 줄을 남기지 않는다.
    const rep = replacements[i] ?? "";
    if (rep !== "") {
      out.push(rep);
    }
    cursor = b.endLine + 1;
  }
  if (cursor - 1 < lines.length) {
    out = out.concat(lines.slice(cursor - 1));
  }
  return out.join("\n");
}
