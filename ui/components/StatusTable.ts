// Pure presentation helper: maps FileChange to Korean labels.
import type { FileChange } from "../lib/ipc";

export type { FileChange };

const KIND_LABELS: Record<string, string> = {
  added: "추가",
  modified: "수정",
  deleted: "삭제",
  renamed: "이름변경",
  copied: "복사됨",
  untracked: "미추적",
  conflicted: "충돌",
};

export function kindLabel(kind: string): string {
  return KIND_LABELS[kind] ?? kind;
}

export function renderStatusTable(
  files: FileChange[],
  onToggle: (path: string, staged: boolean) => void
): HTMLElement {
  const table = document.createElement("table");
  table.className = "w-full text-sm";
  table.innerHTML = `
    <thead>
      <tr class="text-left text-[color:var(--color-ink-muted)] border-b border-[color:var(--color-hairline)]">
        <th class="py-1 w-8"></th>
        <th class="py-1">파일</th>
        <th class="py-1 w-24 text-right">상태</th>
      </tr>
    </thead>
    <tbody id="status-files"></tbody>
  `;
  const tbody = table.querySelector("#status-files")!;
  for (const f of files) {
    const tr = document.createElement("tr");
    tr.className = "border-b border-[color:var(--color-hairline)] hover:bg-[color:var(--color-surface-strong)]";
    const checkbox = document.createElement("input");
    checkbox.type = "checkbox";
    checkbox.checked = f.staged;
    checkbox.disabled = f.kind === "untracked" ? false : !f.staged;
    checkbox.dataset.path = f.path;
    checkbox.addEventListener("change", () => {
      onToggle(f.path, checkbox.checked);
    });
    tr.innerHTML = `
      <td class="py-1"></td>
      <td class="py-1 font-mono truncate max-w-xs" title="${f.path}">${f.path}</td>
      <td class="py-1 text-right">
        <span class="inline-block px-2 py-0.5 rounded text-xs font-medium ${badgeClass(f.kind)}">${kindLabel(f.kind)}</span>
      </td>
    `;
    tr.querySelector("td:first-child")!.appendChild(checkbox);
    tbody.appendChild(tr);
  }
  return table;
}

function badgeClass(kind: string): string {
  switch (kind) {
    case "added": return "gc-badge gc-badge--success";
    case "modified": return "gc-badge gc-badge--warning";
    case "deleted": return "gc-badge gc-badge--danger";
    case "untracked": return "gc-badge gc-badge--neutral";
    case "conflicted": return "gc-badge gc-badge--danger";
    default: return "gc-badge gc-badge--muted";
  }
}
