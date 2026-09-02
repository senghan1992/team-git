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

const START_RE = /^<<<<<<< /;
const MID_RE = /^=======$/;
const END_RE = /^>>>>>>> /;

/**
 * Walk the content line-by-line and yield each conflict block. Lines outside
 * any block are silently dropped — callers are responsible for stitching the
 * untouched segments back together if they need them.
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
    const startLine = i + 1;
    // Scan for the ======= marker.
    let j = i + 1;
    while (j < lines.length && !MID_RE.test(lines[j] ?? "")) {
      j++;
    }
    if (j >= lines.length) {
      // Unterminated block — bail without corrupting the rest.
      break;
    }
    // ours = lines between start and ===, theirs = lines between === and >>>.
    const oursLines = lines.slice(i + 1, j);
    let k = j + 1;
    while (k < lines.length && !END_RE.test(lines[k] ?? "")) {
      k++;
    }
    if (k >= lines.length) {
      break;
    }
    const theirsLines = lines.slice(j + 1, k);
    out.push({
      startLine,
      endLine: k + 1,
      ours: oursLines.join("\n"),
      theirs: theirsLines.join("\n"),
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
    out.push(replacements[i] ?? "");
    cursor = b.endLine + 1;
  }
  if (cursor - 1 < lines.length) {
    out = out.concat(lines.slice(cursor - 1));
  }
  return out.join("\n");
}
