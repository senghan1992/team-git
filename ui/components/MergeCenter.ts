// Merge Center — 병합 요청 대기열(승인·거절)과 충돌 해결 UI.
//
// 팀원은 자기 브랜치에 자유롭게 push하고, **병합 요청 보내기**를 눌렀을 때만
// 여기 대기열에 오른다. 관리자는 요청마다 변경 파일·커밋을 검토하고
// 병합하기(승인) 또는 요청 거절로 답한다. 요청은 refs/gc-mr/* ref 로
// 원격에 공유되므로 모든 팀원이 같은 대기열을 본다.
import {
  ipc,
  ipc_peer,
  type AutoResolveReport,
  type BackupEntry,
  type ConflictDetail,
  type DeleteBranchOutcome,
  type MergedRemoteBranch,
  type MergeOutcome,
  type MergeState,
  type PendingBranch,
  type ProjectConfigResult,
  type PushCredential,
  type PushOutcome,
  type Repo,
  type RequestedMerge,
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
import { renderMergeTimeline } from "./MergeTimeline";
import { openGitLoginModal, openPushCredentialFlow } from "./PushButton";
import { mergeManagerEmails } from "./nextAction";

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

// ── 병합 탭 캐시 — 같은 저장소 페이지를 머무는 동안 다시 만들지 않는다 ──
// 병합 탭은 fetch → 설정 → 대기열 → 타임라인을 연달아 불러오느라 처음 그리는
// 데 수 초가 걸린다. 탭을 왕복하거나 이용 가이드 모달을 닫을 때마다 이 비용을
// 다 내는 일이 없게, 다 그려진 화면을 통째로 재사용한다. 데이터는 재사용
// 직후 조용히 갱신한다(로딩 화면·깜빡임 없음). 저장소 페이지를 벗어나면
// (다른 저장소·홈·로그아웃) 캐시를 버려서 돌아올 때는 다시 읽는다 —
// 앱 새로고침(F5)에서도 당연히 새로 읽는다.
interface CachedMergeCenter {
  el: HTMLElement;
  refresh: () => Promise<void>;
  dispose: () => void;
}
const mergeCenterCache = new Map<string, CachedMergeCenter>();

/** 병합 탭 화면 — 캐시에 있으면 즉시 돌려주고 데이터만 갱신한다. */
export async function getMergeCenter(
  repo: Repo,
  opts: MergeCenterOpts = {},
): Promise<HTMLElement> {
  const hit = mergeCenterCache.get(repo.id);
  if (hit) {
    void hit.refresh();
    return hit.el;
  }
  return renderMergeCenter(repo, opts);
}

/** 현재 저장소의 캐시만 남기고 나머지는 버린다 (저장소 간 이동 시). */
export function pruneMergeCenterCache(keepRepoId: string): void {
  for (const [id, c] of mergeCenterCache) {
    if (id !== keepRepoId) {
      c.dispose();
      mergeCenterCache.delete(id);
    }
  }
}

/** 캐시를 모두 버린다 (저장소 페이지를 벗어날 때). */
export function disposeMergeCenterCache(): void {
  for (const c of mergeCenterCache.values()) c.dispose();
  mergeCenterCache.clear();
}

export async function renderMergeCenter(
  repo: Repo,
  opts: MergeCenterOpts = {},
): Promise<HTMLElement> {
  const root = document.createElement("section");
  root.className = "flex flex-col gap-4";

  let base: string = repo.default_branch || "main";
  // 승인 대기열 — 팀원이 보낸 병합 요청만. 푸시된 브랜치 전부가 아니다:
  // 푸시는 작업 공유, 요청은 병합 승인 요청이다.
  let requests: RequestedMerge[] = [];
  let mergeState: MergeState | null = null;
  // Set of every conflict path observed since the merge started; survives
  // resolution so the file list can mark resolved items with a ✓.
  let knownConflicts: Set<string> = new Set();
  // Per-file cache of parse results + in-progress block edits. Kept across
  // file switches and refreshes so unsaved edits are never lost while the
  // reviewer moves between files.
  const conflictCache = new Map<string, ConflictFileState>();
  let selectedPath: string | null = null;
  // 방금 병합한 브랜치 — 병합 완료 시점에 그 브랜치의 남은 "병합 요청"
  // 알림을 읽음 처리할 때 쓴다 (동기화로 시작한 병합이면 null).
  let mergeSourceBranch: string | null = null;
  let aiEnabled = false;
  // 설정에서 미리 켜 둔 "충돌 나면 곧바로 자동 해결" 스위치 (시나리오 5).
  let aiAutoResolve = false;
  // 설정에서 켜 둔 "자동 해결 후 곧바로 push" — AI가 고친 병합도 확인 단계
  // 없이 팀에 배포한다 (완전 자동 루프). 끄면 결과 확인 후 push한다.
  let aiAutoPush = false;
  // 이 진행 중 병합에 자동 해결을 이미 시도했는가 — 동기화 충돌로 병합 탭에
  // 진입했을 때 한 번 자동으로 돌려 주되, 부분 실패 후 refresh가 같은 병합을
  // 몇 번이고 재시도해 무한 루프에 빠지지 않게 하는 장치다. 병합이
  // 끝나면(또는 중단되면) 리셋한다.
  let autoTriedThisMerge = false;
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
    aiAutoPush = cfg.enabled && cfg.auto_push;
  } catch {
    aiEnabled = false;
    aiAutoResolve = false;
    aiAutoPush = false;
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
    if (!root.isConnected) return; // 분리 중에는 쉰다 — 캐시로 다시 붙으면 재개된다
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

  // ── 최근 7일 병합 흐름 — base 로 무엇이 언제 합류했는지 상단에서 한눈에 ──
  // `base` 는 아래 select 로 바뀔 수 있으므로 load 콜백이 현재 값을 읽는다.
  const timeline = renderMergeTimeline({
    base,
    load: (days) => ipc.mergeTimeline(repo.id, base, days),
  });
  root.appendChild(timeline.el);

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
    const managers = mergeManagerEmails(projectCfg, base);
    if (managers.length === 0) return true;
    const me = getSession();
    if (!me) return false;
    if (managers.includes(me.email.toLowerCase())) return true;
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
        mergeSourceBranch = null;
        await refresh();
        notifyRepoChanged();
      } catch (e) {
        toast(`중단 실패: ${(e as Error).message ?? e}`, "error");
      } finally {
        setBusy(abortBtn, false);
      }
    });
    banner.appendChild(abortBtn);
  }
  function renderRoleBadge() {
    const managers = mergeManagerEmails(projectCfg, base);
    if (managers.length === 0) {
      // 관리자 미지정 — 누구나 병합할 수 있다는 사실을 알려 준다.
      roleBadge.style.display = "";
      roleBadge.className = "gc-badge gc-badge--muted";
      roleBadge.textContent = `${base} 병합 관리자 미지정 — 설정 탭에서 지정할 수 있습니다`;
      return;
    }
    const names = managers.map((email) => {
      const member = projectCfg?.config?.members.find(
        (x) => x.email.toLowerCase() === email,
      );
      return member?.name || email;
    });
    const me = getSession();
    const meEmail = me?.email.toLowerCase() ?? "";
    const isManager = !!me && managers.includes(meEmail);
    const isAdmin = !!me && (projectCfg?.config?.members ?? []).some(
      (x) => x.email.toLowerCase() === meEmail && x.role === "admin",
    );
    roleBadge.style.display = "";
    if (isManager || isAdmin) {
      roleBadge.className = "gc-badge gc-badge--success";
      roleBadge.textContent = isManager
        ? `내가 ${base}의 병합 관리자입니다`
        : `관리자 권한으로 ${base}에 병합할 수 있습니다 (담당: ${names.join(", ")})`;
    } else {
      roleBadge.className = "gc-badge gc-badge--muted";
      roleBadge.textContent = `${base} 병합 관리자: ${names.join(", ")} — 병합은 관리자만 할 수 있습니다`;
    }
  }

  function renderChangeMapSection() {
    changeMapHost.innerHTML = "";
    // 변경 지도는 대기열의 요청들만 본다 — 승인 대상이 무엇을 고치는지가
    // 관리자에게 필요한 정보다. 요청 없는 push는 여기에 섞이지 않는다.
    const queue: PendingBranch[] = requests.map((rm) => ({
      name: rm.request.ref_path,
      short_name: rm.request.branch,
      sha: rm.request.sha,
      author: rm.request.author,
      unix_time: rm.request.created_at,
      subject: rm.request.title,
      ahead: rm.ahead,
      behind: rm.behind,
      changed_files: rm.changed_files,
    }));
    const card = renderChangeMap(queue);
    if (card) changeMapHost.appendChild(card);
  }

  /** 병합 전 리뷰 — 대기 중인 요청의 한 파일이 base와 어떻게 다른지 보여 준다. */
  function openBranchFileDiff(refName: string, shortName: string, path: string) {
    const m = openModal({
      title: path,
      description: `origin/${base} ↔ ${shortName} 변경 내용`,
      cancelLabel: "닫기",
    });
    const host = document.createElement("div");
    host.className =
      "flex flex-col gap-0 rounded-md border border-[color:var(--color-hairline)] overflow-x-auto";
    host.innerHTML = `<div class="text-display-sm text-[color:var(--color-ink-muted)] px-3 py-2">불러오는 중…</div>`;
    m.body.appendChild(host);
    ipc
      .branchFileDiff(repo.id, base, refName, path)
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
    if (requests.length === 0) {
      const empty = document.createElement("div");
      empty.className = "gc-empty gc-card";
      const iw = document.createElement("span");
      iw.className = "gc-empty__icon";
      iw.appendChild(icon("inbox", 32));
      empty.appendChild(iw);
      const t = document.createElement("div");
      t.className = "gc-empty__title";
      t.textContent = "승인 대기 중인 병합 요청이 없습니다";
      empty.appendChild(t);
      const d = document.createElement("div");
      d.className = "gc-empty__desc";
      d.textContent =
        "팀원이 작업 탭에서 push 후 '병합 요청 보내기'를 누르면 여기 대기열에 나타납니다. push만으로는 오르지 않습니다.";
      empty.appendChild(d);
      list.appendChild(empty);
      return;
    }
    // 대기열 머리말 — 무엇이 몇 건 기다리고 있는지, 승인하면 무엇이 되는지.
    const head = document.createElement("div");
    head.className = "flex items-baseline justify-between gap-2";
    const headTitle = document.createElement("div");
    headTitle.className = "font-medium";
    headTitle.textContent = `승인 대기열 · 병합 요청 ${requests.length}건`;
    head.appendChild(headTitle);
    const headHint = document.createElement("div");
    headHint.className = "text-display-sm text-[color:var(--color-ink-muted)]";
    headHint.textContent = `승인하면 ${base}에 병합되고, 푸시와 함께 팀원에게 동기화 알림이 갑니다.`;
    head.appendChild(headHint);
    list.appendChild(head);
    for (const rm of requests) {
      // 카드 렌더링은 기존 브랜치 카드와 같은 모양을 재사용한다 — 리뷰에
      // 필요한 정보(파일·커밋·카운터)는 동일하고, 요청 메타데이터가 추가된다.
      const b: PendingBranch = {
        name: rm.request.ref_path,
        short_name: rm.request.branch,
        sha: rm.request.sha,
        author: rm.request.author,
        unix_time: rm.request.created_at,
        subject: rm.request.title,
        ahead: rm.ahead,
        behind: rm.behind,
        changed_files: rm.changed_files,
      };
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
      meta.textContent = `${b.author} · 요청 ${await relativeTime(b.unix_time)} · ${b.subject}`;
      titleWrap.appendChild(meta);
      header.appendChild(titleWrap);
      // 병합 요청 상태 배지 — 대기열 카드가 무엇인지 한눈에 알게 한다.
      const requestTag = document.createElement("span");
      requestTag.className = "gc-badge gc-badge--info shrink-0";
      requestTag.textContent = "병합 요청";
      header.appendChild(requestTag);
      if (rm.request.local_only) {
        const warn = document.createElement("span");
        warn.className = "gc-badge gc-badge--warning shrink-0";
        warn.textContent = "이 컴퓨터에만 저장됨";
        warn.title =
          "요청을 원격에 공유하지 못했습니다 (네트워크·권한). 다른 팀원의 대기열에는 보이지 않을 수 있습니다.";
        header.appendChild(warn);
      }
      if (!rm.branch_exists) {
        const gone = document.createElement("span");
        gone.className = "gc-badge gc-badge--muted shrink-0";
        gone.textContent = "브랜치 삭제됨";
        gone.title =
          `원격에서 ${b.short_name} 브랜치가 지워졌지만, 요청 시점의 커밋이 요청 ref에 남아 있어 그대로 병합할 수 있습니다.`;
        header.appendChild(gone);
      }
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
        chip.addEventListener("click", () => openBranchFileDiff(b.name, b.short_name, cf.path));
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
      btn.textContent = "병합하기 (승인)";
      // 병합 대상 브랜치에 관리자가 지정되어 있으면 관리자/어드민만 병합할 수 있다.
      let blocked = false;
      let blockHint = "";
      {
        const managers = mergeManagerEmails(projectCfg, base);
        if (managers.length > 0) {
          const names = managers.map((email) => {
            const member = projectCfg?.config?.members.find(
              (x) => x.email.toLowerCase() === email,
            );
            return member?.name || email;
          });
          const me = getSession();
          const meEmail = me?.email.toLowerCase() ?? "";
          const isAdmin = !!me && (projectCfg?.config?.members ?? []).some(
            (x) => x.email.toLowerCase() === meEmail && x.role === "admin",
          );
          const isManager = !!me && managers.includes(meEmail);
          // 로그아웃 상태를 열어 두면 로그인한 팀원보다 익명이 더 많은 권한을
          // 갖게 된다 — 관리자가 지정된 브랜치는 로그인해서 본인 확인을 해야
          // 병합 버튼이 열린다.
          blocked = !isManager && !isAdmin;
          if (blocked) {
            blockHint = me
              ? `${names.join(", ")}님이 ${base}의 병합 관리자입니다. 병합은 관리자만 할 수 있습니다.`
              : `이 브랜치에는 병합 관리자(${names.join(", ")})가 지정되어 있습니다. 로그인하면 내가 관리자인지 확인해 병합 버튼을 엽니다.`;
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
          title: `병합 요청 승인`,
          message: `${b.short_name} 브랜치의 병합 요청을 ${base}에 병합합니다.\n요청 시점의 커밋 ${b.ahead}개, 변경 파일 ${b.changed_files.length}개.`,
        });
        if (!ok) return;
        setBusy(btn, true, "병합 중…");
        try {
          // 이 병합이 끝나면 그 브랜치의 남은 "병합 요청" 알림과 대기열 항목을 정리한다.
          mergeSourceBranch = b.short_name;
          // 검토한 것은 요청 시점의 tip(sha)이다 — 목록을 본 뒤 브랜치가 바뀌었어도
          // 요청 ref가 고정한 커밋을 병합하므로 검토한 것이 곧 병합되는 것이다.
          const out: MergeOutcome = await ipc.startMerge(repo.id, b.name, base, b.sha);
          if (out.ok) {
            toast(`${b.short_name} 병합 완료 — 요청을 닫습니다.`, "success");
            const merged = mergeSourceBranch;
            mergeSourceBranch = null;
            await pushMergedBranch();
            await refresh();
            // 요청 닫기(원각에서도 사라짐) + 알림 정리 + 홈 카드 갱신.
            if (merged) await finalizeMergedBranch(merged);
            notifyRepoChanged();
          } else if (out.conflicted) {
            mergeState = { in_progress: true, conflicted_files: out.conflicted_files };
            knownConflicts = new Set(out.conflicted_files);
            // 대기 목록은 지금 상태를 더 이상 설명하지 않는다 (refresh() 와 같은 규칙).
            requests = [];
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
            // 요청이 가리키는 커밋을 못 찾음 — 새로고침해 최신 요청을 본다.
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

      // 거절 — 대기열에서 내린다. 브랜치·커밋은 그대로고 요청자는 다시 요청할 수 있다.
      const rejectBtn = document.createElement("button");
      rejectBtn.className = "gc-button-secondary";
      rejectBtn.textContent = "요청 거절";
      if (blocked) {
        rejectBtn.disabled = true;
        rejectBtn.title = blockHint;
      }
      rejectBtn.addEventListener("click", async () => {
        const ok = await confirmDialog({
          title: "병합 요청 거절",
          message: `${b.short_name} 브랜치의 병합 요청을 대기열에서 내립니다.\n브랜치와 커밋은 그대로 남고, ${b.author}님은 작업을 마친 뒤 다시 요청할 수 있습니다.`,
          confirmLabel: "거절",
          destructive: true,
        });
        if (!ok) return;
        setBusy(rejectBtn, true, "정리 중…");
        try {
          await ipc.closeMergeRequest(
            repo.id,
            base,
            b.short_name,
            "rejected",
            await savedCreds(repo.id),
          );
          toast(`${b.short_name} 병합 요청을 거절했습니다.`, "success");
          await refresh();
          notifyRepoChanged();
        } catch (e) {
          toast(`거절 실패: ${(e as Error).message ?? e} — 원격 요청이 남아 있으면 다시 시도하세요.`, "error");
        } finally {
          setBusy(rejectBtn, false);
        }
      });
      action.appendChild(rejectBtn);
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
          notifyRepoChanged();
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

  /** 병합·push 결과가 홈 카드의 "다음 할 일"에 즉시 반영되게 알린다. */
  function notifyRepoChanged() {
    window.dispatchEvent(new CustomEvent("gc-repo-changed", { detail: repo.id }));
  }

  /** 병합이 끝난 브랜치의 뒤정리 — 대기열에서 요청을 닫고(원각에서도 사라지게)
   *  남은 "병합 요청" 알림을 읽음 처리한다. 실패해도 병합 흐름은 막지 않는다:
   *  요청 ref는 base에 push된 뒤 다른 기기의 대기열 조회에서 자동으로 닫힌다
   *  (요청 tip이 base의 조상이 되면 목록에서 치운다). */
  async function finalizeMergedBranch(branch: string) {
    try {
      // 원격 ref 삭제도 쓰기라 HTTPS 원격이면 저장된 푸시 자격증명을 재사용한다.
      await ipc.closeMergeRequest(
        repo.id,
        base,
        branch,
        "merged",
        await savedCreds(repo.id),
      );
    } catch {
      // 대기열 정리는 부가 기능 — 병합 자체는 이미 끝났다.
    }
    await markMergedRead(branch);
  }

  /** 병합이 끝난 브랜치의 남은 "병합 요청" 알림을 읽음 처리한다 — 탭에서
   *  바로 병합한 경우(토스트·수신함 버튼을 안 거친 경우)에도 이미 병합한
   *  항목이 다시 "병합 요청"으로 남지 않게 한다. 실패해도 병합 흐름은 막지
   *  않는다. */
  async function markMergedRead(branch: string) {
    try {
      await ipc_peer.markBranchPushRead(repo.id, branch);
      // 수신함 배지가 즉시 따라오도록 알린다.
      window.dispatchEvent(new CustomEvent("gc-team-read-changed"));
    } catch {
      // 읽음 정리는 부가 기능 — 병합 자체는 이미 끝났다.
    }
  }

  /** 병합 커밋 후 자동 푸시 — 실패하면 배너로 재시도를 남긴다. */
  async function pushMergedBranch(): Promise<void> {
    try {
      const outcome = await openPushCredentialFlow(repo, base);
      if (outcome === "ok") {
        toast(`${base} push 완료 — 팀원에게 알림이 전송됩니다.`, "success");
        notifyRepoChanged();
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
          const out = await ipc.completeMerge(repo.id, mergeSourceBranch ? `${mergeSourceBranch} 브렌치 병합` : undefined);
          if (out.ok) {
            toast("병합이 완료되었습니다.", "success");
            conflictCache.clear();
            selectedPath = null;
            mergeState = null;
            const merged = mergeSourceBranch;
            mergeSourceBranch = null;
            await pushMergedBranch();
            await refresh();
            if (merged) await finalizeMergedBranch(merged);
            notifyRepoChanged();
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
    // 이 병합에 대한 시도를 기록한다 — 성공해도 실패해도 자동 트리거는
    // 병합당 한 번만이다 (무한 재시도 방지, refresh의 트리거 조건 참고).
    autoTriedThisMerge = true;
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
          // 수동 실행도 병합당 시도 기록에 포함 — 실패해도 20초 자동 감지가
          // 몰래 자동 해결을 다시 돌리는 일이 없게 한다.
          autoTriedThisMerge = true;
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
    // 규칙만으로 풀린 병합(사람이 이미 아는 내용, 잃는 것이 없음)은 그대로
    // 바로 push한다. AI가 고친 병합은 기본적으로 결과 확인 후 push — push되는
    // 순간 팀원 전원에게 동기화 알림이 가고 잘못된 결과를 되돌릴 길이 없기
    // 때문이다. 단, 설정에서 "자동 해결 후 곧바로 push"를 켠 팀은 확인 단계를
    // 건너뛰고 완전 자동 루프(해결 → 커밋 → push → 팀원 동기화 알림)로 진행한다.
    // push가 실패하면 배너가 남아 재시도할 수 있다. 무슨 충돌을 어떻게
    // 풀었는지는 자동 해결이 병합 커밋 본문에 기록한다.
    const aiTouched = report.resolved.some((r) => r.method === "ai");
    if (report.committed) {
      conflictCache.clear();
      knownConflicts = new Set();
      selectedPath = null;
      mergeState = null;
      // 병합이 끝났으니 이 브랜치의 "병합 요청"을 정리하고 홈 카드를
      // 깨운다.
      const merged = mergeSourceBranch;
      mergeSourceBranch = null;
      if (!aiTouched || aiAutoPush) {
        await pushMergedBranch();
      }
      if (merged) await finalizeMergedBranch(merged);
      notifyRepoChanged();
    } else if (report.remaining.length > 0) {
      // Partial success — the leftover files are still waiting.
      mergeState = { in_progress: true, conflicted_files: report.remaining };
      for (const p of report.remaining) knownConflicts.add(p);
      await loadConflicts();
      renderBanner();
    }
    await refresh();
    showAutoResolveReport(report, report.committed && aiTouched && !aiAutoPush);
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

  // 병합이 끝난 원격 브랜치 삭제에 쓸 Git 호스트 자격증명 — 푸시와 같은
  // 저장소에서 재사용한다. 한 번 로그인하면 "모두 삭제"의 나머지 브랜치에도
  // 모달 없이 그대로 쓴다. (저장된 값이 거부되면 prefill 로만 남긴다.)
  let deleteCreds: PushCredential | null = null;
  let deleteCredsLoaded = false;

  /** 저장된 푸시 자격증명을 이 저장소 것만 꺼낸다 — 대기열 정리(요청 ref
   *  삭제)는 HTTPS 원격이면 쓰기 동작이라 같은 자격증명이 필요하다. */
  async function savedCreds(repoId: string): Promise<PushCredential | null> {
    const saved = await ipc
      .pushCredentialsList()
      .catch(() => ({} as Record<string, PushCredential>));
    return saved[repoId] ?? null;
  }

  /** 병합이 끝난 원격 브랜치 하나를 삭제한다.
   *
   * HTTPS 원격(mod.lge.com 같은 Git 호스트)은 `push --delete` 도 로그인이
   * 필요하다 — 푸시와 같은 흐름으로 자격증명을 처리한다:
   *   1) 저장된/이번 세션의 자격증명으로 시도 (없으면 null 로 시도)
   *   2) 결과가 auth_required 면 로그인 모달 → 입력값으로 재시도
   * SSH 원격은 자격증명 없이 그대로 성공한다.
   * @returns 성공 시 outcome(로컬 정리 정보 포함) | "failed" | "cancelled" */
  async function deleteMergedRemoteBranch(
    branch: string,
  ): Promise<DeleteBranchOutcome | "failed" | "cancelled"> {
    if (!deleteCredsLoaded) {
      deleteCredsLoaded = true;
      const saved = await ipc
        .pushCredentialsList()
        .catch(() => ({} as Record<string, PushCredential>));
      deleteCreds = saved[repo.id] ?? null;
    }
    const attempt = (creds: PushCredential | null, save: boolean) =>
      ipc.deleteRemoteBranch(repo.id, base, branch, creds, save);

    // 1) 아는 자격증명(저장값·이번 세션 입력값)으로 시도한다.
    let res: DeleteBranchOutcome;
    try {
      res = await attempt(deleteCreds, false);
    } catch (e) {
      toast(`삭제 실패: ${(e as Error).message ?? e}`, "error");
      return "failed";
    }
    if (res.ok) return res;
    if (!res.auth_required) {
      toast(`삭제 실패: ${res.message || "알 수 없는 오류"}`, "error");
      return "failed";
    }

    // 2) HTTPS + 인증 필요 → 로그인 모달. 성공한 값은 이번 세션의 나머지
    //    삭제에 재사용하고, 저장 체크 시 설정에도 보관한다.
    const hadCreds = deleteCreds !== null;
    let lastOutcome: DeleteBranchOutcome | null = null;
    const ok = await openGitLoginModal({
      title: "Git 호스트 로그인",
      description: hadCreds
        ? `${repo.display_name} — 이 자격증명으로는 원격에서 브랜치를 삭제할 수 없습니다. 다시 입력하세요.`
        : `${repo.display_name} — origin에서 브랜치를 삭제하려면 Git 호스트 아이디/비밀번호가 필요합니다.`,
      submitLabel: "삭제",
      prefill: deleteCreds ?? null,
      attempt: async (creds, save) => {
        let r: DeleteBranchOutcome;
        try {
          r = await attempt(creds, save);
        } catch (e) {
          return { ok: false, message: (e as Error).message ?? String(e) };
        }
        if (r.ok) {
          deleteCreds = creds; // "모두 삭제"의 나머지 브랜치에 재사용.
          lastOutcome = r; // 성공 outcome 을 모달 밖 호출자에게 전달.
          if (save) {
            toast("자격증명을 설정에 저장했습니다. 다음부터 자동 입력됩니다.", "info");
          }
          return { ok: true, message: "" };
        }
        return { ok: false, message: r.message || (r.auth_required ? "로그인 실패" : "삭제 실패") };
      },
    });
    if (!ok || !lastOutcome) return "cancelled";
    return lastOutcome;
  }

  /// 원격 삭제 성공 토스트 — 백엔드가 함께 정리한 로컬 상태(같은 이름의
  /// 로컬 브랜치 등)를 붙여 보여 준다.
  function deleteSuccessToast(short: string, outcome: DeleteBranchOutcome) {
    const cleaned = (outcome.cleaned_locally ?? []).join(", ");
    const kept = (outcome.kept_locally ?? []).join(" · ");
    const note = [cleaned && `${cleaned} 정리`, kept && kept].filter(Boolean).join(" · ");
    toast(
      `origin/${short} 브랜치를 삭제했습니다${note ? ` — ${note}` : ""}.`,
      kept ? "info" : "success",
    );
  }

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
    const mineCount = mergedRemote.filter((b) => b.mine).length;
    const title = document.createElement("div");
    title.className = "font-medium flex-1";
    title.textContent = `병합이 끝난 원격 브랜치 ${mergedRemote.length}개${mineCount < mergedRemote.length ? ` (내 브랜치 ${mineCount}개)` : ""}`;
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
    desc.textContent =
      mineCount === 0
        ? `이 브랜치들의 커밋은 모두 ${base}에 들어 있어 지워도 잃는 것이 없지만, 본인이 만든 브랜치만 삭제할 수 있습니다.`
        : `이 브랜치들의 커밋은 모두 ${base}에 들어 있어 지워도 잃는 것이 없습니다. 본인이 만든 브랜치만 삭제할 수 있고, 원격·로컬 브랜치가 함께 삭제됩니다.`;
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
      // 본인이 만든 브랜치만 삭제할 수 있다 — 작성자 표시와 함께 비활성 버튼으로
      // 이유를 알려 준다 (목록에서 숨기지 않는 이유: 누가 뭘 만들었는지 보여야
      // 삭제가 왜 안 되는지 알 수 있다).
      const delBtn = document.createElement("button");
      if (!b.mine) {
        delBtn.className =
          "gc-button-secondary text-display-sm opacity-40 cursor-not-allowed";
        delBtn.disabled = true;
        delBtn.textContent = "작성자만 삭제";
        delBtn.title = `${b.author}님이 만든 브랜치입니다. 본인이 만든 브랜치만 삭제할 수 있습니다.`;
      } else {
        delBtn.className = "gc-button-secondary text-display-sm text-[color:var(--color-danger)]";
        delBtn.textContent = "삭제";
        delBtn.addEventListener("click", async () => {
          const ok = await confirmDialog({
            title: "원격 브랜치 삭제",
            message: `origin/${b.short_name} 브랜치를 삭제합니다.\n이 폴더의 같은 이름 로컬 브랜치와 원격 브랜치가 함께 삭제됩니다.\n커밋은 모두 ${base}에 병합되어 있어 잃는 것이 없습니다.`,
            confirmLabel: "삭제",
            destructive: true,
          });
          if (!ok) return;
          setBusy(delBtn, true, "삭제 중…");
          try {
            const r = await deleteMergedRemoteBranch(b.short_name);
            if (typeof r !== "string" && r.ok) {
              deleteSuccessToast(b.short_name, r);
              mergedRemote = mergedRemote.filter((x) => x.short_name !== b.short_name);
              await renderCleanupCard();
            }
            // "failed"는 함수 안에서 이미 토스트로 알렸고, "cancelled"는 취소다.
          } finally {
            setBusy(delBtn, false);
          }
        });
      }
      row.appendChild(delBtn);
      cleanupCard.appendChild(row);
    }

    // "모두 삭제"는 내 브랜치만 대상이다.
    const myBranches = mergedRemote.filter((x) => x.mine);
    if (myBranches.length > 1) {
      const allBtn = document.createElement("button");
      allBtn.className = "gc-button-secondary text-display-sm self-start text-[color:var(--color-danger)]";
      allBtn.textContent = `내 브랜치 모두 삭제 (${myBranches.length}개)`;
      allBtn.addEventListener("click", async () => {
        const names = myBranches.map((x) => x.short_name);
        const ok = await confirmDialog({
          title: "내 브랜치 모두 삭제",
          message: `본인이 만든 브랜치 ${names.length}개를 origin과 이 폴더의 로컬에서 함께 삭제합니다:\n${names.join(", ")}\n커밋은 모두 ${base}에 병합되어 있어 잃는 것이 없습니다.`,
          confirmLabel: "모두 삭제",
          destructive: true,
        });
        if (!ok) return;
        setBusy(allBtn, true, "삭제 중…");
        let deleted = 0;
        let failed = 0;
        let cancelled = false;
        const keptNotes: string[] = [];
        for (const short of names) {
          const r = await deleteMergedRemoteBranch(short);
          if (typeof r !== "string" && r.ok) {
            deleted += 1;
            mergedRemote = mergedRemote.filter((x) => x.short_name !== short);
            for (const n of r.kept_locally ?? []) {
              if (!keptNotes.includes(n)) keptNotes.push(n);
            }
          } else if (r === "cancelled") {
            cancelled = true;
            break;
          } else {
            failed += 1;
          }
        }
        setBusy(allBtn, false);
        const keptNote = keptNotes.length ? ` — ${keptNotes.join(" · ")}` : "";
        if (cancelled) {
          toast(`삭제를 중단했습니다. ${deleted}개는 삭제됐습니다.`, "info");
        } else if (failed > 0) {
          toast(`${deleted}개를 삭제했습니다. ${failed}개는 실패했습니다 — 새 push가 있었을 수 있으니 목록을 다시 확인하세요.`, "error");
        } else {
          toast(`브랜치 ${deleted}개를 삭제했습니다.${keptNote}`, keptNotes.length ? "info" : "success");
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
      ? "자동 병합이 시작되기 전의 충돌 원본입니다. 복원하면 병합 상태는 유지되지만, 이미 해결(스테이징)된 파일은 충돌 목록에 다시 나타나지 않으니 복원한 파일을 정리한 뒤 직접 다시 스테이징해야 합니다."
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
    // 타임라인은 병렬로 — 실패해도(스스로 삼킨다) 본 흐름을 막지 않는다.
    void timeline.refresh();
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
        // 병합이 진행 중이면 대기열은 더 이상 사실이 아니다. 예전에는
        // 병합을 시작한 화면이 그대로 남아 "main(으)로 병합" 버튼이 여전히
        // 눌렸고, 누르면 git 이 거절해 낯선 오류만 떴다. 지금 할 일은 하나뿐
        // (이 병합을 끝내거나 중단하기)이므로 목록을 비운다.
        requests = [];
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
        // 동기화(sync) 중 충돌로 병합 탭에 온 팀원도 버튼을 찾을 필요가 없다
        // — 설정에서 자동 해결이 켜져 있고 이 병합에 아직 시도한 적이 없으면
        // 곧바로 돌린다. (병합 시작 직후 경로는 runAutoResolveNow가 이미
        // 기록하므로 여기서 다시 걸리지 않는다. 부분 실패 시에는 사람이
        // 나서서 마무리한다 — 자동 재시도 루프를 만들지 않는다.)
        if (
          aiAutoResolve &&
          !autoTriedThisMerge &&
          mergeState.conflicted_files.length > 0
        ) {
          void runAutoResolveNow(mergeState.conflicted_files.length);
        }
      } else {
        autoTriedThisMerge = false;
        knownConflicts = new Set();
        conflictCache.clear();
        selectedPath = null;
        // 진행 중 병합이 끝나면(완료·중단·자동 해결) 소스 브랜치 기억을
        // 비운다 — 다음 병합부터 새로 기록한다.
        mergeSourceBranch = null;
        // 승인 대기열 — 푸시된 브랜치가 아니라 **병합 요청**만 온다.
        // (요청이 이미 base에 들어갔다면 백엔드의 목록 조회가 자동으로 닫는다.)
        requests = await ipc.listRequestedMerges(repo.id, base);
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
      requests = [];
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

  // 처음 열 때 한 번 fetch — 저장소 등록 직후나 앱 밖 터미널 push 는 20초
  // 자동 감지의 첫 틱을 기다리지 않아도 바로 보인다. fetch 가 실패해도
  // (오프라인) 마지막으로 아는 ref 로 목록을 그린다.
  try {
    await ipc.fetchRepo(repo.id);
  } catch {
    // 오프라인 등 — 아는 ref 만으로 목록을 그린다.
  }

  // 캐시 재진입 시 돌릴 조용한 갱신 — 진행 중 갱신과 겹치지 않게 하나만 세운다.
  let refreshInFlight = false;
  const refreshGuarded = async () => {
    if (refreshInFlight) return;
    refreshInFlight = true;
    try {
      await refresh();
    } finally {
      refreshInFlight = false;
    }
  };
  mergeCenterCache.set(repo.id, {
    el: root,
    refresh: refreshGuarded,
    dispose: () => window.clearInterval(autoTimer),
  });

  await refresh();
  return root;
}
