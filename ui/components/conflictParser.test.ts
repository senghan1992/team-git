// Bun test runner — same invocation as GitGraph.test.ts.
import { parseConflictBlocks, reassemble } from "./conflictParser";

function assert(cond: boolean, msg: string) {
  if (!cond) throw new Error(`ASSERTION FAILED: ${msg}`);
}

// ── parseConflictBlocks ───────────────────────────────────────────────────────
{
  const input = [
    "line0",
    "<<<<<<< HEAD",
    "ours line",
    "=======",
    "theirs line",
    ">>>>>>> feature/x",
    "lineZ",
  ].join("\n");
  const blocks = parseConflictBlocks(input);
  assert(blocks.length === 1, `single block: got ${blocks.length}`);
  assert(blocks[0]!.ours === "ours line", `ours body: ${blocks[0]!.ours}`);
  assert(blocks[0]!.theirs === "theirs line", `theirs body: ${blocks[0]!.theirs}`);
  assert(blocks[0]!.startLine === 2, `startLine: ${blocks[0]!.startLine}`);
  assert(blocks[0]!.endLine === 6, `endLine: ${blocks[0]!.endLine}`);
  console.log("PASS: single block parsed");
}

{
  const input = ["<<<<<<< HEAD", "=======", ">>>>>>> feature/x"].join("\n");
  const blocks = parseConflictBlocks(input);
  assert(blocks.length === 1, "empty ours/theirs: count");
  assert(blocks[0]!.ours === "", "ours empty");
  assert(blocks[0]!.theirs === "", "theirs empty");
  console.log("PASS: empty ours/theirs handled");
}

{
  const input = [
    "<<<<<<< HEAD",
    "ours1",
    "=======",
    "theirs1",
    ">>>>>>> feature/x",
    "mid",
    "<<<<<<< HEAD",
    "ours2",
    "=======",
    "theirs2",
    ">>>>>>> feature/x",
  ].join("\n");
  const blocks = parseConflictBlocks(input);
  assert(blocks.length === 2, `multiple blocks: got ${blocks.length}`);
  assert(blocks[0]!.ours === "ours1", "first ours");
  assert(blocks[1]!.theirs === "theirs2", "second theirs");
  console.log("PASS: multiple blocks parsed");
}

{
  const input = [
    "    <<<<<<< not a marker",
    "    =======",
    "    >>>>>>> not a marker",
    "<<<<<<< HEAD",
    "ours",
    "=======",
    "theirs",
    ">>>>>>> feature/x",
  ].join("\n");
  const blocks = parseConflictBlocks(input);
  assert(blocks.length === 1, `indented lookalike ignored: got ${blocks.length}`);
  assert(blocks[0]!.ours === "ours", "real ours picked");
  console.log("PASS: indented lookalike ignored");
}

{
  const input = "<<<<<<<no-space\nnot a block\n";
  assert(parseConflictBlocks(input).length === 0, "missing space after <<< not parsed");
  console.log("PASS: marker without trailing space ignored");
}

// ── reassemble ───────────────────────────────────────────────────────────────
{
  const input = [
    "head",
    "<<<<<<< HEAD",
    "ours",
    "=======",
    "theirs",
    ">>>>>>> feature/x",
    "tail",
  ].join("\n");
  const blocks = parseConflictBlocks(input);
  const out = reassemble(input, blocks, ["merged"]);
  assert(out === "head\nmerged\ntail", `reassemble: ${out}`);
  console.log("PASS: reassemble replaces one block");
}

{
  const input = [
    "<<<<<<< HEAD",
    "a",
    "=======",
    "A",
    ">>>>>>> x",
    "middle",
    "<<<<<<< HEAD",
    "b",
    "=======",
    "B",
    ">>>>>>> x",
  ].join("\n");
  const blocks = parseConflictBlocks(input);
  const out = reassemble(input, blocks, ["a+A", "b+B"]);
  assert(out === "a+A\nmiddle\nb+B", `multi reassemble: ${out}`);
  console.log("PASS: reassemble handles multiple blocks");
}
