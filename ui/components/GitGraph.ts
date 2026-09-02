import type { Commit } from "../lib/ipc";
import { formatDate } from "../lib/format";

const LANE_COLORS = [
  "var(--lane-1)",
  "var(--lane-2)",
  "var(--lane-3)",
  "var(--lane-4)",
  "var(--lane-5)",
  "var(--lane-6)",
];

interface LaneInfo {
  index: number;
  color: string;
  active: boolean;
}

/**
 * Assigns each commit to a column ("lane") by walking parents and reusing lanes
 * freed by ancestors. Mirrors the project-kanban GitGraph.tsx algorithm.
 *
 * Returns one row per commit with the resolved lane assignments for all visible
 * columns at that row.
 */
export function assignLanes(commits: Commit[]): Array<{ commit: Commit; lane: number; rows: LaneInfo[][] }> {
  const laneCommit = new Map<number, string | null>(); // lane -> last-seen sha
  const rows: Array<{ commit: Commit; lane: number; rows: LaneInfo[][] }> = [];

  for (const c of commits) {
    let lane = -1;
    // Reuse an existing lane whose head matches this commit's first parent.
    for (const [i, sha] of laneCommit.entries()) {
      if (sha === c.parents[0]) {
        lane = i;
        break;
      }
    }
    if (lane < 0) {
      // Find first empty lane
      for (let i = 0; i < 1000; i++) {
        if (!laneCommit.has(i)) {
          lane = i;
          break;
        }
      }
    }
    laneCommit.set(lane, c.sha);
    // Other parents become new lanes — split off and merge back.
    for (let p = 1; p < c.parents.length; p++) {
      let newLane = -1;
      for (let i = 0; i < 1000; i++) {
        if (!laneCommit.has(i)) {
          newLane = i;
          break;
        }
      }
      laneCommit.set(newLane, c.parents[p]);
    }

    // Build ancestor set for the current commit (transitive closure of parents)
    const ancestors = new Set<string>();
    const stack = [...c.parents];
    while (stack.length > 0) {
      const p = stack.pop()!;
      if (!ancestors.has(p)) {
        ancestors.add(p);
        const parentCommit = commits.find((x) => x.sha === p);
        if (parentCommit) stack.push(...parentCommit.parents);
      }
    }
    // Build a row snapshot (one array of all lane infos for this commit)
    const lanes: LaneInfo[] = [];
    const cols = Math.max(...laneCommit.keys(), 0) + 1;
    for (let col = 0; col < cols; col++) {
      const laneHead = laneCommit.get(col);
      const isActive = laneHead === c.sha || ancestors.has(laneHead ?? "");
      lanes.push({ index: col, color: LANE_COLORS[col % LANE_COLORS.length], active: isActive });
    }
    rows.push({ commit: c, lane, rows: [lanes] });
  }
  return rows;
}

export function renderGitGraph(commits: Commit[]): HTMLElement {
  const wrap = document.createElement("div");
  wrap.className = "flex flex-col gap-3";
  const rows = assignLanes(commits);
  if (rows.length === 0) {
    const empty = document.createElement("div");
    empty.className = "text-display-sm text-[color:var(--color-ink-muted)] text-center py-6";
    empty.textContent = "아직 커밋이 없습니다.";
    wrap.appendChild(empty);
    return wrap;
  }
  for (const r of rows) {
    const row = document.createElement("div");
    row.className = "flex items-center gap-2";
    // Lane markers
    const cols = Math.max(...rows.map((x) => x.lane)) + 1;
    for (let col = 0; col < cols; col++) {
      const laneInfo = r.rows[0]?.[col];
      const color = laneInfo?.color ?? "var(--color-hairline)";
      const isActive = laneInfo?.active ?? false;
      const dot = document.createElement("div");
      dot.style.width = "8px";
      dot.style.height = "8px";
      dot.style.borderRadius = "50%";
      dot.style.background = isActive ? color : "var(--color-hairline)";
      dot.style.flexShrink = "0";
      if (col > 0) {
        const line = document.createElement("div");
        line.style.flex = "1";
        line.style.height = "2px";
        line.style.background = isActive ? color : "var(--color-hairline)";
        row.appendChild(line);
      }
      row.appendChild(dot);
    }
    // Commit message
    const msg = document.createElement("span");
    msg.className = "text-display-md truncate flex-1";
    msg.textContent = r.commit.message;
    row.appendChild(msg);
    // SHA
    const sha = document.createElement("span");
    sha.className = "font-mono text-display-sm text-[color:var(--color-ink-muted)] whitespace-nowrap";
    sha.textContent = r.commit.sha.slice(0, 7);
    row.appendChild(sha);
    // Date
    const date = document.createElement("span");
    date.className = "text-display-sm text-[color:var(--color-ink-muted)] whitespace-nowrap";
    date.textContent = formatDate(r.commit.date);
    row.appendChild(date);
    wrap.appendChild(row);
  }
  return wrap;
}
