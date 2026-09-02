/**
 * Unit tests for the GitGraph lane-assignment algorithm.
 * Runs against the real shipped ui/components/GitGraph.ts via Bun.
 */
import { assignLanes } from "./GitGraph";
import type { Commit } from "../lib/ipc";

const SHA_INIT  = "a".repeat(40);
const SHA_FEAT  = "b".repeat(40);
const SHA_MERGE = "c".repeat(40);

function commit(sha: string, message: string, parents: string[]): Commit {
  return { sha, message, author: "Alice", date: "2024-01-01T10:00:00+00:00", parents };
}

function assert(cond: boolean, msg: string) {
  if (!cond) throw new Error(`ASSERTION FAILED: ${msg}`);
}

// ── Test: single linear commit ───────────────────────────────────────────────
{
  const commits = [commit(SHA_INIT, "initial", [])];
  const rows = assignLanes(commits);
  const [{ lane, rows: rowLanes }] = rows;
  const active = rowLanes[0].filter(l => l.active).map(l => l.index);
  assert(lane === 0, `linear: lane=${lane}, expected 0`);
  assert(active.length === 1, `linear: active lanes=[${active}], expected [0]`);
  console.log("PASS: linear commit → lane 0");
}

// ── Test: two commits in a row (no merge) ────────────────────────────────────
{
  const commits = [
    commit(SHA_INIT, "initial", []),
    commit(SHA_FEAT, "feat: update", [SHA_INIT]),
  ];
  const rows = assignLanes(commits);
  const [init, feat] = rows;
  assert(init.lane === 0, `linear2 init: lane=${init.lane}, expected 0`);
  assert(feat.lane === 0, `linear2 feat: lane=${feat.lane}, expected 0 (reuses parent lane)`);
  const featActive = feat.rows[0].filter(l => l.active).map(l => l.index);
  assert(featActive.length === 1, `linear2 feat active lanes=[${featActive}], expected [0]`);
  console.log("PASS: two commits in a row → both lane 0");
}

// ── Test: branch split ───────────────────────────────────────────────────────
// main: initial
// feat: "feat: update" (parent = initial)
{
  const commits = [
    commit(SHA_INIT, "initial", []),
    commit(SHA_FEAT, "feat: update", [SHA_INIT]),
  ];
  const rows = assignLanes(commits);
  const [, feat] = rows;
  assert(feat.lane === 0, `branch-split: feat lane=${feat.lane}, expected 0`);
  console.log("PASS: branch split → feat reuses parent lane");
}

// ── Test: E2E Step 6 — merge commit (diamond shape) ─────────────────────────
// main: initial
// feat: "feat: update" (parent = initial)
// main: merge feat (parents = [feat, initial])
//
// Algorithm trace:
//   init:  lane=0 (alloc fresh), laneCommit={0:SHA_INIT}
//   feat:  first parent SHA_INIT in lane 0 → lane=0, laneCommit.set(0,SHA_FEAT)
//   merge: first parent SHA_FEAT in lane 0 → lane=0, laneCommit.set(0,SHA_MERGE)
//          p=1: SHA_INIT → not found (was overwritten) → alloc lane 1, laneCommit.set(1,SHA_INIT)
//   Result: laneCommit={0:SHA_MERGE, 1:SHA_INIT}, both active
{
  const commits = [
    commit(SHA_INIT,  "initial",      []),
    commit(SHA_FEAT,  "feat: update", [SHA_INIT]),
    commit(SHA_MERGE, "merge feat",   [SHA_FEAT, SHA_INIT]),
  ];
  const rows = assignLanes(commits);
  const [init, feat, merge] = rows;

  assert(init.lane === 0,  `merge init: lane=${init.lane}, expected 0`);
  assert(feat.lane === 0,  `merge feat: lane=${feat.lane}, expected 0 (reuses first-parent lane)`);
  assert(merge.lane === 0, `merge merge: lane=${merge.lane}, expected 0 (first parent SHA_FEAT was in lane 0)`);

  const mergeActive = merge.rows[0].filter(l => l.active).map(l => l.index);
  assert(mergeActive.length >= 2, `merge active lanes=[${mergeActive}], expected ≥2`);

  assert(merge.commit.parents.length > 1, "merge should have 2 parents for diamond");

  console.log("PASS: merge commit → lane 0, 2 active parent lanes (diamond visible)");
}

// ── Test: multiple branches ─────────────────────────────────────────────────
// A---M1---M2        lane 0
//   \        \      lane 1
//    B1------B2    lane 2
{
  const SHA_A   = "1" + "0".repeat(39);
  const SHA_M1  = "2" + "0".repeat(39);
  const SHA_B1  = "3" + "0".repeat(39);
  const SHA_M2  = "4" + "0".repeat(39);
  const SHA_B2  = "5" + "0".repeat(39);

  const commits = [
    commit(SHA_A,  "A",   []),
    commit(SHA_M1, "M1",  [SHA_A]),
    commit(SHA_B1, "B1",  [SHA_A]),
    commit(SHA_M2, "M2",  [SHA_M1]),
    commit(SHA_B2, "B2",  [SHA_M2, SHA_B1]),
  ];

  const rows = assignLanes(commits);
  const [, m1, b1, m2, b2] = rows;

  assert(m1.lane === 0, `M1 lane=${m1.lane}, expected 0`);
  assert(b1.lane === 1, `B1 lane=${b1.lane}, expected 1 (split from A)`);
  assert(m2.lane === 0, `M2 lane=${m2.lane}, expected 0 (from M1)`);
  // B2: parents=[M2,B1]; first parent M2 in lane 0 → lane=0; p=1 finds B1 at lane 1
  assert(b2.lane === 0, `B2 lane=${b2.lane}, expected 0 (first parent M2 in lane 0)`);

  const b2Active = b2.rows[0].filter(l => l.active).map(l => l.index);
  assert(b2Active.length >= 2, `B2 active lanes=[${b2Active}], expected ≥2`);

  console.log("PASS: multi-branch → lane assignments correct");
}

console.log("\n✓ All GitGraph lane-assignment assertions passed");
