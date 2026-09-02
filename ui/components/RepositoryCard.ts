import type { Repo } from "../lib/ipc";
import { ipc } from "../lib/ipc";
import { toast } from "./Toast";
import { icon, spinner } from "./Icon";
import { setBusy } from "./Busy";
export function renderRepoCard(
  repo: Repo,
  onRemove: () => void,
  onOpen: () => void,
): HTMLElement {
  const card = document.createElement("div");
  card.className = "gc-card flex flex-col gap-3";

  // ── Header: title + open/remove ─────────────────────────────────────────
  const head = document.createElement("div");
  head.className = "flex items-start justify-between gap-2";
  const headText = document.createElement("div");
  headText.className = "min-w-0";
  const title = document.createElement("div");
  title.className = "text-display-lg font-medium truncate";
  title.textContent = repo.display_name;
  headText.appendChild(title);
  const path = document.createElement("div");
  path.className = "text-display-sm text-[color:var(--color-ink-muted)] truncate max-w-md";
  path.textContent = repo.path;
  headText.appendChild(path);
  head.appendChild(headText);
  const headBtns = document.createElement("div");
  headBtns.className = "flex gap-2 shrink-0";
  const openBtn = document.createElement("button");
  openBtn.className = "gc-button-primary";
  openBtn.textContent = "열기";
  openBtn.addEventListener("click", onOpen);
  headBtns.appendChild(openBtn);
  const removeBtn = document.createElement("button");
  removeBtn.className = "gc-button-secondary gc-icon-btn gc-icon-btn--danger";
  removeBtn.title = "삭제";
  removeBtn.setAttribute("aria-label", "삭제");
  removeBtn.appendChild(icon("trash", 16));
  removeBtn.addEventListener("click", async () => {
    if (removeBtn.disabled) return;
    if (!confirm(`${repo.display_name} 저장소를 삭제하시겠습니까?`)) return;
    setBusy(removeBtn, true, "삭제 중…");
    try {
      await ipc.removeRepository(repo.id);
      toast("저장소가 삭제되었습니다", "success");
      onRemove();
    } catch (e) {
      toast(`삭제 실패: ${(e as Error).message ?? e}`, "error");
    } finally {
      setBusy(removeBtn, false);
    }
  });
  headBtns.appendChild(removeBtn);
  head.appendChild(headBtns);
  card.appendChild(head);

  // ── Branch row ─────────────────────────────────────────────────────────
  const branchRow = document.createElement("div");
  branchRow.className = "flex items-center gap-2 text-display-sm text-[color:var(--color-ink-muted)]";
  branchRow.appendChild(icon("branch", 14));
  const branchName = document.createElement("span");
  branchName.className = "font-mono";
  branchName.textContent = repo.working_branch || "-";
  branchRow.appendChild(branchName);
  if (repo.working_branch && repo.working_branch !== repo.default_branch) {
    const warn = document.createElement("span");
    warn.className = "gc-badge gc-badge--warning";
    warn.textContent = "기본과 다름";
    branchRow.appendChild(warn);
  }
  card.appendChild(branchRow);

  // ── Status row (badge) ────────────────────────────────────────────────
  const statusRow = document.createElement("div");
  statusRow.id = "status-row";
  statusRow.className = "flex flex-wrap items-center gap-2";
  const placeholder = document.createElement("span");
  placeholder.className = "gc-badge gc-badge--muted";
  placeholder.textContent = "상태 로딩 중…";
  placeholder.prepend(spinner(12));
  statusRow.appendChild(placeholder);
  card.appendChild(statusRow);

  // ── Action bar (5 labeled actions) ────────────────────────────────────
  const toolbar = document.createElement("div");
  toolbar.className = "gc-action-bar";
  function toolBtn(name: "plus" | "commit" | "push" | "pull" | "log", label: string, cb: () => void | Promise<void>): HTMLButtonElement {
    const b = document.createElement("button");
    b.className = "gc-action-cell";
    b.title = label;
    b.setAttribute("aria-label", label);
    b.appendChild(icon(name, 16));
    const lbl = document.createElement("span");
    lbl.className = "gc-action-cell__label";
    lbl.textContent = label;
    b.appendChild(lbl);
    b.addEventListener("click", () => {
      if (b.disabled) return;
      setBusy(b, true, label);
      void Promise.resolve(cb()).finally(() => setBusy(b, false));
    });
    return b;
  }
  toolbar.appendChild(toolBtn("plus", "모두 스테이지", async () => {
    try {
      await ipc.addFiles(repo.id, ["."]);
      toast("스테이지 완료", "success");
    } catch (e) {
      toast(`스테이지 실패: ${(e as Error).message ?? e}`, "error");
    }
  }));
  toolbar.appendChild(toolBtn("commit", "커밋", async () => {
    const msg = prompt("커밋 메시지:");
    if (!msg) return;
    try {
      await ipc.commit(repo.id, msg, true);
      toast("커밋 완료", "success");
    } catch (e) {
      toast(`커밋 실패: ${(e as Error).message ?? e}`, "error");
    }
  }));
  toolbar.appendChild(toolBtn("push", "푸시", async () => {
    try {
      const res = await ipc.push(repo.id);
      if (res.ok) {
        toast("푸시 완료", "success");
      } else if (res.auth_required) {
        toast(res.message, "error");
      } else {
        toast(`푸시 실패: ${res.message}`, "error");
      }
    } catch (e) {
      toast(`푸시 실패: ${(e as Error).message ?? e}`, "error");
    }
  }));
  toolbar.appendChild(toolBtn("pull", "풀", async () => {
    try {
      const res = await ipc.pull(repo.id);
      toast(res.ok ? "풀 완료" : `풀 실패: ${res.message}`, res.ok ? "success" : "error");
    } catch (e) {
      toast(`풀 실패: ${(e as Error).message ?? e}`, "error");
    }
  }));
  toolbar.appendChild(toolBtn("log", "로그 보기", async () => {
    const branch = repo.working_branch || repo.default_branch || "main";
    try {
      const commits = await ipc.listCommits(repo.id, branch, 20);
      const lines = commits.map((c) => `${c.sha.slice(0, 7)} ${c.message}`).join("\n");
      alert(lines || "커밋이 없습니다.");
    } catch (e) {
      toast(`로그 실패: ${(e as Error).message ?? e}`, "error");
    }
  }));
  card.appendChild(toolbar);

  // ── Status badge ──────────────────────────────────────────────────────
  ipc.status(repo.id).then((s) => {
    const el = card.querySelector<HTMLDivElement>("#status-row")!;
    el.innerHTML = "";
    const mod = s.files.filter((f) =>
      f.kind === "modified" || f.kind === "added" || f.kind === "deleted" || f.kind === "renamed" || f.kind === "copied",
    ).length;
    const untracked = s.files.filter((f) => f.kind === "untracked").length;
    const conflicted = s.files.filter((f) => f.kind === "conflicted").length;
    if (conflicted > 0) {
      const b = document.createElement("span");
      b.className = "gc-badge gc-badge--danger inline-flex items-center gap-1";
      b.appendChild(icon("warn", 12));
      b.appendChild(document.createTextNode(`충돌 ${conflicted}개`));
      el.appendChild(b);
    }
    const dirty = mod + untracked;
    if (dirty > 0) {
      const b = document.createElement("span");
      b.className = "gc-badge gc-badge--muted";
      b.textContent = `변경 ${dirty}개`;
      el.appendChild(b);
    }
    if (conflicted === 0 && dirty === 0) {
      const b = document.createElement("span");
      b.className = "gc-badge gc-badge--neutral";
      b.textContent = "깨끗함";
      el.appendChild(b);
    }
  }).catch(() => {
    const el = card.querySelector<HTMLDivElement>("#status-row")!;
    el.innerHTML = "";
    const b = document.createElement("span");
    b.className = "gc-badge gc-badge--muted";
    b.textContent = "상태를 불러올 수 없음";
    el.appendChild(b);
  });

  return card;
}
