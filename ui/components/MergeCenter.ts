// Merge Center — UI for listing pending remote branches, starting merges,
// and resolving conflicts one block at a time.
import {
  ipc,
  type AutoResolveReport,
  type BackupEntry,
  type ConflictDetail,
  type MergeOutcome,
  type MergeState,
  type PendingBranch,
  type ProjectConfigResult,
  type PushOutcome,
  type Repo,
  type Resolution,
} from "../lib/ipc";
import { confirmDialog, openModal } from "./Modal";
import { toast } from "./Toast";
import { icon } from "./Icon";
import { setBusy } from "./Busy";
import { getSession } from "../lib/session";
import { parseConflictBlocks, reassemble, type ConflictBlock } from "./conflictParser";
import { renderCommitList } from "./CommitList";
import { openPushCredentialFlow } from "./PushButton";

interface BlockEdit {
  /** Replacement body for the entire conflict block. */
  body: string;
  /** Stack of previous bodies — top is current, second-from-top is the most recent undo. */
  history: string[];
}

function pushEdit(state: BlockEdit[], idx: number, body: string) {
  const cur = state[idx];
  if (!cur) {
    state[idx] = { body, history: [] };
    return;
  }
  if (cur.body === body) return;
  cur.history.push(cur.body);
  cur.body = body;
}

function popEdit(state: BlockEdit[], idx: number): string | null {
  const cur = state[idx];
  if (!cur || cur.history.length === 0) return null;
  const prev = cur.body;
  cur.body = cur.history.pop()!;
  return prev;
}

interface ConflictFileState {
  detail: ConflictDetail;
  blocks: ConflictBlock[];
  edits: BlockEdit[];
  loading: boolean;
}

// Note: escaped rendering helpers live in ./format where needed.


