import { ipc, type ProjectConfigResult, type StashEntry, type WorkingTreeStatus } from "../lib/ipc";
import { openModal, confirmDialog } from "../components/Modal";
import { toast } from "../components/Toast";
import { kindHint, kindLabel } from "../components/StatusTable";
import { getMergeCenter } from "../components/MergeCenter";
import { renderProjectConfigPanel } from "../components/ProjectConfigPanel";
import { closeMergeRequestWithAuth, openPushCredentialFlow, requestMergeWithAuth } from "../components/PushButton";
import { getSession } from "../lib/session";
import { icon } from "../components/Icon";
import { setBusy } from "../components/Busy";
import { mergeManagerEmails } from "../components/nextAction";
import type { RepoTab } from "../components/Sidebar";
export async function renderRepoView(
  repoId: string,
  tab: RepoTab = "work",
  onTab?: (t: RepoTab) => void,
): Promise<HTMLElement> {
  const main = document.createElement("main");
  main.className = "flex-1 overflow-y-auto p-8 flex flex-col gap-6";

  const repos = await ipc.listRepositories();
  const repo = repos.find((r) => r.id === repoId);
  if (!repo) {
    const e = document.createElement("div");
    e.className = "gc-card text-display-md";
    e.textContent = "저장소를 찾을 수 없습니다.";
    main.appendChild(e);
    return main;
  }

  // projectCfg는 원래 아래(설정 탭 섹션)에서 await로 채워졌다. 그런데
  // syncBaseName() 같은 조기 호출자가 이 값을 읽는데, default_branch가
  // 없는 저장소(등록 직후·리모트 미탐지)에서는 단락 평가가 풀려 TDZ 오류로
  // 화면 전체가 깨졌다. 선언만 맨 앞으로 올려 두고, 값은 원래 자리에서 채운다.
  let projectCfg: ProjectConfigResult | null = null;

  // ── Header ────────────────────────────────────────────────────────────────
  const headRow = document.createElement("div");
  // 좌측 "여기는 어디"(프로젝트명·경로), 우측 "여기서 무엇을 할지"(작업 버튼).
  // 데스크톱 앱의 정석 헤더 패턴 — 마우스 사용자의 눈이 먼저 가는 자리에
  // 주요 동사를 놓는다. 좁은 창에서는 버튼이 아랦으로 내려와도 우측 정렬(ml-auto).
  headRow.className = "flex flex-wrap items-center justify-between gap-4";
  const head = document.createElement("div");
  head.className = "gc-page-head min-w-0";
  const title = document.createElement("div");
  title.className = "gc-page-head__title";
  title.textContent = repo.display_name;
  head.appendChild(title);
  const sub = document.createElement("div");
  sub.className = "gc-page-head__sub truncate max-w-md";
  sub.textContent = repo.path;
  head.appendChild(sub);
  headRow.appendChild(head);
  main.appendChild(headRow);
  // ── Tabs (work / merge / config) — segmented control ────────────────────
  const tabs = document.createElement("div");
  tabs.className = "gc-tabs";
  const workBtn = document.createElement("button");
  workBtn.className = "gc-tab " + (tab === "work" ? "is-active" : "");
  workBtn.appendChild(icon("edit", 14));
  const workLabel = document.createElement("span");
  workLabel.textContent = "작업";
  workBtn.appendChild(workLabel);
  workBtn.addEventListener("click", () => onTab?.("work"));
  const mergeBtn = document.createElement("button");
  mergeBtn.className = "gc-tab " + (tab === "merge" ? "is-active" : "");
  mergeBtn.appendChild(icon("merge", 14));
  const mergeLabel = document.createElement("span");
  mergeLabel.textContent = "병합";
  mergeBtn.appendChild(mergeLabel);
  mergeBtn.addEventListener("click", () => onTab?.("merge"));
  const configBtn = document.createElement("button");
  configBtn.className = "gc-tab " + (tab === "config" ? "is-active" : "");
  configBtn.appendChild(icon("settings", 14));
  const configLabel = document.createElement("span");
  configLabel.textContent = "설정";
  configBtn.appendChild(configLabel);
  configBtn.addEventListener("click", () => onTab?.("config"));
  tabs.appendChild(workBtn);
  tabs.appendChild(mergeBtn);
  tabs.appendChild(configBtn);
  main.appendChild(tabs);

  if (tab === "merge") {
    main.appendChild(await getMergeCenter(repo, { onGoToWork: () => onTab?.("work") }));
    return main;
  }
  if (tab === "config") {
    main.appendChild(await renderProjectConfigPanel(repo));
    return main;
  }

  // ── Working branch + status row ───────────────────────────────────────────
  const meta = document.createElement("div");
  meta.className = "flex items-center gap-4 flex-wrap";
  main.appendChild(meta);

  // 브랜치 선택은 이 화면의 첫 행동이다 — 평범한 선택 상자가 아니라 눈에 띄는
  // 컨트롤로 만든다: 브랜치 아이콘 + 라벨 + 모노 고정폭의 선택 상자.
  const branchWrap = document.createElement("label");
  branchWrap.className =
    "inline-flex items-center gap-2 h-10 pl-3 pr-2 rounded-[8px] bg-[color:var(--color-plaque)] border border-[color:var(--color-border-strong)] shadow-[0_1px_2px_rgba(37,39,44,.04)]";
  const branchIcon = document.createElement("span");
  branchIcon.className = "text-[color:var(--color-primary)] inline-flex";
  branchIcon.appendChild(icon("branch", 15));
  branchWrap.appendChild(branchIcon);
  const branchLabel = document.createElement("span");
  branchLabel.className =
    "text-[11px] font-semibold text-[color:var(--color-ink-muted)] tracking-[-0.01em] shrink-0";
  branchLabel.textContent = "작업 브랜치";
  branchWrap.appendChild(branchLabel);

  const branchSel = document.createElement("select");
  branchSel.className =
    "bg-transparent outline-none font-mono text-display-sm font-medium text-[color:var(--color-ink)] cursor-pointer max-w-[280px]";
  // 아이콘도 라벨도 없는 선택 상자였다 — 스크린리더에도, 처음 보는 사람에게도
  // 이게 브랜치라는 정보가 없었다.
  branchSel.setAttribute("aria-label", "현재 작업 브랜치");
  branchWrap.appendChild(branchSel);

  const statusPill = document.createElement("span");
  statusPill.className = "gc-status-chip";

  // 병합 관리자 배지 — 이 브랜치의 규칙을 알려 주는 메타데이터일 뿐이므로
  // 행의 끝(오른쪽)으로 보낸다. 컨트롤(동기화·새 브랜치)과 섞이면
  // "누가 할 일"과 "무엇을 누를지"가 한 행에 섞여 읽힌다.
  const managerBadge = document.createElement("span");
  managerBadge.className = "gc-badge gc-badge--neutral ml-auto";
  managerBadge.style.display = "none";

  meta.appendChild(branchWrap);
  meta.appendChild(statusPill);
  meta.appendChild(managerBadge);

  // 원격 브랜치를 골랐을 때만 나타나는 안내 한 줄 — 항상 떠 있으면 잡음이고,
  // 필요한 순간(서버 브랜치를 고른 순간)에만 읽힌다.
  const branchHint = document.createElement("div");
  branchHint.className = "text-display-xs text-[color:var(--color-primary)] font-medium";
  branchHint.style.display = "none";
  main.appendChild(branchHint);

  function paintBranchHint(selectedRemote: boolean) {
    branchHint.style.display = selectedRemote ? "" : "none";
    if (selectedRemote) {
      branchHint.textContent =
        "서버 브랜치를 골랐습니다 — 전환하면 같은 이름의 내 브랜치로 바뀝니다.";
    }
  }

  // ── Sync — pull latest base into the current branch (step 3 of the flow) ─
  //
  // 이름은 “동기화” 한 단어가 아니라 “병합 브랜치명 + 동기화”로 — 무엇을
  // 가져오는지 버튼만 봐도 보여야 한다 (예: main 동기화).
  const syncBaseName = (): string =>
    repo.default_branch || projectCfg?.config?.default_base_branch || "main";
  function refreshSyncLabel() {
    const base = syncBaseName();
    syncLabel.textContent = `${base} 동기화`;
    syncBtn.title = `origin/${base}의 최신 커밋을 지금 브랜치에 병합합니다.`;
  }
  const syncBtn = document.createElement("button");
  syncBtn.className = "gc-button-secondary inline-flex items-center gap-1";
  syncBtn.appendChild(icon("arrow-right", 14));
  const syncLabel = document.createElement("span");
  refreshSyncLabel();
  syncBtn.appendChild(syncLabel);
  syncBtn.addEventListener("click", async () => {
    const current = branchSel.value.replace(/^origin\//, "");
    const base = syncBaseName();
    const confirmed = await confirmDialog({
      title: `${base} 동기화`,
      message: `현재 브랜치(${current})에 origin/${base}의 최신 내용을 병합합니다.`,
      confirmLabel: `${base} 동기화`,
    });
    if (!confirmed) return;
    setBusy(syncBtn, true, `${base} 동기화 중…`);
    try {
      const r = await ipc.syncBranch(repoId, base);
      await loadBranches();
      if (r.conflicted) {
        toast(`충돌 ${r.files.length}개 발생 — 병합 센터에서 해결하세요.`, "info");
        onTab?.("merge");
      } else {
        toast("동기화 완료", "success");
        applyStatus(await ipc.status(repoId).catch(() => null));
      }
    } catch (e) {
      const msg = (e as Error).message ?? String(e);
      if (msg.includes("병합이 있습니다")) {
        toast(`${msg} — 병합 센터에서 먼저 마무리하세요.`, "error");
        onTab?.("merge");
      } else {
        toast(`동기화 실패: ${msg}`, "error");
      }
    } finally {
      setBusy(syncBtn, false);
    }
  });
  meta.appendChild(syncBtn);

  // ── 새 브랜치 — 자신의 작업 브랜치를 만들고 바로 푸시까지 (aos checkout -b) ──
  const newBranchBtn = document.createElement("button");
  newBranchBtn.className = "gc-button-secondary inline-flex items-center gap-1";
  newBranchBtn.appendChild(icon("branch", 14));
  const nbLabel = document.createElement("span");
  nbLabel.textContent = "새 브랜치";
  newBranchBtn.appendChild(nbLabel);
  meta.appendChild(newBranchBtn);
  newBranchBtn.addEventListener("click", () => {
    const m = openModal({
      title: "새 브랜치",
      description: "현재 브랜치에서 작업 브랜치를 만들어 전환합니다. 팀원 브랜치와 겹치지 않는 이름으로 정하세요.",
      submitLabel: "생성",
      onSubmit: async (close) => {
        const name = (m.body.querySelector<HTMLInputElement>("#nb-name")!).value.trim();
        const pushAfter = (m.body.querySelector<HTMLInputElement>("#nb-push")!)?.checked ?? false;
        if (!name) { m.setError("브랜치 이름을 입력하세요."); return; }
        if (/[\s~^:?*[\\]/.test(name)) {
          m.setError("브랜치 이름에 공백이나 특수문자(~^:?*[\\])는 쓸 수 없습니다.");
          return;
        }
        m.setSubmitting(true);
        m.setError(null);
        try {
          await ipc.createBranch(repoId, name);
          if (pushAfter) {
            const outcome = await openPushCredentialFlow(repo, name);
            if (outcome !== "ok" && outcome !== "cancelled") {
              toast(`push 실패: ${outcome.message || "알 수 없는 오류"}`, "error");
            }
          }
          await ipc.updateRepository(repoId, { working_branch: name });
          toast(`브랜치 '${name}' 생성 완료${pushAfter ? " — 원격에 푸시됨" : ""}`, "success");
          // 순서가 중요하다 — 상태(현재 브랜치)를 먼저 반영한 뒤 목록을 다시
          // 그려야 선택 상자가 새 브랜치를 가리킨다. 반대로 하면 select가
          // 이전 브랜치를 고른 채 남고, 관리자 배지가 이전 브랜치 기준으로
          // 푸시를 잠가 버린다 (main 잠금 때문에 팀원의 푸시가 막히는 식으로
          // 실제로 깨진 적이 있다).
          applyStatus(await ipc.status(repoId).catch(() => null));
          await loadBranches();
          close();
          projectCfg = await ipc.projectConfigGet(repoId).catch(() => null);
          refreshManagerBadge();
          refreshSyncLabel();
        } catch (e) {
          m.setError(`생성 실패: ${(e as Error).message ?? e}`);
          m.setSubmitting(false);
        }
      },
    });
    m.body.innerHTML = `
      <div class="flex flex-col gap-1">
        <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="nb-name">브랜치 이름 <span class="text-[color:var(--color-danger)]">*</span></label>
        <input id="nb-name" class="gc-input font-mono" type="text" placeholder="예: feature/데이터정제" spellcheck="false" autocomplete="off" autocapitalize="off" />
      </div>
      <label class="flex items-center gap-2 text-display-sm cursor-pointer">
        <input type="checkbox" id="nb-push" />
        <span>생성 후 원격에 push (팀원에게 알림)</span>
      </label>
    `;
    m.body.querySelector<HTMLInputElement>("#nb-name")!.focus();
  });

  // ── Status table ─────────────────────────────────────────────────────────
  const table = document.createElement("div");
  table.className = "gc-card overflow-x-auto";
  main.appendChild(table);

  // ── Action row — 커밋·푸시·풀·스태시. ↑↓는 위의 상태 칩이 말하므로
  // 헤더·중복 표시는 두지 않고, "다음" 표식으로 지금 누를 곳을 가리킨다.
  // 카드로 감싸지 않고 프로젝트명 행 우측에 앉는다 — 버튼은 이미 유약 두께감의
  // 독립된 대상이고, 마우스로 누르는 데스크톱 화면에서 버튼은 버튼 크기여야
  // 읽힌다. 페이지 중간의 도구 행보다 헤더 액션이 데스크톱 관습이다.
  const commitCard = document.createElement("div");
  commitCard.className = "flex flex-wrap items-center gap-2 ml-auto";
  commitCard.innerHTML = `
    <button id="btn-commit" class="gc-action-btn"></button>
    <button id="btn-push" class="gc-action-btn"></button>
    <button id="btn-pull" class="gc-action-btn"></button>
    <button id="btn-stash" class="gc-action-btn"></button>
  `;
  headRow.appendChild(commitCard);

  // ── 병합 요청 카드 — push와 승인을 잇는 명시적 단계 ─────────────────────
  //
  // 팀원의 흐름: 커밋 → 푸시 → (여기서) 병합 요청 → 관리자 승인.
  // 푸시만으로는 관리자의 승인 대기열에 오르지 않는다 — 푸시는 작업 공유이고,
  // 요청은 "이제 병합해 주세요"라는 신호다.
  const requestCard = document.createElement("div");
  requestCard.className = "gc-card flex flex-col gap-2";
  requestCard.style.display = "none";
  main.appendChild(requestCard);

  // Fill button icons + labels (avoid HTML-entity parsing pitfalls for innerHTML).
  // 각 버튼은 [아이콘 + 라벨 + 다음 태그] — "다음" 태그는 평소 숨겨 두고
  // paintNextAction이 지금 누를 버튼에만 세운다.
  function buildActionBtn(el: HTMLButtonElement, name: Parameters<typeof icon>[0], label: string) {
    el.dataset.action = label;
    // 15px — 16px 라벨과 나란히 둘 때 아이콘이 살짝 가벼워야 균형이 맞다.
    el.appendChild(icon(name, 15));
    const row = document.createElement("span");
    row.className = "gc-action-btn__row";
    const s = document.createElement("span");
    s.textContent = label;
    row.appendChild(s);
    const next = document.createElement("span");
    next.className = "gc-next-tag";
    next.textContent = "다음";
    next.hidden = true;
    row.appendChild(next);
    el.appendChild(row);
  }
  buildActionBtn(commitCard.querySelector<HTMLButtonElement>("#btn-commit")!, "commit", "커밋");
  buildActionBtn(commitCard.querySelector<HTMLButtonElement>("#btn-push")!, "push", "푸시");
  buildActionBtn(commitCard.querySelector<HTMLButtonElement>("#btn-pull")!, "pull", "풀");
  buildActionBtn(commitCard.querySelector<HTMLButtonElement>("#btn-stash")!, "stash", "스태시");

  // 배너·버튼 한 줄용 — 아이콘 + 라벨 (다음 태그 없음).
  function fillBtn(el: HTMLButtonElement, name: Parameters<typeof icon>[0], label: string) {
    el.appendChild(icon(name, 16));
    const s = document.createElement("span");
    s.textContent = label;
    el.appendChild(s);
  }

  // Conflict banner — shown when the most recent pull produced conflicts.
  let conflictBanner: HTMLDivElement | null = null;
  function showConflictBanner(paths: string[]) {
    hideConflictBanner();
    const banner = document.createElement("div");
    banner.className = "gc-banner gc-banner--danger";
    const iconWrap = document.createElement("span");
    iconWrap.className = "gc-banner__icon";
    iconWrap.appendChild(icon("warn", 20));
    banner.appendChild(iconWrap);
    const body = document.createElement("div");
    body.className = "gc-banner__body flex-1";
    const title = document.createElement("div");
    title.className = "gc-banner__title";
    title.textContent = `풀 충돌 ${paths.length}개`;
    body.appendChild(title);
    const sub = document.createElement("div");
    sub.className = "text-display-sm text-[color:var(--color-ink-muted)]";
    sub.textContent = "병합 탭에서 해결하세요.";
    body.appendChild(sub);
    banner.appendChild(body);
    const gotoBtn = document.createElement("button");
    gotoBtn.className = "gc-button-secondary";
    fillBtn(gotoBtn, "arrow-right", "병합 탭으로");
    gotoBtn.addEventListener("click", () => onTab?.("merge"));
    banner.appendChild(gotoBtn);
    conflictBanner = banner;
    // 프로젝트명 행 바로 아래 — 충돌은 흐름을 막는 상태이므로 최상단에서 마주쳐야 한다.
    headRow.after(banner);
  }
  function hideConflictBanner() {
    if (conflictBanner) {
      conflictBanner.remove();
      conflictBanner = null;
    }
  }

  // ── Load data ─────────────────────────────────────────────────────────────
  let currentStatus: WorkingTreeStatus | null = await ipc.status(repoId).catch(() => null);
  let statusJson = JSON.stringify(currentStatus);
  /** Paths the user has checked for staging — survives table re-renders. */
  const selected = new Set<string>();

  function applyStatus(s: WorkingTreeStatus | null) {
    currentStatus = s;
    statusJson = JSON.stringify(s);
    renderStatusTable();
    // 브랜치·ahead가 바뀌면 병합 요청 카드도 다시 그린다 (시그니처가 같으면
    // 조용히 건너뛴다 — 6초 폴링이 원격 조회를 반복하지 않게).
    void refreshRequestCard();
  }

  // Lightweight polling — teammates' commits/pushes surface without
  // a manual refresh. Skipped while a modal is open or an input is focused.
  let polling = false;
  const pollTimer = window.setInterval(async () => {
    // 화면에서 떨어진 뒤에도 인터벌이 앱 수명 내내 쌓이지 않게 스스로 정리한다.
    if (!main.isConnected) {
      window.clearInterval(pollTimer);
      return;
    }
    if (polling) return;
    if (document.querySelector("dialog[open]")) return;
    const active = document.activeElement;
    if (active && (active.tagName === "TEXTAREA" || active.tagName === "INPUT")) return;
    polling = true;
    try {
      const next = await ipc.status(repoId).catch(() => null);
      if (next && JSON.stringify(next) !== statusJson) applyStatus(next);
    } finally {
      polling = false;
    }
  }, 6000);

  // ── Diff 미리보기 — 변경 내용을 색깔 있는 라인으로 보여준다 ──────────
  function openFileDiff(path: string, staged: boolean, unstaged: boolean, kind: string) {
    const isUntracked = kind === "untracked";
    // 스테이지+작업 트리 둘 다 바뀐 파일은 다음 커밋에 들어갈 작업 트리 diff를 먼저 보여준다.
    const showStaged = staged && !unstaged;
    const desc = isUntracked
      ? "새 파일 — 커밋 시 포함됩니다"
      : showStaged
        ? "스테이징된 변경 내용"
        : "작업 트리의 변경 내용";
    const m = openModal({
      title: path,
      description: desc,
      cancelLabel: "닫기",
    });
    const host = document.createElement("div");
    host.className = "flex flex-col gap-0 rounded-md border border-[color:var(--color-hairline)] overflow-x-auto";
    host.innerHTML = `<div class="text-display-sm text-[color:var(--color-ink-muted)] px-3 py-2">불러오는 중…</div>`;
    m.body.appendChild(host);
    const render = (text: string) => {
      host.innerHTML = "";
      if (!text || !text.trim()) {
        host.innerHTML = `<div class="text-display-sm text-[color:var(--color-ink-muted)] px-3 py-2">${isUntracked ? "새 파일이라 diff가 없습니다. 커밋에 포함됩니다." : "변경 내용이 없습니다"}</div>`;
        return;
      }
      const nav = document.createElement("div");
      nav.className = "flex items-center justify-between px-3 py-1.5 border-b border-[color:var(--color-hairline)] text-display-xs text-[color:var(--color-ink-muted)] font-mono";
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
    };
    const fetchDiff = (s: boolean) =>
      ipc.diff(repoId, path, s, false).then((text) => {
        // 상태 플래그와 실제 diff가 어긋나면(비어 있으면) 다른 쪽으로 대체.
        if ((!text || !text.trim()) && s !== showStaged) render("");
        else render(text);
      });
    fetchDiff(showStaged)
      .catch(() => ipc.diff(repoId, path, !showStaged, false))
      .catch((e) => {
        host.innerHTML = `<div class="text-display-sm text-[color:var(--color-danger)] px-3 py-2">diff 불러오기 실패: ${escape(String((e as Error).message ?? e))}</div>`;
      });
  }

  /** 커밋이 아직 없는 저장소인지 — 그때는 브랜치 ref 가 하나도 없다. */
  let noCommitsYet = false;

  async function loadBranches() {
    const branches = await ipc.listBranches(repoId).catch(() => []);
    branchSel.innerHTML = "";
    // 방금 `git init` 한 저장소에는 브랜치 ref 가 없어서 목록이 통째로 비었다.
    // 선택 상자가 빈 채로 놓여 있으면 무엇이 잘못된 건지 알 수 없으므로,
    // status 가 알려 주는 (아직 만들어지지 않은) 브랜치 이름을 보여 준다.
    noCommitsYet = branches.length === 0;
    if (noCommitsYet) {
      const name = currentStatus?.branch || repo?.default_branch || "main";
      const opt = document.createElement("option");
      opt.value = name;
      opt.textContent = `${name} (커밋 없음)`;
      opt.selected = true;
      branchSel.appendChild(opt);
      branchSel.disabled = true;
      branchSel.title = "첫 커밋을 만들면 브랜치가 생깁니다.";
    } else {
      branchSel.disabled = false;
      branchSel.title = "";
      // 내 브랜치와 원격(origin/) 브랜치를 구분해서 보여 준다 — 원격 것은
      // 서버에 있는 브랜치라 팀원 것이 섞여 있어도 헷갈리지 않게 그룹으로 나눈다.
      const localGroup = document.createElement("optgroup");
      localGroup.label = "내 브랜치";
      const remoteGroup = document.createElement("optgroup");
      remoteGroup.label = "원격 브랜치 (origin · 서버)";
      remoteGroup.dataset.remote = "1";
      for (const b of branches) {
        const opt = document.createElement("option");
        opt.value = b.name;
        if (b.is_remote) {
          opt.textContent = b.name.replace(/^origin\//, "") + " (서버)";
          opt.title =
            "서버에 있는 브랜치입니다. 고르면 같은 이름의 내 브랜치로 전환됩니다.";
          remoteGroup.appendChild(opt);
        } else {
          const isCurrent = b.name === currentStatus?.branch;
          opt.textContent = b.name + (isCurrent ? " (현재)" : "");
          localGroup.appendChild(opt);
        }
      }
      branchSel.appendChild(localGroup);
      branchSel.appendChild(remoteGroup);
      // 실제로 체크아웃된 브랜치를 항상 고르게 한다 — 등록 직후나 전환 직후에도
      // 선택 상자가 현재 브랜치를 말하도록. 로컬에 없으면 origin/ 항목을 고른다.
      const prefer = currentStatus?.branch || repo?.working_branch;
      if (prefer) {
        let picked = false;
        for (const opt of Array.from(branchSel.options)) {
          if (opt.value === prefer) {
            opt.selected = true;
            picked = true;
            break;
          }
        }
        if (!picked) {
          for (const opt of Array.from(branchSel.options)) {
            if (opt.value === `origin/${prefer}`) {
              opt.selected = true;
              break;
            }
          }
        }
      }
      paintBranchHint(false);
    }
    renderFirstStepBanner();
  }

  // ── 첫걸음 안내 ──────────────────────────────────────────────────────────
  //
  // 저장소를 막 만든 사람은 화면에 무엇을 해야 하는지가 없으면 멈춘다.
  // 커밋이 없을 때 / 원격이 없을 때 각각 다음 한 걸음을 알려 준다.
  const firstStep = document.createElement("div");
  firstStep.className = "gc-banner gc-banner--info";
  firstStep.style.display = "none";
  main.insertBefore(firstStep, table);

  function renderFirstStepBanner() {
    const noRemote = !repo?.remote_url;
    if (!noCommitsYet && !noRemote) {
      firstStep.style.display = "none";
      return;
    }
    firstStep.style.display = "";
    firstStep.innerHTML = "";
    const iw = document.createElement("span");
    iw.className = "gc-banner__icon";
    iw.appendChild(icon("info", 20));
    firstStep.appendChild(iw);
    const body = document.createElement("div");
    body.className = "gc-banner__body flex-1 flex flex-col gap-0.5";
    const title = document.createElement("div");
    title.className = "gc-banner__title";
    const desc = document.createElement("div");
    desc.className = "text-display-sm whitespace-pre-line";
    if (noCommitsYet) {
      title.textContent = "아직 커밋이 없습니다";
      desc.textContent =
        "아래 목록에서 파일을 고르고 ‘커밋’을 누르면 첫 커밋이 만들어집니다. 그때 브랜치도 함께 생깁니다.";
    } else {
      title.textContent = "이 저장소에는 원격(origin)이 없습니다";
      desc.textContent =
        "커밋은 이 컴퓨터에 저장되지만, 팀원과 주고받으려면 원격이 필요합니다.\n터미널에서 한 번 등록하세요:  git remote add origin <저장소 주소>";
    }
    body.appendChild(title);
    body.appendChild(desc);
    firstStep.appendChild(body);
  }

  /** 상태 → 액션 바의 "다음" 표식. 홈 카드의 다음 할 일과 같은 순서
   *  (커밋 → 푸시 → 받기)이되, 막혀 있는 버튼은 건너뛴다 — 다음 행동을
   *  가리키면서 누를 수 없으면 거짓말이 된다. 관리자 잠금·원격 없음으로
   *  버튼이 나중에 막히는 경우를 위해 잠금 갱신 쪽에서도 다시 그린다. */
  function paintNextAction() {
    const fileCount = currentStatus?.files.length ?? 0;
    const ahead = currentStatus?.ahead ?? 0;
    const behind = currentStatus?.behind ?? 0;
    const candidates: (HTMLButtonElement | null)[] = [];
    if (fileCount > 0) candidates.push(commitCard.querySelector<HTMLButtonElement>("#btn-commit"));
    if (ahead > 0) candidates.push(commitCard.querySelector<HTMLButtonElement>("#btn-push"));
    if (behind > 0) candidates.push(commitCard.querySelector<HTMLButtonElement>("#btn-pull"));
    let picked: HTMLButtonElement | null = null;
    for (const c of candidates) {
      if (c && !c.disabled) { picked = c; break; }
    }
    for (const btn of commitCard.querySelectorAll<HTMLButtonElement>(".gc-action-btn")) {
      const tag = btn.querySelector<HTMLElement>(".gc-next-tag");
      const isNext = btn === picked;
      btn.classList.toggle("is-next", isNext);
      if (tag) tag.hidden = !isNext;
      if (isNext) btn.setAttribute("aria-label", `다음 할 일: ${btn.dataset.action}`);
      else btn.removeAttribute("aria-label");
    }
  }

  /** 상태 한 줄 스트립 — "변경 사항 없음"을 온 카드로 말하는 대신 조용한
   *  한 줄로. 상태는 색만이 아니라 표식으로도 읽히게 (Pattern-Carry Rule). */
  function paintCleanStrip(kind: "clean" | "error", text: string, sub?: string) {
    table.className = "gc-clean";
    table.innerHTML = "";
    const ic = document.createElement("span");
    ic.className = "gc-clean__icon";
    if (kind === "error") ic.style.color = "var(--color-danger)";
    ic.appendChild(icon(kind === "clean" ? "check" : "warn", 14));
    table.appendChild(ic);
    const t = document.createElement("span");
    t.className = "gc-clean__text";
    t.textContent = text;
    table.appendChild(t);
    if (sub) {
      const s = document.createElement("span");
      s.className = "gc-clean__sub";
      s.textContent = sub;
      table.appendChild(s);
    }
  }

  function renderStatusTable() {
    if (!currentStatus) {
      paintCleanStrip("error", "상태를 불러올 수 없습니다");
      statusPill.textContent = "?";
      paintNextAction();
      return;
    }
    const { ahead, behind, files } = currentStatus;
    // 예전에는 `" 1개 파일"` 처럼 앞에 빈 칸이 남았고, 변경이 없을 때도
    // "0개 파일" 이라고 했다. 상태 한 줄은 그 자체로 읽혀야 한다.
    const parts: string[] = [];
    if (ahead > 0) parts.push(`↑${ahead}`);
    if (behind > 0) parts.push(`↓${behind}`);
    parts.push(files.length === 0 ? "변경 없음" : `변경 ${files.length}개`);
    // 완전히 깨끗한 상태는 셀라돈 체크로도 읽히게 — 색만이 아니라 표식.
    statusPill.innerHTML = "";
    if (ahead === 0 && behind === 0 && files.length === 0) {
      const ok = document.createElement("span");
      ok.className = "inline-flex items-center self-center text-[color:var(--color-success)]";
      ok.appendChild(icon("check", 12));
      statusPill.appendChild(ok);
    }
    const pillText = document.createElement("span");
    pillText.textContent = parts.join(" ");
    statusPill.appendChild(pillText);
    statusPill.title =
      (ahead > 0 ? `푸시하지 않은 커밋 ${ahead}개. ` : "") +
      (behind > 0 ? `아직 받지 않은 커밋 ${behind}개. ` : "") +
      (files.length === 0 ? "커밋할 변경이 없습니다." : `커밋하지 않은 파일 ${files.length}개.`);
    // 커밋할 것이 없을 때 '커밋'을 누르면 메시지를 다 쓴 뒤에야 실패한다.
    // 눌리지 않게 하고, 무엇을 하면 눌리는지 툴팁에 적는다.
    const commitBtnEl = commitCard.querySelector<HTMLButtonElement>("#btn-commit")!;
    commitBtnEl.disabled = files.length === 0;
    commitBtnEl.title = files.length === 0
      ? "커밋할 변경이 없습니다. 파일을 고치면 아래 목록에 나타납니다."
      : "";
    if (files.length === 0) {
      paintCleanStrip("clean", "커밋할 변경이 없습니다", "파일을 고치면 여기에 나타납니다");
      paintNextAction();
      return;
    }
    table.className = "gc-card overflow-x-auto";
    // 라벨/설명은 StatusTable 의 것을 쓴다. 예전에는 같은 표가 여기에도
    // 복사돼 있어서, 한쪽만 고치면 화면은 그대로였다.
    const rows = files.map((f) => `
      <tr>
        <td class="px-3 py-2"><input type="checkbox" data-path="${escape(f.path)}" aria-label="${escape(f.path)}" /></td>
        <td class="px-3 py-2 text-display-sm font-medium" title="${escape(kindHint(f.kind))}">${escape(kindLabel(f.kind))}</td>
        <td class="px-3 py-2 text-display-sm">${escape(f.path)}</td>
        <td class="px-3 py-2 text-right">
          <button class="gc-button-secondary text-display-sm" data-diff="${escape(f.path)}" data-staged="${f.staged ? "1" : "0"}" data-unstaged="${f.unstaged ? "1" : "0"}" data-kind="${escape(f.kind)}">변경 내용</button>
        </td>
      </tr>
    `).join("");
    table.innerHTML = `
      <table class="w-full text-left">
        <thead>
          <tr class="border-b border-[color:var(--color-hairline)]">
            <th class="px-3 py-2 text-display-sm text-[color:var(--color-ink-muted)] w-8">
              <input id="status-select-all" type="checkbox" aria-label="모두 선택" />
            </th>
            <th class="px-3 py-2 text-display-sm text-[color:var(--color-ink-muted)]">상태</th>
            <th class="px-3 py-2 text-display-sm text-[color:var(--color-ink-muted)]">파일</th>
            <th class="px-3 py-2 text-display-sm text-[color:var(--color-ink-muted)] w-28"></th>
          </tr>
        </thead>
        <tbody>${rows}</tbody>
      </table>
    `;
    // 파일별 diff 미리보기 — 커밋 전에 무엇이 바뀌는지 확인한다.
    for (const btn of table.querySelectorAll<HTMLButtonElement>("button[data-diff]")) {
      btn.addEventListener("click", (e) => {
        e.stopPropagation();
        openFileDiff(
          btn.dataset.diff!,
          btn.dataset.staged === "1",
          btn.dataset.unstaged === "1",
          btn.dataset.kind ?? "modified",
        );
      });
    }
    // Restore + track checkbox selection so polling re-renders never lose it.
    const boxes = table.querySelectorAll<HTMLInputElement>("tbody input[type=checkbox]");
    boxes.forEach((cb) => {
      const p = cb.dataset.path!;
      cb.checked = selected.has(p);
      cb.addEventListener("change", () => {
        if (cb.checked) selected.add(p);
        else selected.delete(p);
      });
    });
    // Wire the select-all checkbox to every row's checkbox.
    const selectAll = table.querySelector<HTMLInputElement>("#status-select-all");
    if (selectAll) {
      selectAll.checked = boxes.length > 0 && Array.from(boxes).every((cb) => cb.checked);
      selectAll.addEventListener("change", () => {
        boxes.forEach((cb) => {
          cb.checked = selectAll.checked;
          const p = cb.dataset.path!;
          if (cb.checked) selected.add(p);
          else selected.delete(p);
        });
      });
    }
    paintNextAction();
  }

  renderStatusTable();
  await loadBranches();

  // ── 병합 관리자 (프로젝트 설정 .gpconfig) ────────────────────────────────
  // 브랜치별 관리자를 표시하고, 명시된 관리자가 아닌 로그인 사용자의 푸시를 잠근다.
  // ── 병합 요청 (푸시와 승인을 잇는 명시적 단계) ───────────────
  //
  // 현재 브랜치가 병합 대상이 아니고 푸시된 상태면, 관리자에게 승인을 요청하는
  // 카드를 보여 준다. 이미 열린 요청이 있으면 그 상태(갱신/취소)를 보여 준다.
  let mrSig = "";
  let mrBusy = false;

  function relativeTimeShort(unix: number): string {
    const diff = Math.max(0, Date.now() / 1000 - unix);
    if (diff < 60) return "방금";
    if (diff < 3600) return `${Math.floor(diff / 60)}분 전`;
    if (diff < 86400) return `${Math.floor(diff / 3600)}시간 전`;
    return `${Math.floor(diff / 86400)}일 전`;
  }

  function mergeBaseNow(): string {
    return projectCfg?.config?.default_base_branch || repo?.default_branch || "main";
  }
  function isCurrentBranchMergeTarget(): boolean {
    const branch = currentStatus?.branch;
    if (!branch) return false;
    const fallback = mergeBaseNow();
    const targets = projectCfg?.config?.merge_targets?.length
      ? projectCfg.config.merge_targets
      : [fallback];
    return targets.includes(branch) || branch === fallback;
  }

  async function refreshRequestCard(force = false) {
    if (mrBusy) return;
    const branch = currentStatus?.branch || null;
    const base = mergeBaseNow();
    const ahead = currentStatus?.ahead ?? 0;
    const sig = `${branch}|${ahead}|${base}`;
    if (!force && sig === mrSig) return;
    mrSig = sig;
    mrBusy = true;
    try {
      requestCard.innerHTML = "";
      const hide = () => { requestCard.style.display = "none"; };
      if (!branch || isCurrentBranchMergeTarget() || noRemote) {
        hide();
        return;
      }
      requestCard.style.display = "";

      // 세 상태(푸시 필요 · 요청 가능 · 요청됨)가 같은 카드 모양을 공유한다:
      // [아이콘 + 병합 요청 + 상태 배지] → 한 줄 설명 → 행동.
      const head = document.createElement("div");
      head.className = "flex items-center gap-2 flex-wrap";
      const ic = document.createElement("span");
      ic.className = "text-[color:var(--color-ink-muted)] inline-flex";
      ic.appendChild(icon("merge", 15));
      head.appendChild(ic);
      const title = document.createElement("div");
      title.className = "font-medium";
      title.textContent = "병합 요청";
      head.appendChild(title);
      requestCard.appendChild(head);

      const badge = (text: string, cls: string) => {
        const b = document.createElement("span");
        b.className = `gc-badge ${cls}`;
        b.textContent = text;
        head.appendChild(b);
      };

      // 아직 푸시하지 않은 커밋이 있으면 요청할 수 없다 — 요청은 항상
      // 푸시된 커밋을 대상으로 한다 (어느 컴퓨터에서 봐도 같아야 하므로).
      if (ahead > 0) {
        badge("푸시 필요", "gc-badge--warning");
        const note = document.createElement("div");
        note.className = "text-display-sm text-[color:var(--color-ink-muted)]";
        note.textContent = `푸시하지 않은 커밋 ${ahead}개가 있습니다. 푸시로 원격에 올린 뒤 ${base} 관리자에게 승인을 요청할 수 있습니다.`;
        requestCard.appendChild(note);
        return;
      }
      // 푸시된 상태 — 이 브랜치의 열린 요청이 있는지 본다.
      const open = await ipc
        .listRequestedMerges(repoId, base)
        .then((list) => list.find((r) => r.request.branch === branch) ?? null)
        .catch(() => null);
      if (!open) {
        const desc = document.createElement("div");
        desc.className = "text-display-sm text-[color:var(--color-ink-muted)]";
        desc.textContent = `${branch}의 커밋이 원격에 올라와 있습니다. 승인을 요청하면 ${base} 병합 관리자의 대기열에 오르고, 승인·push되면 팀원 전원에게 동기화 알림이 갑니다.`;
        requestCard.appendChild(desc);
        const reqBtn = document.createElement("button");
        reqBtn.className = "gc-button-primary self-start";
        reqBtn.textContent = "병합 요청 보내기";
        reqBtn.addEventListener("click", () => openMergeRequestModal(branch, base, reqBtn));
        requestCard.appendChild(reqBtn);
        return;
      }
      // 이미 요청됨 — 관리자의 승인을 기다리는 중.
      badge("요청됨 · 관리자 승인 대기", "gc-badge--info");
      const meta = document.createElement("div");
      meta.className = "text-display-sm text-[color:var(--color-ink-muted)] min-w-0 truncate";
      meta.textContent = `${base} ← ${open.request.branch} · ${relativeTimeShort(open.request.created_at)} · ${open.request.title}`;
      meta.title = open.request.local_only
        ? "요청을 원격에 공유하지 못했습니다 (네트워크·권한). 갱신으로 다시 시도하세요."
        : open.request.title;
      requestCard.appendChild(meta);
      const btnRow = document.createElement("div");
      btnRow.className = "flex gap-2";
      const renewBtn = document.createElement("button");
      renewBtn.className = "gc-button-secondary";
      renewBtn.textContent = "요청 갱신";
      renewBtn.title = "푸시를 더 했거나 제목을 바꿀 때 — 요청이 최신 push된 커밋을 가리키게 합니다.";
      renewBtn.addEventListener("click", () => openMergeRequestModal(branch, base, renewBtn, true));
      btnRow.appendChild(renewBtn);
      const cancelBtn = document.createElement("button");
      cancelBtn.className = "gc-button-secondary";
      cancelBtn.textContent = "요청 취소";
      cancelBtn.addEventListener("click", async () => {
        const ok = await confirmDialog({
          title: "병합 요청 취소",
          message: `${base} 병합 요청을 대기열에서 내립니다. 브랜치와 커밋은 그대로 남습니다.`,
          confirmLabel: "취소",
          destructive: true,
        });
        if (!ok) return;
        setBusy(cancelBtn, true, "정리 중…");
        try {
          const r = await closeMergeRequestWithAuth(repo, base, branch, "withdrawn");
          if (r.status === "ok") {
            toast("병합 요청을 취소했습니다.", "success");
            mrSig = "";
            await refreshRequestCard(true);
          } else if (r.status === "failed") {
            toast(
              `취소 실패: ${r.message} — 원격 요청이 남아 있으면 관리자 대기열에 계속 보입니다. 다시 시도하세요.`,
              "error",
            );
          }
          // "cancelled" — 로그인을 닫은 경우. 아무 일도 하지 않는다.
        } catch (e) {
          toast(`취소 실패: ${(e as Error).message ?? e}`, "error");
        } finally {
          setBusy(cancelBtn, false);
        }
      });
      btnRow.appendChild(cancelBtn);
      requestCard.appendChild(btnRow);
    } finally {
      mrBusy = false;
    }
  }

  /** 병합 요청 보내기/갱신 — 제목을 받는 모달. 갱신이면 확인 문구가 다르다. */
  function openMergeRequestModal(branch: string, base: string, trigger: HTMLButtonElement, renew = false) {
    // renderRepoView 상단 가드에서 이미 존재가 확인된 저장소다 (const + hoisted
    // 함수라 타입 추론이 좁히지 못하므로 명시적으로 단언).
    const r = repo!;
    const m = openModal({
      title: renew ? "병합 요청 갱신" : "병합 요청 보내기",
      description: renew
        ? `요청이 최신 push된 커밋을 가리키게 합니다. ${base} 병합 관리자의 승인을 기다립니다.`
        : `${base} 병합 관리자에게 ${branch}의 push된 커밋을 승인 요청합니다. 승인되면 ${base}에 병합되고 팀원에게 동기화 알림이 갑니다.`,
      submitLabel: renew ? "갱신" : "요청 보내기",
      onSubmit: async (close) => {
        const title = m.body.querySelector<HTMLInputElement>("#mr-title")!.value.trim();
        m.setSubmitting(true);
        m.setError(null);
        try {
          const r2 = await requestMergeWithAuth(r, base, branch, title || null);
          if (r2.status === "ok") {
            toast(
              renew
                ? "병합 요청을 최신 커밋으로 갱신했습니다."
                : `병합 요청을 보냈습니다 — ${base} 관리자의 승인을 기다립니다.`,
              "success",
            );
            mrSig = "";
            void refreshRequestCard(true);
            close();
          } else if (r2.status === "cancelled") {
            m.setSubmitting(false);
          } else {
            m.setError(`요청 실패: ${r2.message}`);
            m.setSubmitting(false);
          }
        } catch (e) {
          m.setError(`요청 실패: ${(e as Error).message ?? e}`);
          m.setSubmitting(false);
        }
      },
    });
    m.body.innerHTML = `
      <div class="flex flex-col gap-1">
        <label class="text-display-sm font-medium" for="mr-title">요청 제목</label>
        <input id="mr-title" class="gc-input" placeholder="마지막 커밋 제목이 기본값입니다" />
        <div class="text-display-xs text-[color:var(--color-ink-muted)]">비워 두면 마지막 커밋 제목이 사용됩니다.</div>
      </div>
    `;
    const input = m.body.querySelector<HTMLInputElement>("#mr-title")!;
    // 마지막 커밋 제목을 기본값으로 채운다 (여러 줄이면 첫 줄만).
    ipc
      .listCommits(repoId, branch, 1)
      .then((cs) => {
        const first = cs[0]?.message?.split("\n")[0]?.trim();
        if (first && !input.value) input.value = first;
      })
      .catch(() => undefined);
    input.focus();
    void trigger;
  }

  projectCfg = await ipc.projectConfigGet(repoId).catch(() => null);
  refreshSyncLabel();
  const pushBtnRef = () => commitCard.querySelector<HTMLButtonElement>("#btn-push")!;
  /** 원격이 없으면 푸시·풀·동기화는 무엇을 해도 실패한다 — 아래 두 곳이 함께 본다. */
  const noRemote = !repo?.remote_url;
  const noRemoteWhy =
    "이 저장소에는 원격(origin)이 없어 주고받을 곳이 없습니다.\n" +
    "터미널에서 등록하세요:  git remote add origin <저장소 주소>";

  function refreshManagerBadge() {
    const branch = branchSel.value.replace(/^origin\//, "");
    const managers = mergeManagerEmails(projectCfg, branch);
    if (managers.length === 0) {
      managerBadge.style.display = "none";
      // 관리자 잠금이 없다고 해서 푸시를 무조건 열면 안 된다 — 원격이 없는
      // 저장소에서 브랜치를 전환하거나 로그인/로그아웃할 때마다 여기가
      // 불려서, 눌러 보면 실패하는 버튼이 되살아났다.
      pushBtnRef().disabled = noRemote;
      pushBtnRef().title = noRemote ? noRemoteWhy : "";
      return;
    }
    const names = managers.map((email) => {
      const member = projectCfg?.config?.members.find((x) => x.email.toLowerCase() === email);
      return member?.name ?? email;
    });
    const me = getSession();
    const meEmail = me?.email.toLowerCase() ?? "";
    const isAdmin = me
      ? (projectCfg?.config?.members ?? []).some(
          (x) => x.email.toLowerCase() === meEmail && x.role === "admin",
        )
      : false;
    const isManager = !!me && managers.includes(meEmail);
    managerBadge.style.display = "";
    managerBadge.textContent = `병합 관리자: ${names.join(", ")}${isManager ? " (나)" : ""}`;
    // 명시된 관리자가 있으면 관리자(또는 admin)만 푸시할 수 있다. 로그아웃
    // 상태도 잠근다 — 익명이 로그인한 팀원보다 많은 권한을 가지면 안 된다.
    const blocked = !isManager && !isAdmin;
    const btn = pushBtnRef();
    btn.disabled = blocked || noRemote;
    btn.title = blocked
      ? me
        ? `${names.join(", ")}님이 이 브랜치의 병합 관리자입니다. 푸시는 관리자만 할 수 있습니다.`
        : `이 브랜치에는 병합 관리자(${names.join(", ")})가 지정되어 있습니다. 로그인하면 내가 관리자인지 확인해 푸시를 엽니다.`
      : noRemote
        ? noRemoteWhy
        : "";
    paintNextAction();
  }

  window.addEventListener("gc-account-changed", refreshManagerBadge);
  refreshManagerBadge();

  // 첫 그림 — 푸시된 상태면 병합 요청 카드를 바로 보여 준다.
  void refreshRequestCard(true);

  // ── 누르면 반드시 실패하는 버튼은 막아 둔다 ──────────────────────────────
  //
  // 원격(origin)이 없는 저장소에서 푸시·풀·동기화는 예외 없이 실패한다.
  // 메시지를 친절하게 바꾸는 것만으로는 부족하다 — 처음 쓰는 사람에게
  // "눌러 보면 실패하는 버튼"은 자기가 뭘 잘못한 줄 알게 만든다.
  // 눌리지 않게 하고, 왜 그런지와 무엇을 하면 되는지 툴팁에 남긴다.
  function refreshRemoteDependentButtons() {
    if (!noRemote) return;
    const why = noRemoteWhy;
    const targets: (HTMLButtonElement | null)[] = [
      commitCard.querySelector<HTMLButtonElement>("#btn-push"),
      commitCard.querySelector<HTMLButtonElement>("#btn-pull"),
      syncBtn,
    ];
    for (const b of targets) {
      if (!b) continue;
      b.disabled = true;
      b.title = why;
    }
    paintNextAction();
  }
  refreshRemoteDependentButtons();

  // ── Branch change ─────────────────────────────────────────────────────────
  branchSel.addEventListener("change", async () => {
    // 원격 트래킹 항목(origin/…)을 선택한 경우 로컬 브랜치 이름으로 정규화해 전환한다.
    const branch = branchSel.value.replace(/^origin\//, "");
    paintBranchHint(branchSel.value.startsWith("origin/"));
    branchSel.disabled = true;
    setBusy(statusPill, true, "전환 중…");
    try {
      await ipc.checkoutBranch(repoId, branch);
      await ipc.updateRepository(repoId, { working_branch: branch });
      toast("브랜치 전환 완료", "success");
      applyStatus(await ipc.status(repoId).catch(() => null));
      projectCfg = await ipc.projectConfigGet(repoId).catch(() => null);
      refreshManagerBadge();
      refreshSyncLabel();
      // 전환 후에는 선택 상자도 로컬 브랜치 이름을 보여야 한다 (origin/ 항목을
      // 골랐어도 실제로는 같은 이름의 내 브랜치가 생겨 있다).
      const fresh = await ipc.listBranches(repoId).catch(() => null);
      if (fresh && fresh.length > 0) await loadBranches();
    } catch (e) {
      toast(`브랜치 전환 실패: ${(e as Error).message ?? e}`, "error");
      // 전환에 실패했는데 선택 상자가 새 브랜치를 가리키고 있으면, 다음
      // 푸시·동기화가 엉뚱한 브랜치를 대상으로 잡는다 — 실제 HEAD로 되돌린다.
      const actual = currentStatus?.branch || repo.working_branch;
      if (actual) branchSel.value = actual;
    } finally {
      branchSel.disabled = false;
      setBusy(statusPill, false);
      // 전환 실패로 선택이 되돌아갔을 수도 있다 — 실제 선택값으로 힌트를 맞춘다.
      paintBranchHint(branchSel.value.startsWith("origin/"));
    }
  });
  // ── Commit modal ─────────────────────────────────────────────────────────
  commitCard.querySelector<HTMLButtonElement>("#btn-commit")!.addEventListener("click", () => {
    const m = openModal({
      title: "커밋 메시지 작성",
      description: "무엇을 왜 바꿨는지 한 줄로 적으면 나중에 팀원이 이 커밋을 찾을 때 도움이 됩니다.",
      submitLabel: "커밋",
      onSubmit: async (close) => {
        const msg = (m.body.querySelector<HTMLTextAreaElement>("#commit-msg")!).value.trim();
        if (!msg) { m.setError("커밋 메시지를 입력하세요."); return; }
        const stageAll = (m.body.querySelector<HTMLInputElement>("#stage-all")!).checked;
        const checkboxes = table.querySelectorAll<HTMLInputElement>("input[type=checkbox]:checked");
        const paths = Array.from(checkboxes).map((cb) => cb.dataset.path!).filter(Boolean);
        m.setSubmitting(true);
        m.setError(null);
        try {
          if (paths.length > 0) {
            await ipc.addFiles(repoId, paths);
          }
          // When stageAll is true and no paths given, git commit -a handles staging implicitly.
          const r = await ipc.commit(repoId, msg, stageAll);
          if (!r.ok) {
            // 백엔드가 실패 사유(사용자 정보 없음, index.lock 등)를 한국어로
            // 담아 보낸다 — 모달을 열어 둔 채 그대로 보여 준다.
            m.setError(`커밋 실패: ${r.message || "알 수 없는 오류"}`);
            return;
          }
          toast("커밋 완료", "success");
          applyStatus(await ipc.status(repoId).catch(() => null));
          close();
        } catch (e) {
          m.setError(`커밋 실패: ${(e as Error).message ?? e}`);
        } finally {
          m.setSubmitting(false);
        }
      },
    });

    m.body.innerHTML = `
      <div class="flex flex-col gap-1">
        <textarea id="commit-msg" class="gc-input min-h-[80px] resize-y" placeholder="커밋 메시지 입력..."></textarea>
      </div>
      <label class="flex items-center gap-2 text-display-sm cursor-pointer">
        <input type="checkbox" id="stage-all" />
        <span id="stage-all-label"></span>
      </label>
    `;
    // 목록에서 파일을 체크했다면 "전부 커밋"이 그 선택을 덮어쓰면 안 된다 —
    // 체크한 파일만 커밋이 기본이 되고, 전부 커밋은 직접 켜는 선택지로 남는다.
    const stageAllBox = m.body.querySelector<HTMLInputElement>("#stage-all")!;
    const stageAllLabel = m.body.querySelector<HTMLElement>("#stage-all-label")!;
    stageAllBox.checked = selected.size === 0;
    stageAllLabel.textContent =
      selected.size > 0
        ? `바뀐 파일 전부 커밋 (끄면 체크한 파일 ${selected.size}개만)`
        : "바뀐 파일 전부 커밋 (끄면 위 목록에서 체크한 파일만)";
  });

  // ── Push ─────────────────────────────────────────────────────────────────
  const pushBtn = commitCard.querySelector<HTMLButtonElement>("#btn-push")!;
  pushBtn.addEventListener("click", async () => {
    if (pushBtn.disabled) return;
    setBusy(pushBtn, true, "푸시 중…");
    try {
      // 푸시 대상은 화면의 선택 상자가 아니라 실제 체크아웃된 브랜치(HEAD)를
      // 따른다 — 전환 실패 직후에도 다른 팀원의 원격 브랜치로 밀리지 않는다.
      const currentBranch =
        currentStatus?.branch || branchSel.value.replace(/^origin\//, "") || null;
      const outcome = await openPushCredentialFlow(repo, currentBranch);
      if (outcome === "ok") {
        toast("푸시 완료", "success");
        // 푸시 직후가 병합 요청을 보낼 수 있는 순간이다 — 카드를 즉시 새로 그린다.
        mrSig = "";
        void refreshRequestCard(true);
      } else if (outcome === "cancelled") {
        toast("푸시를 취소했습니다.", "info");
      } else {
        toast(`푸시 실패: ${outcome.message || "알 수 없는 오류"}`, "error");
      }
      applyStatus(await ipc.status(repoId).catch(() => null));
    } catch (e) {
      toast(`푸시 실패: ${(e as Error).message ?? e}`, "error");
    } finally {
      setBusy(pushBtn, false);
    }
  });
  // ── Pull ─────────────────────────────────────────────────────────────────
  const pullBtn = commitCard.querySelector<HTMLButtonElement>("#btn-pull")!;
  pullBtn.addEventListener("click", async () => {
    const current = branchSel.value.replace(/^origin\//, "");
    const confirmed = await confirmDialog({
      title: "풀",
      message: `원격(origin)에 올라온 ${current} 브랜치의 새 커밋을 이 컴퓨터로 받아옵니다.\n같은 줄을 서로 고쳤다면 충돌이 날 수 있고, 그때는 병합 탭에서 해결합니다.`,
      confirmLabel: "받아오기",
    });
    if (!confirmed) return;
    setBusy(pullBtn, true, "풀 중…");
    try {
      const result = await ipc.pull(repoId);
      if (result.ok) {
        toast("풀 완료", "success");
      } else {
        toast(`풀 실패: ${result.message}`, "error");
      }
      if (result.conflicted_files.length > 0) {
        showConflictBanner(result.conflicted_files);
      } else {
        hideConflictBanner();
      }
      applyStatus(await ipc.status(repoId).catch(() => null));    } catch (e) {
      toast(`풀 실패: ${(e as Error).message ?? e}`, "error");
    } finally {
      setBusy(pullBtn, false);
    }
  });
  // ── Stash ─────────────────────────────────────────────────────────────────
  const stashBtn = commitCard.querySelector<HTMLButtonElement>("#btn-stash")!;
  stashBtn.addEventListener("click", async () => {
    const m = openModal({
      title: "스태시",
      hideFooter: true,
    });
    m.body.innerHTML = `
      <div class="flex flex-col gap-3">
        <div class="text-display-sm text-[color:var(--color-ink-muted)]">
          작업 트리 변경을 잠시 보관했다가 나중에 복원할 수 있습니다. 병합·전환 전 정리에 유용합니다.
        </div>
        <button id="stash-save" class="gc-button-primary self-start">변경 사항 스태시</button>
        <div class="text-display-md font-medium">저장된 스태시</div>
        <div id="stash-list" class="flex flex-col gap-1"></div>
      </div>
    `;
    const saveBtn = m.body.querySelector<HTMLButtonElement>("#stash-save")!;
    saveBtn.addEventListener("click", async () => {
      saveBtn.disabled = true;
      try {
        await ipc.stash(repoId, "save:임시 저장");
        toast("변경 사항을 스태시에 저장했습니다.", "success");
        applyStatus(await ipc.status(repoId).catch(() => null));
        await renderStashList();
      } catch (e) {
        toast(`스태시 저장 실패: ${(e as Error).message ?? e}`, "error");
      } finally {
        saveBtn.disabled = false;
      }
    });

    async function renderStashList() {
      const host = m.body.querySelector<HTMLElement>("#stash-list");
      if (!host) return;
      const entries = await ipc.stashList(repoId).catch(() => [] as StashEntry[]);
      host.innerHTML = "";
      if (entries.length === 0) {
        const empty = document.createElement("div");
        empty.className = "text-display-sm text-[color:var(--color-ink-muted)]";
        empty.textContent = "저장된 스태시가 없습니다.";
        host.appendChild(empty);
        return;
      }
      for (const e of entries) {
        const row = document.createElement("div");
        row.className = "flex items-center gap-2 text-display-sm border border-[color:var(--color-hairline)] rounded-md px-3 py-2";
        const idx = document.createElement("span");
        idx.className = "font-mono text-[color:var(--color-ink-muted)] shrink-0";
        idx.textContent = e.index;
        row.appendChild(idx);
        const sub = document.createElement("span");
        sub.className = "flex-1 min-w-0 truncate";
        sub.textContent = e.subject || "(메시지 없음)";
        sub.title = e.subject;
        row.appendChild(sub);
        const popBtn = document.createElement("button");
        popBtn.className = "gc-button-secondary text-display-sm";
        popBtn.textContent = "복원";
        popBtn.addEventListener("click", async () => {
          popBtn.disabled = true;
          try {
            await ipc.stash(repoId, `pop:${e.index}`);
            toast("스태시를 복원했습니다.", "success");
            applyStatus(await ipc.status(repoId).catch(() => null));
            await renderStashList();
          } catch (err) {
            toast(`복원 실패: ${(err as Error).message ?? err}`, "error");
          } finally {
            popBtn.disabled = false;
          }
        });
        row.appendChild(popBtn);
        const dropBtn = document.createElement("button");
        dropBtn.className = "gc-button-secondary text-display-sm text-[color:var(--color-danger)]";
        dropBtn.textContent = "삭제";
        dropBtn.addEventListener("click", async () => {
          const ok = await confirmDialog({
            title: "스태시 삭제",
            message: `${e.index} 항목을 삭제하시겠습니까? 복원할 수 없습니다.`,
            confirmLabel: "삭제",
            destructive: true,
          });
          if (!ok) return;
          dropBtn.disabled = true;
          try {
            await ipc.stash(repoId, `drop:${e.index}`);
            toast("스태시를 삭제했습니다.", "success");
            await renderStashList();
          } catch (err) {
            toast(`삭제 실패: ${(err as Error).message ?? err}`, "error");
          } finally {
            dropBtn.disabled = false;
          }
        });
        row.appendChild(dropBtn);
        host.appendChild(row);
      }
    }
    await renderStashList();
  });

  return main;
}

function escape(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}
