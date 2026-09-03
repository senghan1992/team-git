import type { ProjectConfigResult, Repo, WorkingTreeStatus } from "../lib/ipc";
import { ipc } from "../lib/ipc";
import { toast } from "./Toast";
import { icon, spinner } from "./Icon";
import { setBusy } from "./Busy";
import { confirmDialog } from "./Modal";
import { getSession } from "../lib/session";
import type { RepoTab } from "./Sidebar";
import { computeNextAction, isMergeManagerFor, type NextAction } from "./nextAction";

/**
 * 저장소 카드 — 상태를 보여 주고 **다음 할 일 하나**를 제안한다.
 *
 * 예전에는 스테이지/커밋/푸시/풀/로그 다섯 버튼을 늘어놓고 커밋 메시지를
 * `prompt()`로 받았다. 작업 탭이 같은 일을 훨씬 안전하게 하고 있었고, 이
 * 버튼들은 병합 관리자 정책(관리자만 병합 브랜치에 푸시)을 우회했다.
 * 그래서 카드는 "무엇을 해야 하는지"만 말하고, 실제 작업은 탭으로 보낸다.
 */
export function renderRepoCard(
  repo: Repo,
  onRemove: () => void,
  onOpen: (tab?: RepoTab) => void,
): HTMLElement {
  const card = document.createElement("div");
  card.className = "gc-card flex flex-col gap-3";
  const baseBranch = repo.default_branch || "main";

  // ── Header: title + remove ──────────────────────────────────────────────
  const head = document.createElement("div");
  head.className = "flex items-start justify-between gap-2";
  const headText = document.createElement("div");
  headText.className = "min-w-0";
  const title = document.createElement("button");
  title.className = "text-display-lg font-medium truncate text-left hover:underline";
  title.textContent = repo.display_name;
  title.addEventListener("click", () => onOpen("work"));
  headText.appendChild(title);
  const path = document.createElement("div");
  path.className = "text-display-sm text-[color:var(--color-ink-muted)] truncate max-w-md";
  path.textContent = repo.path;
  headText.appendChild(path);
  head.appendChild(headText);
  const removeBtn = document.createElement("button");
  removeBtn.className = "gc-button-secondary gc-icon-btn gc-icon-btn--danger shrink-0";
  removeBtn.title = "저장소 삭제";
  removeBtn.setAttribute("aria-label", "저장소 삭제");
  removeBtn.appendChild(icon("trash", 16));
  removeBtn.addEventListener("click", async () => {
    if (removeBtn.disabled) return;
    const ok = await confirmDialog({
      title: "저장소 삭제",
      message: `${repo.display_name}을(를) 목록에서 제거합니다.\n디스크의 실제 저장소와 커밋은 지워지지 않습니다.`,
      confirmLabel: "제거",
    });
    if (!ok) return;
    setBusy(removeBtn, true, "삭제 중…");
    try {
      await ipc.removeRepository(repo.id);
      toast("저장소를 목록에서 제거했습니다", "success");
      onRemove();
    } catch (e) {
      toast(`삭제 실패: ${(e as Error).message ?? e}`, "error");
    } finally {
      setBusy(removeBtn, false);
    }
  });
  head.appendChild(removeBtn);
  card.appendChild(head);

  // ── Branch + status pills ───────────────────────────────────────────────
  const infoRow = document.createElement("div");
  infoRow.className = "flex flex-wrap items-center gap-2 text-display-sm";
  const branchChip = document.createElement("span");
  branchChip.className =
    "inline-flex items-center gap-1 text-[color:var(--color-ink-muted)]";
  branchChip.appendChild(icon("branch", 14));
  const branchName = document.createElement("span");
  branchName.className = "font-mono";
  branchName.textContent = repo.working_branch || baseBranch;
  branchChip.appendChild(branchName);
  infoRow.appendChild(branchChip);
  const pills = document.createElement("span");
  pills.className = "inline-flex flex-wrap items-center gap-1.5";
  const loadingPill = document.createElement("span");
  loadingPill.className = "gc-badge gc-badge--muted inline-flex items-center gap-1";
  loadingPill.appendChild(spinner(12));
  loadingPill.appendChild(document.createTextNode("확인 중…"));
  pills.appendChild(loadingPill);
  infoRow.appendChild(pills);
  card.appendChild(infoRow);

  // ── 다음 할 일 ──────────────────────────────────────────────────────────
  const todo = document.createElement("div");
  todo.className = "gc-todo";
  card.appendChild(todo);

  function paintPills(status: WorkingTreeStatus | null, pending: number | null) {
    pills.innerHTML = "";
    const add = (text: string, cls: string) => {
      const b = document.createElement("span");
      b.className = `gc-badge ${cls}`;
      b.textContent = text;
      pills.appendChild(b);
    };
    if (!status) {
      add("상태를 불러올 수 없음", "gc-badge--muted");
      return;
    }
    const conflicted = status.files.filter((f) => f.kind === "conflicted").length;
    const dirty = status.files.filter((f) => f.kind !== "conflicted").length;
    if (conflicted > 0) add(`충돌 ${conflicted}`, "gc-badge--danger");
    if (dirty > 0) add(`변경 ${dirty}`, "gc-badge--muted");
    if (status.ahead > 0) add(`↑${status.ahead} 미푸시`, "gc-badge--warning");
    if (status.behind > 0) add(`↓${status.behind} 뒤처짐`, "gc-badge--info");
    if (pending !== null && pending > 0) add(`병합 대기 ${pending}`, "gc-badge--warning");
    if (pills.children.length === 0) add("깨끗함", "gc-badge--neutral");
  }

  function paintTodo(action: NextAction, status: WorkingTreeStatus | null) {
    todo.innerHTML = "";
    todo.classList.toggle("gc-todo--urgent", action.urgent);
    todo.classList.toggle("gc-todo--calm", action.kind === "clean");

    const text = document.createElement("div");
    text.className = "flex-1 min-w-0 flex flex-col gap-0.5";
    const label = document.createElement("div");
    label.className = "text-display-sm font-medium";
    label.textContent = action.kind === "clean" ? "다음 할 일 없음" : "다음 할 일";
    text.appendChild(label);
    const reason = document.createElement("div");
    reason.className = "text-display-xs text-[color:var(--color-ink-muted)]";
    reason.textContent = action.reason;
    text.appendChild(reason);
    todo.appendChild(text);

    const btn = document.createElement("button");
    btn.className =
      (action.urgent ? "gc-button-primary" : "gc-button-secondary") + " shrink-0";
    btn.textContent = action.label;
    if (action.kind === "sync") {
      // 동기화는 카드에서 바로 끝낼 수 있는 유일한 행동 — 되돌릴 수 있고,
      // 실패하면 병합 탭으로 안내한다.
      btn.addEventListener("click", async () => {
        setBusy(btn, true, "동기화 중…");
        try {
          const r = await ipc.syncBranch(repo.id, baseBranch);
          if (r.conflicted) {
            toast(`충돌 ${r.files.length}개 발생 — 병합 탭에서 해결하세요.`, "info");
            onOpen("merge");
          } else {
            toast("동기화 완료 — 최신 변경을 내 브랜치에 반영했습니다.", "success");
            void load();
          }
        } catch (e) {
          const msg = (e as Error).message ?? String(e);
          toast(`동기화 실패: ${msg}`, "error");
          // 커밋/스태시 또는 진행 중 병합 마무리가 필요한 실패는 그 일을
          // 할 수 있는 탭으로 바로 데려다 준다.
          if (msg.includes("커밋하지 않은 변경")) onOpen("work");
          else if (msg.includes("병합이 있습니다")) onOpen("merge");
        } finally {
          setBusy(btn, false);
        }
      });
    } else {
      btn.addEventListener("click", () => onOpen(action.tab ?? "work"));
    }
    todo.appendChild(btn);

    // 상태를 못 읽었을 때는 이유를 숨기지 않는다.
    if (!status) {
      reason.textContent =
        "상태를 확인할 수 없습니다. 경로나 SSH 연결을 확인하세요.";
    }
  }

  // ── 로드: 상태 + 팀 규칙(동시에) → 화면 먼저 → (관리자일 때만) 병합 대기 수 ──
  //
  // 병합 대기 조회는 원격 ref를 브랜치마다 훑기 때문에 SSH 저장소에서는
  // 수 초가 걸린다. 예전에는 상태·설정·병합 대기를 차례로 기다린 뒤에야
  // 첫 그림을 그려서, 느린 저장소 하나가 "확인 중…"을 10초 넘게 붙잡았다.
  // 지금은 상태와 설정을 동시에 묻고 도착하는 즉시 그리고, 병합 대기 수는
  // 그 뒤에 따로 채운다 (실패하면 조용히 넘긴다 — 카드가 죽지 않게).
  async function load() {
    const [status, cfg] = await Promise.all([
      ipc.status(repo.id).catch((): WorkingTreeStatus | null => null),
      ipc.projectConfigGet(repo.id).catch((): ProjectConfigResult | null => null),
    ]);
    const me = getSession();
    const isManager = isMergeManagerFor(cfg, me?.email ?? null, baseBranch);
    const wantPending = isManager && !!status;

    paintPills(status, null);
    if (wantPending) {
      const checking = document.createElement("span");
      checking.className = "gc-badge gc-badge--muted inline-flex items-center gap-1";
      checking.appendChild(spinner(12));
      checking.appendChild(document.createTextNode("병합 대기 확인 중…"));
      pills.appendChild(checking);
    }
    paintTodo(
      computeNextAction({ status, pendingCount: null, isMergeManager: isManager, baseBranch }),
      status,
    );
    if (!wantPending) return;

    let pending: number | null = null;
    try {
      pending = (await ipc.listPendingBranches(repo.id, baseBranch)).length;
    } catch {
      pending = null;
    }
    if (!card.isConnected) return;
    paintPills(status, pending);
    paintTodo(
      computeNextAction({ status, pendingCount: pending, isMergeManager: isManager, baseBranch }),
      status,
    );
  }

  void load();

  // 홈 카드가 한 번 그려진 채 얼어 있으면 "다음 할 일"이 거짓말이 된다 —
  // 팀원의 push, 다른 탭에서 한 커밋이 30초 안에 카드에 반영되도록 가볍게
  // 다시 읽는다. 카드가 화면에서 떨어지면 스스로 정리한다.
  let loading = false;
  const refreshTimer = window.setInterval(async () => {
    if (!card.isConnected) {
      window.clearInterval(refreshTimer);
      return;
    }
    if (loading || document.querySelector("dialog[open]")) return;
    loading = true;
    try {
      await load();
    } finally {
      loading = false;
    }
  }, 30_000);

  return card;
}