async function relativeTime(unix: number): Promise<string> {
  const diff = Math.max(0, Date.now() / 1000 - unix);
  if (diff < 60) return "방금";
  if (diff < 3600) return `${Math.floor(diff / 60)}분 전`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}시간 전`;
  return `${Math.floor(diff / 86400)}일 전`;
}

export async function renderMergeCenter(repo: Repo): Promise<HTMLElement> {
  const root = document.createElement("section");
  root.className = "flex flex-col gap-4";

  let base: string = repo.default_branch || "main";
  let branches: PendingBranch[] = [];
  let mergeState: MergeState | null = null;
  // Set of every conflict path observed since the merge started; survives
  // resolution so the file list can mark resolved items with a ✓.
  let knownConflicts: Set<string> = new Set();
  // Per-file cache of parse results + in-progress block edits. Kept across
  // file switches and refreshes so unsaved edits are never lost while the
  // reviewer moves between files.
  const conflictCache = new Map<string, ConflictFileState>();
  let selectedPath: string | null = null;
  let aiEnabled = false;
  // Auto-resolve backups (safety net) for the current merge.
  let backups: BackupEntry[] = [];
  // `.gpconfig` — 병합 대상 브랜치 + 브랜치별 병합 관리자.
  let projectCfg: ProjectConfigResult | null = null;

  try {
    const cfg = await ipc.getAiConfig();
    aiEnabled = cfg.enabled;
  } catch {
    aiEnabled = false;
  }
  projectCfg = await ipc.projectConfigGet(repo.id).catch(() => null);

  /** 병합이 허용되는 대상 브랜치 — `.gpconfig` 우선, 없으면 기본 베이스만. */
  function effectiveTargets(): string[] {
    if (projectCfg?.config?.merge_targets?.length) {
      return [...new Set(projectCfg.config.merge_targets)];
    }
    return [projectCfg?.config?.default_base_branch || repo.default_branch || "main"];
  }
  /** 기본 베이스가 대상에 있으면 그것, 아니면 첫 번째 대상. */
  function initialBase(): string {
    const preferred = projectCfg?.config?.default_base_branch || repo.default_branch || "main";
    const targets = effectiveTargets();
    return targets.includes(preferred) ? preferred : targets[0];
  }
  base = initialBase();

  // ── Top row: base select + fetch ─────────────────────────────────────────
  const topRow = document.createElement("div");
  const baseSel = document.createElement("select");
  baseSel.className = "gc-input";
  baseSel.dataset.baseBranchSelect = "true";
  const targetsNow = effectiveTargets();
  for (const n of targetsNow) {
    const opt = document.createElement("option");
    opt.value = n;
    opt.textContent = n;
    if (n === base) opt.selected = true;
    baseSel.appendChild(opt);
  }
  topRow.className = "flex items-center gap-3";
  const fetchBtn = document.createElement("button");
  fetchBtn.className = "gc-button-secondary inline-flex items-center gap-1";
  fetchBtn.appendChild(icon("refresh", 14));
  const fetchLabel = document.createElement("span");
  fetchLabel.textContent = "가져오기";
  fetchBtn.appendChild(fetchLabel);
  topRow.appendChild(baseSel);
  topRow.appendChild(fetchBtn);
  // ── Auto-refresh toggle — polls fetch + branch list so teammates' pushes
  //    surface without the reviewer having to click 가져오기.
  const autoWrap = document.createElement("label");
  autoWrap.className = "inline-flex items-center gap-1.5 text-display-sm text-[color:var(--color-ink-muted)] cursor-pointer ml-auto";
  const autoCheck = document.createElement("input");
  autoCheck.type = "checkbox";
  autoCheck.checked = true;
  autoWrap.appendChild(autoCheck);
  const autoLabel = document.createElement("span");
  autoLabel.textContent = "자동 감지(20초)";
  autoWrap.appendChild(autoLabel);
  topRow.appendChild(autoWrap);
  let autoRefresh = true;
  autoCheck.addEventListener("change", () => { autoRefresh = autoCheck.checked; });
  let autoTicking = false;
  const autoTimer = window.setInterval(async () => {
    if (!root.isConnected) {
      window.clearInterval(autoTimer);
      return;
    }
    if (!autoRefresh || autoTicking) return;
    // Never disturb an in-progress edit or an open dialog.
    if (document.querySelector("dialog[open]")) return;
    const active = document.activeElement;
    if (active && (active.tagName === "TEXTAREA" || active.tagName === "INPUT")) return;
    autoTicking = true;
    try {
      await ipc.fetchRepo(repo.id);
      await refresh();
    } catch {
      // Transient SSH/network errors are fine — the next tick retries.
    } finally {
      autoTicking = false;
    }
  }, 20_000);
  root.appendChild(topRow);
  // ── In-progress merge banner (warning tint) ──────────────────────────────
  const banner = document.createElement("div");
  banner.className = "gc-banner gc-banner--warning";
  banner.style.display = "none";
  root.appendChild(banner);

  // ── Overlap warning (danger tint) ─────────────────────────────────────────
  const overlap = document.createElement("div");
  overlap.className = "gc-banner gc-banner--danger flex-col items-start";
  overlap.style.display = "none";
  root.appendChild(overlap);

  // ── Branch list ──────────────────────────────────────────────────────────
  const list = document.createElement("div");
  list.className = "flex flex-col gap-3";
  root.appendChild(list);

  // ── Conflict panel ───────────────────────────────────────────────────────
  const panel = document.createElement("div");
  panel.className = "gc-card flex flex-col gap-3";
  panel.style.display = "none";
  root.appendChild(panel);

  // ── Push banner after successful merge (success tint) ───────────────────
  const pushBanner = document.createElement("div");
  pushBanner.className = "gc-banner gc-banner--success";
  pushBanner.style.display = "none";
  root.appendChild(pushBanner);

  // ── Auto-resolve backup restore (safety net) ────────────────────────────
  const backupCard = document.createElement("div");
  backupCard.className = "gc-card flex flex-col gap-2";
  backupCard.style.display = "none";
  root.appendChild(backupCard);

  // ── Renderers ───────────────────────────────────────────────────────────
  function renderBanner() {
    if (!mergeState?.in_progress) {
      banner.style.display = "none";
      return;
    }
    banner.style.display = "";
    banner.innerHTML = "";
    const iw = document.createElement("span");
    iw.className = "gc-banner__icon";
    iw.appendChild(icon("merge", 20));
    banner.appendChild(iw);
    const text = document.createElement("span");
    text.className = "gc-banner__body flex-1";
    const n = mergeState.conflicted_files.length;
    text.textContent = n > 0
      ? `병합 진행 중 — 충돌 ${n}개`
      : "병합 진행 중 — 정리 중";
    banner.appendChild(text);
    const abortBtn = document.createElement("button");
    abortBtn.className = "gc-button-secondary";
    abortBtn.textContent = "병합 중단";
    abortBtn.addEventListener("click", async () => {
      const ok = await confirmDialog({
        title: "병합 중단",
        message: "진행 중인 병합을 중단하시겠습니까? 모든 충돌 해소 작업이 사라집니다.",
      });
      if (!ok) return;
      setBusy(abortBtn, true, "중단 중…");
      try {
        await ipc.abortMerge(repo.id);
        toast("병합을 중단했습니다.", "success");
        mergeState = null;
        conflictCache.clear();
        knownConflicts = new Set();
        await refresh();
      } catch (e) {
        toast(`중단 실패: ${(e as Error).message ?? e}`, "error");
      } finally {
        setBusy(abortBtn, false);
      }
    });
    banner.appendChild(abortBtn);
  }
  function renderOverlap() {
    if (branches.length < 2) {
      overlap.style.display = "none";
      return;
    }
    const msgs: string[] = [];
    for (let i = 0; i < branches.length; i++) {
      for (let j = i + 1; j < branches.length; j++) {
        const a = new Set(branches[i]!.changed_files.map((c) => c.path));
        const b = branches[j]!.changed_files.map((c) => c.path);
        const common = b.filter((p) => a.has(p));
        if (common.length > 0) {
          msgs.push(
            `${branches[i]!.short_name} ↔ ${branches[j]!.short_name}: 겹치는 파일 ${common.length}개(${common.join(", ")})`,
          );
        }
      }
    }
    if (msgs.length === 0) {
      overlap.style.display = "none";
      return;
    }
    overlap.style.display = "";
    overlap.innerHTML = "";
    const iw = document.createElement("span");
    iw.className = "gc-banner__icon";
    iw.appendChild(icon("warn", 20));
    overlap.appendChild(iw);
    const body = document.createElement("div");
    body.className = "gc-banner__body flex-1 flex flex-col gap-1";
    const title = document.createElement("div");
    title.className = "gc-banner__title";
    title.textContent = "수정 겹침 경고";
    body.appendChild(title);
    for (const m of msgs) {
      const div = document.createElement("div");
      div.textContent = m;
      body.appendChild(div);
    }
    overlap.appendChild(body);
  }

  function fileKindColor(kind: string): string {
    // 유약 안료 — celadon(추가)/copper(삭제)/cobalt(수정)/iron(이름변경)
    if (kind === "A") return "#276b4e";
    if (kind === "D") return "#ad392c";
    if (kind === "M") return "#2c4b8f";
    if (kind.startsWith("R")) return "#8a5a10";
    return "var(--color-ink-muted)";
  }
  async function renderBranchList() {
    list.innerHTML = "";
    if (branches.length === 0) {
      const empty = document.createElement("div");
      empty.className = "gc-empty gc-card";
      const iw = document.createElement("span");
      iw.className = "gc-empty__icon";
      iw.appendChild(icon("inbox", 32));
      empty.appendChild(iw);
      const t = document.createElement("div");
      t.className = "gc-empty__title";
      t.textContent = "대기 중인 병합이 없습니다";
      empty.appendChild(t);
      const d = document.createElement("div");
      d.className = "gc-empty__desc";
      d.textContent = "팀원이 push하면 이 화면에 나타납니다.";
      empty.appendChild(d);
      list.appendChild(empty);
      return;
    }
    for (const b of branches) {
      const card = document.createElement("div");
      card.className = "gc-card flex flex-col gap-2";
      const header = document.createElement("div");
      header.className = "flex items-center gap-3";
      const avatar = document.createElement("span");
      avatar.className = "inline-flex items-center justify-center w-9 h-9 rounded-full text-white font-medium";
      const laneIdx = (parseInt(b.sha.slice(0, 2), 16) % 6) + 1;
      avatar.style.background = `var(--lane-${laneIdx})`;
      avatar.textContent = (b.author || "?").trim().charAt(0).toUpperCase() || "?";
      header.appendChild(avatar);
      const titleWrap = document.createElement("div");
      titleWrap.className = "flex-1 min-w-0";
      const title = document.createElement("div");
      title.className = "flex items-center gap-2 min-w-0";
      const titleText = document.createElement("span");
      titleText.className = "font-medium truncate";
      titleText.textContent = b.short_name;
      title.appendChild(titleText);
      if (b.local) {
        const localTag = document.createElement("span");
        localTag.className = "gc-badge gc-badge--info shrink-0";
        localTag.textContent = "로컬";
        title.appendChild(localTag);
      }
      titleWrap.appendChild(title);
      const meta = document.createElement("div");
      meta.className = "text-display-sm text-[color:var(--color-ink-muted)] truncate";
      meta.textContent = `${b.author} · ${await relativeTime(b.unix_time)} · ${b.subject}`;
      titleWrap.appendChild(meta);
      header.appendChild(titleWrap);
      const counters = document.createElement("span");
      counters.className = "inline-flex items-center gap-1 shrink-0";
      if (b.ahead > 0) {
        const a = document.createElement("span");
        a.className = "gc-badge gc-badge--success";
        a.textContent = `↑${b.ahead}`;
        counters.appendChild(a);
      }
      if (b.behind > 0) {
        const bb = document.createElement("span");
        bb.className = "gc-badge gc-badge--muted";
        bb.textContent = `↓${b.behind}`;
        counters.appendChild(bb);
      }
      header.appendChild(counters);
      card.appendChild(header);

      const files = document.createElement("div");
      files.className = "flex flex-wrap gap-2 text-display-sm";
      for (const cf of b.changed_files) {
        const chip = document.createElement("span");
        chip.className = "gc-badge gc-badge--muted font-mono";
        chip.style.color = fileKindColor(cf.kind);
        chip.textContent = `${cf.kind} ${cf.path}`;
        files.appendChild(chip);
      }
      card.appendChild(files);

      // Collapsible commit list — gives the reviewer context on what the
      // branch contains before merging.
      const commitsRow = document.createElement("div");
      commitsRow.className = "flex flex-col gap-1";
      const commitsToggle = document.createElement("button");
      commitsToggle.className = "gc-button-secondary text-display-sm self-start";
      commitsToggle.textContent = "커밋 보기";
      const commitsHost = document.createElement("div");
      commitsHost.style.display = "none";
      commitsToggle.addEventListener("click", async () => {
        if (commitsHost.style.display !== "none") {
          commitsHost.style.display = "none";
          commitsToggle.textContent = "커밋 보기";
          return;
        }
        commitsToggle.disabled = true;
        try {
          const commits = await ipc.listCommits(repo.id, b.name, 15);
          commitsHost.innerHTML = "";
          commitsHost.appendChild(renderCommitList(commits));
          commitsHost.style.display = "";
          commitsToggle.textContent = "커밋 접기";
        } catch (e) {
          toast(`커밋 목록 조회 실패: ${(e as Error).message ?? e}`, "error");
        } finally {
          commitsToggle.disabled = false;
        }
      });
      commitsRow.appendChild(commitsToggle);
      commitsRow.appendChild(commitsHost);
      card.appendChild(commitsRow);

      const action = document.createElement("div");
      action.className = "flex flex-col gap-2 items-end";
      const btn = document.createElement("button");
      btn.className = "gc-button-primary";
      btn.textContent = `${base}(으)로 병합`;
      // 병합 대상 브랜치에 관리자가 지정되어 있으면 관리자/어드민만 병합할 수 있다.
      let blocked = false;
      let blockHint = "";
      {
        const managerEmail = projectCfg?.config?.merge_managers?.[base];
        if (managerEmail) {
          const member = projectCfg?.config?.members.find(
            (x) => x.email.toLowerCase() === managerEmail.toLowerCase(),
          );
          const name = member?.name || managerEmail;
          const me = getSession();
          const isAdmin = !!me && (projectCfg?.config?.members ?? []).some(
            (x) => x.email.toLowerCase() === me.email.toLowerCase() && x.role === "admin",
          );
          const isManager = !!me && me.email.toLowerCase() === managerEmail.toLowerCase();
          blocked = !!me && !isManager && !isAdmin;
          if (blocked || isManager || isAdmin) {
            blockHint = `${name}님이 ${base}의 병합 관리자입니다. 병합은 관리자만 할 수 있습니다.`;
          }
        }
      }
      if (blocked) {
        btn.disabled = true;
        btn.title = blockHint;
      } else if (blockHint) {
        // 관리자/본인인 경우 힌트만 보여 준다.
        const hint = document.createElement("div");
        hint.className = "text-display-xs text-[color:var(--color-ink-muted)]";
        hint.textContent = blockHint;
        action.appendChild(hint);
      }
      btn.addEventListener("click", async () => {
        const ok = await confirmDialog({
          title: `${base}로 병합`,
          message: `${b.short_name} 브랜치를 ${base}에 병합합니다.\n앞으로 ${b.ahead}개 커밋, 변경 파일 ${b.changed_files.length}개.`,
        });
        if (!ok) return;
        setBusy(btn, true, "병합 중…");
        try {
          const out: MergeOutcome = await ipc.startMerge(repo.id, b.name, base);
          if (out.ok) {
            toast(`${b.short_name} 병합 완료`, "success");
            await pushMergedBranch();
            await refresh();
          } else if (out.conflicted) {
            toast(`충돌 ${out.conflicted_files.length}개를 해결해야 합니다.`, "info");
            mergeState = { in_progress: true, conflicted_files: out.conflicted_files };
            knownConflicts = new Set(out.conflicted_files);
            await loadConflicts();
            renderBanner();
            renderPanel();
          } else {
            toast(out.message || "병합에 실패했습니다.", "error");
          }
        } catch (e) {
          const msg = (e as Error).message ?? String(e);
          if (msg.includes("변경")) {
            toast(`${msg} — 작업 탭에서 처리하세요.`, "error");
            location.hash = `#repo/${repo.id}/work`;
          } else {
            toast(`병합 실패: ${msg}`, "error");
          }
        } finally {
          setBusy(btn, false);
        }
      });
      action.appendChild(btn);
      card.appendChild(action);
      list.appendChild(card);
    }
  }

  function showPushBanner() {
    pushBanner.style.display = "";
    pushBanner.innerHTML = "";
    const iw = document.createElement("span");
    iw.className = "gc-banner__icon";
    iw.appendChild(icon("push", 20));
    pushBanner.appendChild(iw);
    const span = document.createElement("span");
    span.className = "gc-banner__body flex-1";
    span.textContent = `origin/${base}에 푸시가 필요합니다`;
    pushBanner.appendChild(span);
    const pushBtn = document.createElement("button");
    pushBtn.className = "gc-button-primary";
    pushBtn.textContent = `origin/${base}에 push`;
    pushBtn.addEventListener("click", async () => {
      setBusy(pushBtn, true, "push 중…");
      try {
        const outcome = await openPushCredentialFlow(repo, base);
        if (outcome === "ok") {
          toast(`${base} push 완료 — 팀원에게 알림이 전송됩니다.`, "success");
          pushBanner.style.display = "none";
        } else if (outcome !== "cancelled") {
          toast(`push 실패: ${outcome.message || "알 수 없는 오류"}`, "error");
        }
      } catch (e) {
        toast(`push 실패: ${(e as Error).message ?? e}`, "error");
      } finally {
        setBusy(pushBtn, false);
      }
    });
    pushBanner.appendChild(pushBtn);
  }

  /** 병합 커밋 후 자동 푸시 — 실패하면 배너로 재시도를 남긴다. */
  async function pushMergedBranch(): Promise<void> {
    try {
      const outcome = await openPushCredentialFlow(repo, base);
      if (outcome === "ok") {
        toast(`${base} push 완료 — 팀원에게 알림이 전송됩니다.`, "success");
        return;
      }
      if (outcome === "cancelled") {
        toast("푸시를 취소했습니다. 아래 배너에서 다시 시도할 수 있습니다.", "info");
      } else {
        const msg = (outcome as PushOutcome).message || "알 수 없는 오류";
        toast(`push 실패: ${msg}`, "error");
      }
      showPushBanner();
    } catch (e) {
      toast(`push 실패: ${(e as Error).message ?? e}`, "error");
      showPushBanner();
    }
  }

  async function loadConflicts() {
    if (!mergeState) return;
    const remaining = mergeState.conflicted_files;
    if (selectedPath && !remaining.includes(selectedPath)) {
      selectedPath = remaining[0] ?? null;
    }
    if (!selectedPath && remaining.length > 0) {
      selectedPath = remaining[0]!;
    }
    // Selected file first, then the rest — lazy, so unopened files don't
    // trigger SSH round-trips. Already-cached states (with any unsaved edits)
    // are reused untouched.
    const paths = selectedPath
      ? [selectedPath, ...remaining.filter((p) => p !== selectedPath)]
      : remaining;
    for (const path of paths) {
      if (conflictCache.has(path)) continue;
      try {
        const detail = await ipc.conflictDetail(repo.id, path);
        const blocks = parseConflictBlocks(detail.working);
        const edits: BlockEdit[] = blocks.map((b) => ({ body: b.ours, history: [] }));
        conflictCache.set(path, { detail, blocks, edits, loading: false });
      } catch (e) {
        toast(`충돌 파일을 불러오지 못했습니다: ${(e as Error).message ?? e}`, "error");
      }
    }
  }

  function cached(path: string | null): ConflictFileState | undefined {
    return path ? conflictCache.get(path) : undefined;
  }

  function renderPanel() {
    const remaining = mergeState?.conflicted_files ?? [];
    if (remaining.length === 0 && !mergeState?.in_progress) {
      panel.style.display = "none";
      return;
    }
    panel.style.display = "";
    panel.innerHTML = "";

    const head = document.createElement("div");
    head.className = "flex items-start gap-3";
    const fileList = document.createElement("div");
    fileList.className = "flex flex-col gap-1 w-64";
    for (const path of knownConflicts) {
      const resolved = !remaining.includes(path);
      const item = document.createElement("button");
      item.className = "gc-select-item" +
        (selectedPath === path ? " is-active" : "") +
        (resolved ? " is-resolved" : "");
      if (resolved) {
        item.appendChild(icon("check", 14));
      } else {
        const dot = document.createElement("span");
        dot.className = "gc-select-item__dot";
        item.appendChild(dot);
      }
      const label = document.createElement("span");
      label.className = "truncate flex-1 text-left";
      label.textContent = path;
      item.appendChild(label);
      item.addEventListener("click", async () => {
        selectedPath = path;
        await loadConflicts();
        renderPanel();
      });
      fileList.appendChild(item);
    }
    head.appendChild(fileList);

    // One-click auto resolve — the whole point of this feature.
    const actionCol = document.createElement("div");
    actionCol.className = "flex-1 flex flex-col items-end gap-2";
    if (remaining.length > 0) {
      const autoBtn = document.createElement("button");
      autoBtn.className = "gc-button-primary inline-flex items-center gap-1";
      autoBtn.appendChild(icon("sparkles", 16));
      const autoLabel = document.createElement("span");
      autoLabel.textContent = "AI 자동 병합";
      autoBtn.appendChild(autoLabel);
      autoBtn.addEventListener("click", () => openAutoResolve());
      actionCol.appendChild(autoBtn);
      const hint = document.createElement("div");
      hint.className = "text-display-sm text-[color:var(--color-ink-muted)] text-right";
      hint.textContent = aiEnabled
        ? "AI 보조로 충돌을 해결하고 병합 커밋까지 완료합니다."
        : "규칙 기반(나의 것/상대 것)으로 해결하고 병합 커밋까지 완료합니다.";
      actionCol.appendChild(hint);
    }
    head.appendChild(actionCol);
    panel.appendChild(head);

    // Everything resolved — offer the final commit.
    if (remaining.length === 0) {
      const done = document.createElement("div");
      done.className = "gc-card flex flex-col gap-3 flex-1";
      const doneText = document.createElement("div");
      doneText.textContent = "모든 충돌이 해소되었습니다.";
      done.appendChild(doneText);
      const commitBtn = document.createElement("button");
      commitBtn.className = "gc-button-primary self-start";
      commitBtn.textContent = "병합 완료";
      commitBtn.addEventListener("click", async () => {
        setBusy(commitBtn, true, "커밋 중…");
        try {
          const out = await ipc.completeMerge(repo.id);
          if (out.ok) {
            toast("병합이 완료되었습니다.", "success");
            conflictCache.clear();
            selectedPath = null;
            mergeState = null;
            await pushMergedBranch();
            await refresh();
          }
        } catch (e) {
          toast(`완료 실패: ${(e as Error).message ?? e}`, "error");
        } finally {
          setBusy(commitBtn, false);
        }
      });
      done.appendChild(commitBtn);
      panel.appendChild(done);
      return;
    }

    const c = cached(selectedPath);
    if (!c) {
      const loading = document.createElement("div");
      loading.className = "flex-1 text-display-sm text-[color:var(--color-ink-muted)]";
      loading.textContent = "충돌 내용을 불러오는 중…";
      panel.appendChild(loading);
      void loadConflicts().then(renderPanel).catch(() => {});
      return;
    }

    const right = document.createElement("div");
    right.className = "flex-1 flex flex-col gap-3";

    if (c.detail.is_binary || c.detail.too_large) {
      const note = document.createElement("div");
      note.className = "text-display-sm text-[color:var(--color-ink-muted)]";
      note.textContent = c.detail.is_binary
        ? "바이너리 파일 — ours 또는 theirs만 선택할 수 있습니다."
        : "파일이 너무 큽니다 — ours 또는 theirs만 선택할 수 있습니다.";
      right.appendChild(note);
      const btnRow = document.createElement("div");
      btnRow.className = "flex gap-2";
      for (const side of ["ours", "theirs"] as const) {
        const b = document.createElement("button");
        b.className = "gc-button-secondary";
        b.textContent = side === "ours" ? "나 것 사용" : "상대 것 사용";
        b.addEventListener("click", () => applyResolution({ type: side }));
        btnRow.appendChild(b);
      }
      right.appendChild(btnRow);
    } else {
      const blocksContainer = document.createElement("div");
      blocksContainer.className = "flex flex-col gap-3";
      c.blocks.forEach((b, idx) => {
        blocksContainer.appendChild(renderBlock(c, b, idx));
      });
      right.appendChild(blocksContainer);

      const saveRow = document.createElement("div");
      saveRow.className = "flex items-center gap-2";
      const saveBtn = document.createElement("button");
      saveBtn.className = "gc-button-primary";
      saveBtn.textContent = "파일 저장하고 스테이징";
      saveBtn.addEventListener("click", () => applyResolution({
        type: "manual",
        content: reassemble(c.detail.working, c.blocks, c.edits.map((e) => e.body)),
      }));
      saveRow.appendChild(saveBtn);
      right.appendChild(saveRow);
    }

    panel.appendChild(right);
  }

  function renderBlock(c: ConflictFileState, b: ConflictBlock, idx: number): HTMLElement {
    const block = document.createElement("div");
    block.className = "gc-card flex flex-col gap-2";
    const header = document.createElement("div");
    header.className = "text-display-sm font-medium";
    header.textContent = `블록 ${idx + 1} · 줄 ${b.startLine}–${b.endLine}`;
    block.appendChild(header);

    const grid = document.createElement("div");
    grid.className = "grid grid-cols-2 gap-2";
    for (const side of [
      { label: "ours", body: b.ours, ko: "나(현재)" },
      { label: "theirs", body: b.theirs, ko: "가져옴" },
    ] as const) {
      const col = document.createElement("div");
      col.className = "flex flex-col gap-1";
      const label = document.createElement("div");
      const chip = document.createElement("span");
      chip.className = `gc-badge gc-badge--${side.label === "ours" ? "success" : "info"} font-mono`;
      chip.textContent = `${side.label} · ${side.ko}`;
      label.appendChild(chip);
      col.appendChild(label);
      const pre = document.createElement("pre");
      pre.className = "bg-[color:var(--color-surface-strong)] p-2 rounded text-display-sm overflow-x-auto whitespace-pre-wrap";
      pre.textContent = side.body || "(비어 있음)";
      col.appendChild(pre);
      const pickBtn = document.createElement("button");
      pickBtn.className = "gc-button-secondary text-display-sm";
      pickBtn.textContent = side.label === "ours" ? "나 것 선택" : "상대 것 선택";
      pickBtn.addEventListener("click", () => {
        pushEdit(c.edits, idx, side.body);
        renderPanel();
      });
      col.appendChild(pickBtn);
      grid.appendChild(col);
    }
    block.appendChild(grid);

    const edit = document.createElement("div");
    edit.className = "flex flex-col gap-1";
    const editLabel = document.createElement("div");
    editLabel.className = "text-display-sm font-mono text-[color:var(--color-ink-muted)]";
    editLabel.textContent = `현재 블록 결과 (${c.edits[idx]?.body.length ?? 0}자)`;
    edit.appendChild(editLabel);
    const ta = document.createElement("textarea");
    ta.className = "gc-input font-mono text-display-sm min-h-24";
    ta.value = c.edits[idx]?.body ?? "";
    ta.addEventListener("input", () => {
      c.edits[idx]!.body = ta.value;
    });
    edit.appendChild(ta);
    const tools = document.createElement("div");
    tools.className = "flex gap-2";
    const aiBtn = document.createElement("button");
    aiBtn.className = "gc-button-secondary text-display-sm inline-flex items-center gap-1";
    aiBtn.appendChild(icon("sparkles", 14));
    const aiLabel = document.createElement("span");
    aiLabel.textContent = "AI 제안";
    aiBtn.appendChild(aiLabel);
    aiBtn.style.display = aiEnabled ? "" : "none";
    aiBtn.addEventListener("click", async () => {
      setBusy(aiBtn, true, "AI 호출 중…");
      try {
        const suggestion = await ipc.aiSuggestResolution(
          c.detail.path,
          c.detail.base,
          b.ours,
          b.theirs,
        );
        pushEdit(c.edits, idx, suggestion);
        renderPanel();
      } catch (e) {
        toast(`AI 제안 실패: ${(e as Error).message ?? e}`, "error");
      } finally {
        setBusy(aiBtn, false);
      }
    });
    tools.appendChild(aiBtn);
    const undoBtn = document.createElement("button");
    undoBtn.className = "gc-button-secondary text-display-sm";
    undoBtn.textContent = "되돌리기";
    undoBtn.disabled = !(c.edits[idx]?.history.length);
    undoBtn.addEventListener("click", () => {
      const prev = popEdit(c.edits, idx);
      if (prev !== null) renderPanel();
    });
    tools.appendChild(undoBtn);
    edit.appendChild(tools);
    block.appendChild(edit);
    return block;
  }

  // ── One-click auto merge ──────────────────────────────────────────────────
  function openAutoResolve() {
    let strategy: "ours" | "theirs" = "theirs";
    const m = openModal({
      title: "AI 자동 병합",
      description: "충돌 파일을 먼저 백업한 뒤 AI(또는 규칙)로 자동 해결하고 병합 커밋을 만듭니다.",
      submitLabel: "자동 병합 시작",
      onSubmit: async (close) => {
        m.setSubmitting(true);
        try {
          const report = await ipc.mergeAutoResolve(repo.id, strategy);
          close();
          await afterAutoResolve(report);
        } catch (e) {
          m.setSubmitting(false);
          m.setError((e as Error).message ?? String(e));
        }
      },
    });

    const wrap = document.createElement("div");
    wrap.className = "flex flex-col gap-3";

    const strategyWrap = document.createElement("div");
    strategyWrap.className = "flex flex-col gap-1";
    const strategyLabel = document.createElement("label");
    strategyLabel.className = "text-display-sm";
    strategyLabel.textContent = "바이너리·대용량 파일 처리";
    strategyWrap.appendChild(strategyLabel);
    const strategySel = document.createElement("select");
    strategySel.className = "gc-input";
    const theirOpt = document.createElement("option");
    theirOpt.value = "theirs";
    theirOpt.textContent = "상대 것(가져온 브랜치) 사용 — 기본";
    strategySel.appendChild(theirOpt);
    const ourOpt = document.createElement("option");
    ourOpt.value = "ours";
    ourOpt.textContent = "나의 것(현재 브랜치) 사용";
    strategySel.appendChild(ourOpt);
    strategySel.addEventListener("change", () => {
      strategy = strategySel.value === "ours" ? "ours" : "theirs";
    });
    strategyWrap.appendChild(strategySel);
    wrap.appendChild(strategyWrap);

    const note = document.createElement("div");
    note.className = "text-display-sm text-[color:var(--color-ink-muted)]";
    note.textContent = aiEnabled
      ? "AI 보조가 켜져 있습니다. AI 결과가 부적절하면 규칙 기반으로 대체됩니다."
      : "AI가 꺼져 있어 규칙 기반으로 처리합니다. 원본은 항상 백업됩니다.";
    wrap.appendChild(note);

    m.body.appendChild(wrap);
  }

  async function afterAutoResolve(report: AutoResolveReport) {
    if (report.committed) {
      conflictCache.clear();
      knownConflicts = new Set();
      selectedPath = null;
      mergeState = null;
      await pushMergedBranch();
    } else if (report.remaining.length > 0) {
      // Partial success — the leftover files are still waiting.
      mergeState = { in_progress: true, conflicted_files: report.remaining };
      for (const p of report.remaining) knownConflicts.add(p);
      await loadConflicts();
      renderBanner();
    }
    await refresh();
    showAutoResolveReport(report);
  }

  function showAutoResolveReport(report: AutoResolveReport) {
    const m = openModal({
      title: "자동 병합 결과",
      cancelLabel: "닫기",
    });
    const wrap = document.createElement("div");
    wrap.className = "flex flex-col gap-3";

    const summary = document.createElement("div");
    summary.textContent = report.message;
    wrap.appendChild(summary);

    if (report.resolved.length > 0) {
      const lbl = document.createElement("div");
      lbl.className = "text-display-sm font-medium";
      lbl.textContent = "해결된 파일";
      wrap.appendChild(lbl);
      const rows = document.createElement("div");
      rows.className = "flex flex-col gap-1";
      const meta: Record<string, { label: string; cls: string }> = {
        ai: { label: "AI 해결", cls: "gc-badge--success" },
        ours: { label: "나의 것", cls: "gc-badge--info" },
        theirs: { label: "상대 것", cls: "gc-badge--warning" },
      };
      for (const r of report.resolved) {
        const row = document.createElement("div");
        row.className = "flex items-center gap-2 min-w-0";
        const chip = document.createElement("span");
        chip.className = `gc-badge ${meta[r.method]?.cls ?? "gc-badge--muted"} shrink-0`;
        chip.textContent = meta[r.method]?.label ?? r.method;
        row.appendChild(chip);
        const pathEl = document.createElement("span");
        pathEl.className = "font-mono text-display-sm truncate";
        pathEl.textContent = r.path;
        row.appendChild(pathEl);
        rows.appendChild(row);
        if (r.note) {
          const note = document.createElement("div");
          note.className = "text-display-sm text-[color:var(--color-ink-muted)] pl-9";
          note.textContent = r.note;
          rows.appendChild(note);
        }
      }
      wrap.appendChild(rows);
    }

    if (report.remaining.length > 0) {
      const lbl = document.createElement("div");
      lbl.className = "text-display-sm font-medium text-[color:var(--color-danger)]";
      lbl.textContent = "남은 충돌 — 병합 센터에서 처리하세요";
      wrap.appendChild(lbl);
      for (const p of report.remaining) {
        const row = document.createElement("div");
        row.className = "font-mono text-display-sm";
        row.textContent = p;
        wrap.appendChild(row);
      }
    }
    m.body.appendChild(wrap);
  }

  // ── Backup restore (safety net) ───────────────────────────────────────────
  async function loadBackups() {
    try {
      backups = await ipc.mergeBackupList(repo.id);
    } catch {
      backups = [];
    }
    renderBackupCard();
  }

  function renderBackupCard() {
    if (!mergeState?.in_progress || backups.length === 0) {
      backupCard.style.display = "none";
      return;
    }
    backupCard.style.display = "";
    backupCard.innerHTML = "";
    const title = document.createElement("div");
    title.className = "font-medium";
    title.textContent = "원본 백업 복원";
    backupCard.appendChild(title);
    const desc = document.createElement("div");
    desc.className = "text-display-sm text-[color:var(--color-ink-muted)]";
    desc.textContent = "자동 병합이 시작되기 전의 충돌 원본입니다. 복원하면 병합 상태는 유지됩니다.";
    backupCard.appendChild(desc);
    for (const b of backups) {
      const row = document.createElement("div");
      row.className = "flex items-center gap-2";
      const ts = document.createElement("span");
      ts.className = "flex-1 text-display-sm min-w-0";
      let when = b.created_at;
      try {
        when = new Date(b.created_at).toLocaleString("ko-KR");
      } catch {
        // keep raw RFC3339 string
      }
      ts.textContent = `${when} · 파일 ${b.files.length}개`;
      row.appendChild(ts);
      const btn = document.createElement("button");
      btn.className = "gc-button-secondary text-display-sm";
      btn.textContent = "복원";
      btn.addEventListener("click", async () => {
        const ok = await confirmDialog({
          title: "원본 복원",
          message: `파일 ${b.files.length}개를 자동 병합 전의 충돌 원본으로 되돌립니다.\n현재 편집 내용은 덮어써지며, 병합 상태는 유지됩니다.`,
        });
        if (!ok) return;
        setBusy(btn, true, "복원 중…");
        try {
          const n = await ipc.mergeBackupRestore(repo.id, b.id);
          toast(`파일 ${n}개를 복원했습니다.`, "success");
          // Cached parse results are stale — reload from the restored worktree.
          conflictCache.clear();
          selectedPath = null;
          await loadConflicts();
          renderBanner();
          renderPanel();
        } catch (e) {
          toast(`복원 실패: ${(e as Error).message ?? e}`, "error");
        } finally {
          setBusy(btn, false);
        }
      });
      row.appendChild(btn);
      backupCard.appendChild(row);
    }
  }

  async function applyResolution(r: Resolution) {
    const c = cached(selectedPath);
    if (!c) return;
    try {
      const remaining = await ipc.resolveConflict(repo.id, c.detail.path, r);
      for (const p of remaining) knownConflicts.add(p);
      conflictCache.delete(c.detail.path);
      mergeState = { in_progress: true, conflicted_files: remaining };
      if (remaining.length === 0) {
        await refresh();
      } else {
        await loadConflicts();
        renderBanner();
        renderPanel();
      }
    } catch (e) {
      toast(`해결 실패: ${(e as Error).message ?? e}`, "error");
    }
  }

  async function refresh() {
    try {
      // 설정이 언제든 바뀔 수 있으므로 열 때마다 다시 읽는다.
      projectCfg = await ipc.projectConfigGet(repo.id).catch(() => null);
      if (!effectiveTargets().includes(base)) {
        base = initialBase();
        // 유효한 대상이 바뀌었으므로 선택 목록을 재구성한다.
        baseSel.innerHTML = "";
        for (const n of effectiveTargets()) {
          const opt = document.createElement("option");
          opt.value = n;
          opt.textContent = n;
          if (n === base) opt.selected = true;
          baseSel.appendChild(opt);
        }
      }
      mergeState = await ipc.mergeState(repo.id);
      if (mergeState.in_progress) {
        // Seed knownConflicts from the live set the first time we observe a
        // merge; later resolutions only shrink it, never grow it.
        for (const p of mergeState.conflicted_files) knownConflicts.add(p);
        // Drop cached states that are no longer conflicted (resolved here or
        // by another client) — untouched files keep their in-progress edits.
        for (const path of [...conflictCache.keys()]) {
          if (!mergeState.conflicted_files.includes(path)) conflictCache.delete(path);
        }
        await loadConflicts();
        void loadBackups();
      } else {
        knownConflicts = new Set();
        conflictCache.clear();
        selectedPath = null;
        branches = await ipc.listPendingBranches(repo.id, base);
        renderBackupCard();
      }
    } catch (e) {
      toast(`불러오기 실패: ${(e as Error).message ?? e}`, "error");
      branches = [];
      mergeState = null;
    }
    renderBanner();
    renderOverlap();
    if (!mergeState?.in_progress) {
      await renderBranchList();
    }
    renderPanel();
  }

  fetchBtn.addEventListener("click", async () => {
    setBusy(fetchBtn, true, "가져오는 중…");
    try {
      await ipc.fetchRepo(repo.id);
      await refresh();
      toast("가져오기 완료", "success");
    } catch (e) {
      toast(`가져오기 실패: ${(e as Error).message ?? e}`, "error");
    } finally {
      setBusy(fetchBtn, false);
    }
  });

  baseSel.addEventListener("change", () => {
    base = baseSel.value;
    refresh();
  });

  // 계정 전환 시 병합 관리자 게이트를 다시 평가한다.
  window.addEventListener("gc-account-changed", () => {
    if (!mergeState?.in_progress) void renderBranchList();
  });

  await refresh();
  return root;
}
