import type { Commit } from "../lib/ipc";
import { formatDate } from "../lib/format";

export function renderCommitList(commits: Commit[]): HTMLElement {
  const ul = document.createElement("ul");
  ul.className = "divide-y divide-[color:var(--color-hairline)]";
  if (commits.length === 0) {
    const li = document.createElement("li");
    li.className = "py-6 text-center text-display-sm text-[color:var(--color-ink-muted)]";
    li.textContent = "커밋이 없습니다.";
    ul.appendChild(li);
    return ul;
  }
  for (const c of commits) {
    const li = document.createElement("li");
    li.className = "py-3 flex items-baseline gap-3";
    li.innerHTML = `
      <span class="gc-hash-chip">${c.sha.slice(0, 7)}</span>
      <span class="flex-1 truncate text-display-md">${escape(c.message)}</span>
      <span class="text-display-sm text-[color:var(--color-ink-muted)] whitespace-nowrap">${escape(c.author)} · ${formatDate(c.date)}</span>
    `;
    ul.appendChild(li);
  }
  return ul;
}

function escape(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}
