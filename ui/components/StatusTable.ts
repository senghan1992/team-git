// Pure presentation helper: maps FileChange to Korean labels.
import type { FileChange } from "../lib/ipc";

export type { FileChange };

// git 용어를 그대로 옮기지 않는다. "미추적(untracked)" 은 git 을 아는 사람에게만
// 통하는 말이고, 화면에서 실제로 뜻하는 것은 "아직 git 이 모르는 새 파일" 이다.
const KIND_LABELS: Record<string, string> = {
  added: "추가",
  modified: "수정",
  deleted: "삭제",
  renamed: "이름변경",
  copied: "복사됨",
  untracked: "새 파일",
  conflicted: "충돌",
};

/** 상태별 한 줄 설명 — 배지에 마우스를 올렸을 때 뜻을 알 수 있게. */
const KIND_HINTS: Record<string, string> = {
  added: "커밋에 포함되도록 표시된 파일입니다.",
  modified: "내용이 바뀐 파일입니다.",
  deleted: "지워진 파일입니다. 커밋하면 저장소에서도 사라집니다.",
  renamed: "이름이 바뀐 파일입니다.",
  copied: "다른 파일에서 복사된 파일입니다.",
  untracked: "git 이 아직 모르는 새 파일입니다. 커밋하면 저장소에 들어갑니다.",
  conflicted: "병합 충돌이 남아 있습니다. 병합 탭에서 해결해야 합니다.",
};

export function kindHint(kind: string): string {
  return KIND_HINTS[kind] ?? "";
}

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
        <span class="inline-block px-2 py-0.5 rounded text-xs font-medium ${badgeClass(f.kind)}" title="${kindHint(f.kind)}">${kindLabel(f.kind)}</span>
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
