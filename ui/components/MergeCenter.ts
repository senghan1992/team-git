// Merge Center — UI for listing pending remote branches, starting merges,
// and resolving conflicts one block at a time.
import {
  ipc,
  type AutoResolveReport,
  type BackupEntry,
  type ConflictDetail,
  type MergedRemoteBranch,
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
import { renderChangeMap } from "./ChangeMap";
import { openPushCredentialFlow } from "./PushButton";

interface BlockEdit {
  /** Replacement body for the entire conflict block. */
  body: string;
  /** Stack of previous bodies — top is current, second-from-top is the most recent undo. */
  history: string[];
  /**
   * 사용자가 이 블록에 대해 뭔가 결정을 내렸는가 (선택 버튼·AI 제안·직접 편집).
   * 초기값이 ours 본문이라 "내 것 선택"은 body 비교로는 구분할 수 없다 —
   * 결정하지 않은 블록이 조용히 ours 로 저장되면 가져온 브랜치의 변경이
   * 사라지므로, 저장 전에 이 플래그로 경고한다.
   */
  decided: boolean;
}

function pushEdit(state: BlockEdit[], idx: number, body: string) {
  const cur = state[idx];
  if (!cur) {
    state[idx] = { body, history: [], decided: true };
    return;
  }
  cur.decided = true;
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

export interface MergeCenterOpts {
  /** 작업 탭으로 보내기 — 병합 전에 작업 트리를 정리해야 할 때 쓴다. */
  onGoToWork?: () => void;
}

export async function renderMergeCenter(
  repo: Repo,
  opts: MergeCenterOpts = {},
): Promise<HTMLElement> {
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
  // 설정에서 미리 켜 둔 "충돌 나면 곧바로 자동 해결" 스위치 (시나리오 5).
  let aiAutoResolve = false;
  // Auto-resolve backups (safety net) for the current merge.
  let backups: BackupEntry[] = [];
  // 병합이 끝나 정리해도 되는 원격 브랜치들.
  let mergedRemote: MergedRemoteBranch[] = [];
  // `.gpconfig` — 병합 대상 브랜치 + 브랜치별 병합 관리자.
  let projectCfg: ProjectConfigResult | null = null;

  try {
    const cfg = await ipc.getAiConfig();
    aiEnabled = cfg.enabled;
    aiAutoResolve = cfg.enabled && cfg.auto_resolve;
  } catch {
    aiEnabled = false;
    aiAutoResolve = false;
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
  baseSel.className = "gc-input w-auto";
  baseSel.dataset.baseBranchSelect = "true";
  baseSel.id = "merge-base-select";
  // 라벨 없는 선택 상자는 "main" 이라고만 적힌 칸이 된다 — 무엇을 고르는
  // 자리인지 화면에 적어 둔다.
  const baseLabel = document.createElement("label");
  baseLabel.className = "text-display-sm text-[color:var(--color-ink-muted)]";
  baseLabel.htmlFor = "merge-base-select";
  baseLabel.textContent = "병합 대상";
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
  // "가져오기" 는 git fetch 다. 코드를 바꾸지 않는다는 점을 알려 두면
  // 처음 쓰는 사람이 눌러 보기를 겁내지 않는다.
  fetchBtn.title = "팀원이 새로 push한 내용이 있는지 확인합니다. 내 파일은 바뀌지 않습니다.";
  topRow.appendChild(baseLabel);
  topRow.appendChild(baseSel);
  topRow.appendChild(fetchBtn);
  // 이 브랜치의 병합 관리자 — 카드마다 반복하지 않고 여기서 한 번만 알린다.
  const roleBadge = document.createElement("span");
  roleBadge.className = "gc-badge";
  roleBadge.style.display = "none";
  topRow.appendChild(roleBadge);
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
    // 자동 해결이 도는 중에는 절대 새로고침하지 않는다 — 해결기가 파일과
    // 인덱스를 바꾸는 중이라 화면을 다시 그리면 상태가 어긋난다.
    if (autoRunning) return;
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

  // ── 변경 지도 — 파일 기준으로 "누가 어디를 고치고 있는지" (시나리오 4) ──
  const changeMapHost = document.createElement("div");
  root.appendChild(changeMapHost);

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

  // ── 병합이 끝난 원격 브랜치 정리 ─────────────────────────────────────────
  const cleanupCard = document.createElement("div");
  cleanupCard.className = "gc-card flex flex-col gap-2";
  cleanupCard.style.display = "none";
  root.appendChild(cleanupCard);

  // ── Renderers ───────────────────────────────────────────────────────────

  /** 이 사람이 base로 병합할 수 있는가 — 병합 버튼·브랜치 정리에 같은 규칙. */
  function viewerCanMerge(): boolean {
    const managerEmail = projectCfg?.config?.merge_managers?.[base];
    if (!managerEmail) return true;
    const me = getSession();
    if (!me) return false;
    if (me.email.toLowerCase() === managerEmail.toLowerCase()) return true;
    return (projectCfg?.config?.members ?? []).some(
      (x) => x.email.toLowerCase() === me.email.toLowerCase() && x.role === "admin",
    );
  }
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
  function renderRoleBadge() {
    const managerEmail = projectCfg?.config?.merge_managers?.[base];
    if (!managerEmail) {
      // 관리자 미지정 — 누구나 병합할 수 있다는 사실을 알려 준다.
      roleBadge.style.display = "";
      roleBadge.className = "gc-badge gc-badge--muted";
      roleBadge.textContent = `${base} 병합 관리자 미지정 — 설정 탭에서 지정할 수 있습니다`;
      return;
    }
    const member = projectCfg?.config?.members.find(
      (x) => x.email.toLowerCase() === managerEmail.toLowerCase(),
    );
    const name = member?.name || managerEmail;
    const me = getSession();
    const isManager = !!me && me.email.toLowerCase() === managerEmail.toLowerCase();
    const isAdmin = !!me && (projectCfg?.config?.members ?? []).some(
      (x) => x.email.toLowerCase() === me.email.toLowerCase() && x.role === "admin",
    );
    roleBadge.style.display = "";
    if (isManager || isAdmin) {
      roleBadge.className = "gc-badge gc-badge--success";
      roleBadge.textContent = isManager
        ? `내가 ${base}의 병합 관리자입니다`
        : `관리자 권한으로 ${base}에 병합할 수 있습니다 (담당: ${name})`;
    } else {
      roleBadge.className = "gc-badge gc-badge--muted";
      roleBadge.textContent = `${base} 병합 관리자: ${name} — 병합은 관리자만 할 수 있습니다`;
    }
  }

  function renderChangeMapSection() {
    changeMapHost.innerHTML = "";
    const card = renderChangeMap(branches);
    if (card) changeMapHost.appendChild(card);
  }

  /** 병합 전 리뷰 — 대기 브랜치의 한 파일이 base와 어떻게 다른지 보여 준다. */
  function openBranchFileDiff(b: PendingBranch, path: string) {
    const m = openModal({
      title: path,
      description: `origin/${base} ↔ ${b.short_name} 변경 내용`,
      cancelLabel: "닫기",
    });
    const host = document.createElement("div");
    host.className =
      "flex flex-col gap-0 rounded-md border border-[color:var(--color-hairline)] overflow-x-auto";
    host.innerHTML = `<div class="text-display-sm text-[color:var(--color-ink-muted)] px-3 py-2">불러오는 중…</div>`;
    m.body.appendChild(host);
    ipc
      .branchFileDiff(repo.id, base, b.name, path)
      .then((text) => {
        host.innerHTML = "";
        if (!text || !text.trim()) {
          host.innerHTML = `<div class="text-display-sm text-[color:var(--color-ink-muted)] px-3 py-2">변경 내용이 없습니다</div>`;
          return;
        }
        const nav = document.createElement("div");
        nav.className =
          "flex items-center justify-between px-3 py-1.5 border-b border-[color:var(--color-hairline)] text-display-xs text-[color:var(--color-ink-muted)] font-mono";
        const add = (text.match(/^\+/gm) ?? []).length;
        const del = (text.match(/^-/gm) ?? []).length;
        nav.textContent = `+${add} −${del}`;
        host.appendChild(nav);
        const body = document.createElement("pre");
        body.className = "font-mono text-display-sm leading-5 whitespace-pre px-0 py-0";
        const out = document.createElement("code");
        out.className = "block min-w-max px-3 py-2";
        for (const line of text.replace(/\r\n/g, "\n").split("\n")) {
          const ln = document.createElement("div");
          ln.className = line.startsWith("+")
            ? "bg-[color:var(--color-diff-add)] text-[color:var(--color-success)]"
            : line.startsWith("-")
              ? "bg-[color:var(--color-diff-del)] text-[color:var(--color-danger)]"
              : line.startsWith("@@")
                ? "text-[color:var(--color-ink-muted)]"
                : "text-[color:var(--color-ink)]";
          ln.textContent = line || " ";
          out.appendChild(ln);
        }
        body.appendChild(out);
        host.appendChild(body);
      })
      .catch((e) => {
        host.innerHTML = `<div class="text-display-sm text-[color:var(--color-danger)] px-3 py-2">diff 불러오기 실패</div>`;
        toast(`diff 불러오기 실패: ${(e as Error).message ?? e}`, "error");
      });
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
        // 관리자는 파일 이름만 보고 병합을 결정하지 않는다 — 칩을 누르면
        // base와의 실제 diff가 열린다.
        const chip = document.createElement("button");
        chip.className = "gc-badge gc-badge--muted font-mono cursor-pointer";
        chip.style.color = fileKindColor(cf.kind);
        chip.textContent = `${cf.kind} ${cf.path}`;
        chip.title = `클릭하면 ${base}와의 변경 내용을 봅니다`;
        chip.addEventListener("click", () => openBranchFileDiff(b, cf.path));
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

      // 병합은 됐는데 push가 실패/취소된 브랜치 — 같은 병합을 또 권하지 않는다.
      // 필요한 다음 걸음은 아래 push 배너 하나뿐이다.
      if (b.merged_locally) {
        const doneTag = document.createElement("span");
        doneTag.className = "gc-badge gc-badge--warning";
        doneTag.textContent = `로컬 ${base}에 병합됨 — 푸시 대기`;
        doneTag.title = `이 브랜치는 이미 이 컴퓨터의 ${base}에 병합되었습니다. origin/${base}에 push하면 목록에서 사라집니다.`;
        action.appendChild(doneTag);
        card.appendChild(action);
        list.appendChild(card);
        continue;
      }

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
          // 로그아웃 상태를 열어 두면 로그인한 팀원보다 익명이 더 많은 권한을
          // 갖게 된다 — 관리자가 지정된 브랜치는 로그인해서 본인 확인을 해야
          // 병합 버튼이 열린다.
          blocked = !isManager && !isAdmin;
          if (blocked) {
            blockHint = me
              ? `${name}님이 ${base}의 병합 관리자입니다. 병합은 관리자만 할 수 있습니다.`
              : `이 브랜치에는 병합 관리자(${name})가 지정되어 있습니다. 로그인하면 내가 관리자인지 확인해 병합 버튼을 엽니다.`;
          }
        }
      }
      if (blocked) {
        btn.disabled = true;
        btn.title = blockHint;
        // 잠긴 이유는 툴팁만으로는 안 보인다 — 이 카드에서 왜 못 누르는지
        // 한 줄로 말해 준다. 반대로 내가 관리자일 때는 버튼이 눌리는 것 자체가
        // 답이므로, 카드마다 같은 문장을 반복하지 않는다 (상단에 한 번만 표시).
        const hint = document.createElement("div");
        hint.className = "text-display-xs text-[color:var(--color-ink-muted)] text-right max-w-sm";
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
          // 검토한 tip(sha)을 함께 보낸다 — 목록을 본 뒤 팀원이 push(또는
          // force-push)했다면 백엔드가 병합을 멈추고 새로고침을 요구한다.
          const out: MergeOutcome = await ipc.startMerge(repo.id, b.name, base, b.sha);
          if (out.ok) {
            toast(`${b.short_name} 병합 완료`, "success");
            await pushMergedBranch();
            await refresh();
          } else if (out.conflicted) {
            mergeState = { in_progress: true, conflicted_files: out.conflicted_files };
            knownConflicts = new Set(out.conflicted_files);
            // 대기 목록은 지금 상태를 더 이상 설명하지 않는다 (refresh() 와 같은 규칙).
            branches = [];
            list.innerHTML = "";
            renderChangeMapSection();
            if (aiAutoResolve) {
              // 설정에서 미리 켜 둔 자동 해결 — 관리자가 아무것도 누르지 않아도
              // 저장된 지침대로 AI가 고치고 병합 커밋까지 끝낸다 (시나리오 5).
              // 충돌 본문은 일부러 읽지 않는다: 해결기가 같은 파일을 덮어쓰는
              // 중이라 지금 읽어 봐야 곧 낡은 내용이 된다.
              renderBanner();
              await runAutoResolveNow(out.conflicted_files.length);
            } else {
              toast(`충돌 ${out.conflicted_files.length}개를 해결해야 합니다.`, "info");
              await loadConflicts();
              renderBanner();
              renderPanel();
            }
          } else {
            toast(out.message || "병합에 실패했습니다.", "error");
          }
        } catch (e) {
          const msg = (e as Error).message ?? String(e);
          if (msg.includes("새 push가 있었습니다") || msg.includes("찾을 수 없습니다")) {
            // 검토 후 브랜치가 바뀌었거나(새 push/force-push) 방금 삭제됨 —
            // 목록을 새로 그려 최신 상태를 보여 준다.
            toast(msg, "error");
            await refresh();
          } else if (msg.includes("진행 중인 병합")) {
            toast(msg, "error");
            await refresh();
          } else if (msg.includes("변경")) {
            // 이 앱에는 해시 라우터가 없다 — 예전에는 location.hash 를 바꿔서
            // 아무 일도 일어나지 않았고, 안내만 하고 그 자리에 남았다.
            toast(`${msg} — 작업 탭에서 커밋하거나 스태시한 뒤 다시 시도하세요.`, "error");
            opts.onGoToWork?.();
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

  function showPushBanner(unpushedCount?: number) {
    pushBanner.style.display = "";
    pushBanner.innerHTML = "";
    const iw = document.createElement("span");
    iw.className = "gc-banner__icon";
    iw.appendChild(icon("push", 20));
    pushBanner.appendChild(iw);
    const span = document.createElement("span");
    span.className = "gc-banner__body flex-1";
    span.textContent = unpushedCount
      ? `병합 커밋 ${unpushedCount}개가 아직 origin/${base}에 올라가지 않았습니다 — push해야 팀원에게 전달됩니다`
      : `origin/${base}에 푸시가 필요합니다`;
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
          // "푸시 대기" 카드와 대기 목록이 방금 push로 달라졌다.
          await refresh();
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
        const edits: BlockEdit[] = blocks.map((b) => ({
          body: b.ours,
          history: [],
          decided: false,
        }));
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
      hint.className = "text-display-sm text-[color:var(--color-ink-muted)] text-right max-w-sm";
      hint.textContent = aiEnabled
        ? "저장된 지침으로 AI가 해결하고 병합 커밋까지 완료합니다. AI가 못 고친 파일은 아래에 남겨 두니 직접 확인하세요."
        : "규칙 기반(나의 것/상대 것)으로 한쪽을 골라 해결하고 병합 커밋까지 완료합니다. 고르지 않은 쪽 변경은 사라집니다.";
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
        ? "이미지·압축 파일처럼 줄 단위로 비교할 수 없는 파일입니다 — 한쪽을 통째로 골라야 합니다."
        : "파일이 너무 커서 줄 단위로 비교할 수 없습니다 — 한쪽을 통째로 골라야 합니다.";
      right.appendChild(note);
      const btnRow = document.createElement("div");
      btnRow.className = "flex gap-2";
      for (const side of ["ours", "theirs"] as const) {
        const b = document.createElement("button");
        b.className = "gc-button-secondary";
        b.textContent = side === "ours" ? `내 것 사용 (${base})` : "가져온 것 사용";
        b.addEventListener("click", () => applyResolution({ type: side }));
        btnRow.appendChild(b);
      }
      right.appendChild(btnRow);
    } else if (c.blocks.length === 0) {
      // 충돌 표시(<<<<<<<)가 없는 충돌 — 한쪽 브랜치가 파일을 삭제하고 다른
      // 쪽이 수정한 경우다. 빈 편집 화면을 놓아 두면 저장 버튼이 수정본을
      // 조용히 유지한다 — 무엇이 벌어졌는지 말하고 명시적으로 고르게 한다.
      const oursDeleted = !c.detail.ours && !!c.detail.theirs;
      const theirsDeleted = !!c.detail.ours && !c.detail.theirs;
      const note = document.createElement("div");
      note.className = "text-display-sm text-[color:var(--color-ink-muted)] whitespace-pre-line";
      note.textContent = oursDeleted
        ? `내 쪽(${base})에서 이 파일이 삭제되었고, 가져온 브랜치는 수정했습니다.\n파일을 남길지(수정본 유지) 지울지 골라야 합니다.`
        : theirsDeleted
          ? `가져온 브랜치에서 이 파일이 삭제되었고, 내 쪽(${base})은 수정했습니다.\n파일을 남길지(수정본 유지) 지울지 골라야 합니다.`
          : "이 파일의 충돌은 줄 단위로 비교할 수 없습니다 — 한쪽을 통째로 골라야 합니다.";
      right.appendChild(note);
      const btnRow = document.createElement("div");
      btnRow.className = "flex gap-2";
      for (const side of ["ours", "theirs"] as const) {
        const b = document.createElement("button");
        b.className = "gc-button-secondary";
        const deleted = side === "ours" ? oursDeleted : theirsDeleted;
        b.textContent =
          side === "ours"
            ? deleted
              ? `내 것 사용 (${base}) — 파일 삭제`
              : `내 것 사용 (${base})`
            : deleted
              ? "가져온 것 사용 — 파일 삭제"
              : "가져온 것 사용";
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
      saveBtn.addEventListener("click", async () => {
        // 결정하지 않은 블록은 초기값(내 것)으로 저장된다 — 가져온 브랜치의
        // 변경이 사라질 수 있으므로, 개수를 세어 한 번 확인받는다.
        const undecided = c.blocks.reduce(
          (n, _b, i) => n + (c.edits[i]?.decided ? 0 : 1),
          0,
        );
        if (undecided > 0) {
          const ok = await confirmDialog({
            title: "미결정 블록이 있습니다",
            message: `블록 ${undecided}개를 아직 결정하지 않았습니다.\n결정하지 않은 블록은 내 것(${base}) 그대로 저장됩니다 — 가져온 브랜치의 변경이 사라질 수 있습니다.\n계속할까요?`,
            confirmLabel: "그대로 저장",
          });
          if (!ok) return;
        }
        await applyResolution({
          type: "manual",
          content: reassemble(c.detail.working, c.blocks, c.edits.map((e) => e.body)),
        });
      });
      saveRow.appendChild(saveBtn);
      right.appendChild(saveRow);
    }

    panel.appendChild(right);
  }

  function renderBlock(c: ConflictFileState, b: ConflictBlock, idx: number): HTMLElement {
    const block = document.createElement("div");
    block.className = "gc-card flex flex-col gap-2";
    const header = document.createElement("div");
    header.className = "text-display-sm font-medium flex items-center gap-2";
    const headerText = document.createElement("span");
    headerText.textContent = `블록 ${idx + 1} · 줄 ${b.startLine}–${b.endLine}`;
    header.appendChild(headerText);
    if (!c.edits[idx]?.decided) {
      const undecided = document.createElement("span");
      undecided.className = "gc-badge gc-badge--warning";
      undecided.textContent = "미결정";
      undecided.title = "아직 아무 쪽도 고르지 않았습니다. 그대로 저장하면 내 것(현재 브랜치)이 남습니다.";
      header.appendChild(undecided);
    }
    block.appendChild(header);

    const grid = document.createElement("div");
    grid.className = "grid grid-cols-2 gap-2";
    // 칩과 버튼은 한국어를 앞에 둔다 — ours/theirs 는 git 문서에서 다시 만날
    // 때를 위해 괄호로만 남긴다.
    for (const side of [
      { label: "ours", body: b.ours, ko: `내 것 (${base})` },
      { label: "theirs", body: b.theirs, ko: "가져온 것" },
    ] as const) {
      const col = document.createElement("div");
      col.className = "flex flex-col gap-1";
      const label = document.createElement("div");
      const chip = document.createElement("span");
      chip.className = `gc-badge gc-badge--${side.label === "ours" ? "success" : "info"} font-mono`;
      chip.textContent = `${side.ko} · ${side.label}`;
      label.appendChild(chip);
      col.appendChild(label);
      const pre = document.createElement("pre");
      pre.className = "bg-[color:var(--color-surface-strong)] p-2 rounded text-display-sm overflow-x-auto whitespace-pre-wrap";
      pre.textContent = side.body || "(비어 있음)";
      col.appendChild(pre);
      const pickBtn = document.createElement("button");
      pickBtn.className = "gc-button-secondary text-display-sm";
      pickBtn.textContent = side.label === "ours" ? "이쪽(내 것) 선택" : "이쪽(가져온 것) 선택";
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
      c.edits[idx]!.decided = true;
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

  // ── Auto merge without a prompt (설정에서 미리 켜 둔 경우) ───────────────
  //
  // 시나리오 5: 병합 관리자는 충돌이 났다는 사실을 알아차리고 버튼을 찾을
  // 필요가 없다. 설정에 저장해 둔 지침·전략으로 즉시 해결을 돌리고, 결과만
  // 보고받는다. 실패해도 백업이 남고 MERGE_HEAD가 유지되므로 수동 해결로
  // 이어갈 수 있다.
  let autoRunning = false;
  async function runAutoResolveNow(conflictCount: number) {
    if (autoRunning) return;
    autoRunning = true;
    // 해결 중에는 충돌 편집 패널을 숨긴다 — 파일이 바뀌는 동안 낡은 본문을
    // 편집하게 두면 사용자가 작업을 잃는다.
    panel.style.display = "none";
    showAutoProgress(conflictCount);
    try {
      // strategy 인자를 비워 백엔드가 저장된 설정값을 쓰게 한다.
      const report = await ipc.mergeAutoResolve(repo.id);
      hideAutoProgress();
      await afterAutoResolve(report);
    } catch (e) {
      hideAutoProgress();
      toast(
        `자동 해결 실패: ${(e as Error).message ?? e} — 아래에서 직접 해결하세요.`,
        "error",
      );
      await loadConflicts();
      renderBanner();
      renderPanel();
    } finally {
      autoRunning = false;
    }
  }

  const autoProgress = document.createElement("div");
  autoProgress.className = "gc-banner gc-banner--info";
  autoProgress.style.display = "none";
  // 진행 표시는 변경 지도(큰 카드)보다 위, 병합 배너 바로 아래에 둔다.
  root.insertBefore(autoProgress, changeMapHost);

  function showAutoProgress(n: number) {
    autoProgress.style.display = "";
    autoProgress.innerHTML = "";
    const iw = document.createElement("span");
    iw.className = "gc-banner__icon gc-spin";
    iw.appendChild(icon("sparkles", 20));
    autoProgress.appendChild(iw);
    const body = document.createElement("span");
    body.className = "gc-banner__body flex-1";
    body.textContent = `충돌 ${n}개 — 저장된 지침으로 AI가 자동 해결 중입니다…`;
    autoProgress.appendChild(body);
  }
  function hideAutoProgress() {
    autoProgress.style.display = "none";
    autoProgress.innerHTML = "";
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
    strategyLabel.textContent = aiEnabled
      ? "바이너리·대용량 파일 처리 (diff를 만들 수 없는 파일)"
      : "한쪽 선택 기준 (모든 충돌 파일)";
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
    note.className = "text-display-sm text-[color:var(--color-ink-muted)] whitespace-pre-line";
    note.textContent = aiEnabled
      ? "설정에 저장해 둔 해결 지침으로 AI가 고칩니다.\nAI가 쓸 만한 결과를 못 내고 양쪽이 모두 고친 파일이면, 자동으로 한쪽을 고르지 않고 그대로 남겨 둡니다 — 팀원의 커밋이 조용히 사라지지 않게 하기 위한 규칙입니다.\n원본은 항상 백업됩니다."
      : "AI가 꺼져 있어 규칙 기반으로 처리합니다. 양쪽이 모두 고친 파일도 아래 전략에 따라 한쪽만 남으니, 사라지는 쪽이 있어도 괜찮은지 확인하세요.\n원본은 항상 백업됩니다.";
    wrap.appendChild(note);

    m.body.appendChild(wrap);
  }

  async function afterAutoResolve(report: AutoResolveReport) {
    // AI가 파일을 고쳐 커밋까지 만든 경우, push는 관리자가 결과를 확인한
    // 다음이다 — push되는 순간 팀원 전원에게 동기화 알림이 가므로, 잘못된
    // AI 결과를 확인 없이 팀에 배포하면 되돌릴 길이 없다. 한쪽 규칙만으로
    // 풀린 병합(사람이 이미 아는 내용)은 그대로 바로 push한다.
    const aiTouched = report.resolved.some((r) => r.method === "ai");
    if (report.committed) {
      conflictCache.clear();
      knownConflicts = new Set();
      selectedPath = null;
      mergeState = null;
      if (!aiTouched) {
        await pushMergedBranch();
      }
    } else if (report.remaining.length > 0) {
      // Partial success — the leftover files are still waiting.
      mergeState = { in_progress: true, conflicted_files: report.remaining };
      for (const p of report.remaining) knownConflicts.add(p);
      await loadConflicts();
      renderBanner();
    }
    await refresh();
    showAutoResolveReport(report, report.committed && aiTouched);
  }

  function showAutoResolveReport(report: AutoResolveReport, offerPush = false) {
    const m = openModal({
      title: "자동 병합 결과",
      cancelLabel: "닫기",
    });
    const wrap = document.createElement("div");
    wrap.className = "flex flex-col gap-3";

    const summary = document.createElement("div");
    summary.textContent = report.message;
    wrap.appendChild(summary);

    if (offerPush) {
      const holdNote = document.createElement("div");
      holdNote.className = "text-display-sm text-[color:var(--color-ink-muted)] whitespace-pre-line";
      holdNote.textContent =
        "AI가 고친 파일이 있어 push를 잠시 멈췄습니다 — push되는 순간 팀원 전원에게 동기화 알림이 갑니다.\n결과가 이상하면 아래 '원본 백업 복원'으로 되돌린 뒤 다시 시도하세요.";
      wrap.appendChild(holdNote);
      const pushNow = document.createElement("button");
      pushNow.className = "gc-button-primary self-start";
      pushNow.textContent = `확인했어요 — origin/${base}에 push`;
      pushNow.addEventListener("click", async () => {
        setBusy(pushNow, true, "push 중…");
        try {
          await pushMergedBranch();
          m.close();
          await refresh();
        } finally {
          setBusy(pushNow, false);
        }
      });
      wrap.appendChild(pushNow);
    }

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
      lbl.textContent = "직접 확인해야 하는 파일";
      wrap.appendChild(lbl);
      // 파일 이름만 보여 주면 "오류가 났나?"로 읽힌다. 왜 자동으로 안 고쳤는지
      // 함께 보여 줘야 다음 행동(직접 병합)이 자연스럽게 이어진다.
      const reasons = new Map(
        (report.remainingReasons ?? []).map((r) => [r.path, r.note ?? ""]),
      );
      for (const p of report.remaining) {
        const row = document.createElement("div");
        row.className = "flex flex-col gap-0.5";
        const pathEl = document.createElement("div");
        pathEl.className = "font-mono text-display-sm";
        pathEl.textContent = p;
        row.appendChild(pathEl);
        const why = reasons.get(p);
        if (why) {
          const note = document.createElement("div");
          note.className = "text-display-xs text-[color:var(--color-ink-muted)]";
          note.textContent = why;
          row.appendChild(note);
        }
        wrap.appendChild(row);
      }
      const go = document.createElement("button");
      go.className = "gc-button-primary self-start";
      go.textContent = "충돌 해결하러 가기";
      go.addEventListener("click", () => {
        selectedPath = report.remaining[0] ?? null;
        m.close();
        renderPanel();
        panel.scrollIntoView({ behavior: "smooth", block: "start" });
      });
      wrap.appendChild(go);
    }
    m.body.appendChild(wrap);
  }

  // ── 병합이 끝난 원격 브랜치 정리 ─────────────────────────────────────────
  //
  // 병합·push가 끝난 브랜치는 대기 목록에서 사라질 뿐 origin에는 그대로
  // 남는다 — 죽은 feature 브랜치가 쌓이면 모두의 브랜치 선택 상자가
  // 어지러워진다. 커밋이 전부 base에 들어간 브랜치만 후보로 보여 주고,
  // 삭제 직전에 백엔드가 조상 여부를 다시 확인한다.
  let cleanupExpanded = false;

  async function renderCleanupCard() {
    if (mergeState?.in_progress || !viewerCanMerge() || mergedRemote.length === 0) {
      cleanupCard.style.display = "none";
      return;
    }
    cleanupCard.style.display = "";
    cleanupCard.innerHTML = "";

    const head = document.createElement("div");
    head.className = "flex items-center gap-2";
    const iw = document.createElement("span");
    iw.className = "text-[color:var(--color-ink-muted)]";
    iw.appendChild(icon("branch", 16));
    head.appendChild(iw);
    const title = document.createElement("div");
    title.className = "font-medium flex-1";
    title.textContent = `병합이 끝난 원격 브랜치 ${mergedRemote.length}개`;
    head.appendChild(title);
    const toggle = document.createElement("button");
    toggle.className = "gc-button-secondary text-display-sm";
    toggle.textContent = cleanupExpanded ? "접기" : "정리하기";
    toggle.addEventListener("click", () => {
      cleanupExpanded = !cleanupExpanded;
      void renderCleanupCard();
    });
    head.appendChild(toggle);
    cleanupCard.appendChild(head);

    const desc = document.createElement("div");
    desc.className = "text-display-sm text-[color:var(--color-ink-muted)]";
    desc.textContent = `이 브랜치들의 커밋은 모두 ${base}에 들어 있어 지워도 잃는 것이 없습니다. 정리하면 모두의 브랜치 목록이 깔끔해집니다.`;
    cleanupCard.appendChild(desc);

    if (!cleanupExpanded) return;

    for (const b of mergedRemote) {
      const row = document.createElement("div");
      row.className = "flex items-center gap-2";
      const nameEl = document.createElement("span");
      nameEl.className = "font-mono text-display-sm flex-1 min-w-0 truncate";
      nameEl.textContent = b.short_name;
      nameEl.title = b.name;
      row.appendChild(nameEl);
      const meta = document.createElement("span");
      meta.className = "text-display-sm text-[color:var(--color-ink-muted)] shrink-0";
      meta.textContent = `${b.author} · ${await relativeTime(b.unix_time)}`;
      row.appendChild(meta);
      const delBtn = document.createElement("button");
      delBtn.className = "gc-button-secondary text-display-sm text-[color:var(--color-danger)]";
      delBtn.textContent = "삭제";
      delBtn.addEventListener("click", async () => {
        const ok = await confirmDialog({
          title: "원격 브랜치 삭제",
          message: `origin/${b.short_name} 브랜치를 삭제합니다.\n커밋은 모두 ${base}에 병합되어 있어 잃는 것이 없습니다. ${b.author}님이 이 이름으로 계속 작업 중이어도 다시 push하면 브랜치가 새로 생깁니다.`,
          confirmLabel: "삭제",
          destructive: true,
        });
        if (!ok) return;
        setBusy(delBtn, true, "삭제 중…");
        try {
          await ipc.deleteRemoteBranch(repo.id, base, b.short_name);
          toast(`origin/${b.short_name} 브랜치를 삭제했습니다.`, "success");
          mergedRemote = mergedRemote.filter((x) => x.short_name !== b.short_name);
          await renderCleanupCard();
        } catch (e) {
          toast(`삭제 실패: ${(e as Error).message ?? e}`, "error");
        } finally {
          setBusy(delBtn, false);
        }
      });
      row.appendChild(delBtn);
      cleanupCard.appendChild(row);
    }

    if (mergedRemote.length > 1) {
      const allBtn = document.createElement("button");
      allBtn.className = "gc-button-secondary text-display-sm self-start text-[color:var(--color-danger)]";
      allBtn.textContent = `모두 삭제 (${mergedRemote.length}개)`;
      allBtn.addEventListener("click", async () => {
        const names = mergedRemote.map((x) => x.short_name);
        const ok = await confirmDialog({
          title: "원격 브랜치 모두 삭제",
          message: `병합이 끝난 브랜치 ${names.length}개를 origin에서 삭제합니다:\n${names.join(", ")}\n커밋은 모두 ${base}에 병합되어 있어 잃는 것이 없습니다.`,
          confirmLabel: "모두 삭제",
          destructive: true,
        });
        if (!ok) return;
        setBusy(allBtn, true, "삭제 중…");
        let failed = 0;
        for (const short of names) {
          try {
            await ipc.deleteRemoteBranch(repo.id, base, short);
            mergedRemote = mergedRemote.filter((x) => x.short_name !== short);
          } catch {
            failed += 1;
          }
        }
        setBusy(allBtn, false);
        if (failed > 0) {
          toast(`브랜치 ${names.length - failed}개를 삭제했습니다. ${failed}개는 실패했습니다 — 새 push가 있었을 수 있으니 목록을 다시 확인하세요.`, "error");
        } else {
          toast(`브랜치 ${names.length}개를 삭제했습니다.`, "success");
        }
        await renderCleanupCard();
      });
      cleanupCard.appendChild(allBtn);
    }
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
    // 병합 커밋이 끝난 뒤에야 "AI 결과가 이상하다"는 걸 알아차리는 일이
    // 많다 — 복원 카드는 커밋 후에도 최근 백업이 있는 한 계속 보인다.
    // (오래된 백업까지 늘어놓으면 소음이므로 24시간으로 자른다.)
    const inProgress = !!mergeState?.in_progress;
    const shown = inProgress
      ? backups
      : backups.filter((b) => {
          const t = Date.parse(b.created_at);
          return Number.isFinite(t) && Date.now() - t < 24 * 3600 * 1000;
        });
    if (shown.length === 0) {
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
    desc.textContent = inProgress
      ? "자동 병합이 시작되기 전의 충돌 원본입니다. 복원하면 병합 상태는 유지됩니다."
      : "자동 병합 전의 충돌 원본입니다. 이미 커밋된 뒤라면 복원 후 작업 탭에서 변경을 확인하고 새 커밋으로 정리하세요.";
    backupCard.appendChild(desc);
    for (const b of shown) {
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
        // 병합이 진행 중이면 대기 브랜치 목록은 더 이상 사실이 아니다. 예전에는
        // 병합을 시작한 화면이 그대로 남아 "main(으)로 병합" 버튼이 여전히
        // 눌렸고, 누르면 git 이 거절해 낯선 오류만 떴다. 지금 할 일은 하나뿐
        // (이 병합을 끝내거나 중단하기)이므로 목록을 비운다.
        branches = [];
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
        // 병합 커밋은 만들어졌는데 push가 안 된 상태는 화면(그리고 앱)을
        // 다시 열어도 살아 있어야 한다 — 로컬 base와 origin/base를 비교해
        // 배너를 매번 다시 세운다.
        const unpushed = await ipc.baseUnpushedCount(repo.id, base).catch(() => 0);
        if (unpushed > 0) showPushBanner(unpushed);
        else pushBanner.style.display = "none";
        void loadBackups();
        mergedRemote = await ipc
          .listMergedRemoteBranches(repo.id, base)
          .catch(() => [] as MergedRemoteBranch[]);
      }
    } catch (e) {
      toast(`불러오기 실패: ${(e as Error).message ?? e}`, "error");
      branches = [];
      mergeState = null;
    }
    renderBanner();
    renderRoleBadge();
    renderChangeMapSection();
    if (!mergeState?.in_progress) {
      await renderBranchList();
    } else {
      // 병합 중에는 정리 카드도 치운다 — 지금 할 일은 하나뿐이다.
      mergedRemote = [];
      list.innerHTML = "";
    }
    renderPanel();
    void renderCleanupCard();
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
    renderRoleBadge();
    if (!mergeState?.in_progress) void renderBranchList();
  });

  await refresh();
  return root;
}
