import { ipc, type StashEntry, type WorkingTreeStatus } from "../lib/ipc";
import { openModal, confirmDialog } from "../components/Modal";
import { toast } from "../components/Toast";
import { kindHint, kindLabel } from "../components/StatusTable";
import { renderMergeCenter } from "../components/MergeCenter";
import { renderProjectConfigPanel } from "../components/ProjectConfigPanel";
import { openPushCredentialFlow } from "../components/PushButton";
import { getSession } from "../lib/session";
import { icon } from "../components/Icon";
import { setBusy } from "../components/Busy";
import type { RepoTab } from "../components/Sidebar";
import { renderOpenWithButton } from "../components/OpenWithButton";
export async function renderRepoView(
  repoId: string,
  tab: RepoTab = "work",
  onTab?: (t: RepoTab) => void,
  onNavSettings?: () => void,
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

  // ── Header ────────────────────────────────────────────────────────────────
  const headRow = document.createElement("div");
  headRow.className = "flex items-start justify-between gap-4";
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
  // "이 저장소를 에디터/터미널로 열기" — 저장소를 보고 있을 때가 그 동작을
  // 원하는 순간이다. 도구가 없거나 SSH 저장소면 아무것도 붙지 않는다.
  // `main` 은 아직 document 에 붙지 않은 상태로 반환되므로 isConnected 로
  // 가드하면 안 된다 — 만들고 있는 트리에 그대로 붙인다.
  void renderOpenWithButton(repo, {
    onEditTools: () => onNavSettings?.(),
  }).then((btn) => {
    if (btn) headRow.appendChild(btn);
  });
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
    main.appendChild(await renderMergeCenter(repo, { onGoToWork: () => onTab?.("work") }));
    return main;
  }
  if (tab === "config") {
    main.appendChild(await renderProjectConfigPanel(repo));
    return main;
  }

  // ── Working branch + status row ───────────────────────────────────────────
  const meta = document.createElement("div");
  meta.className = "flex items-center gap-4";
  main.appendChild(meta);

  const branchSel = document.createElement("select");
  branchSel.className = "gc-input w-auto";
  // 아이콘도 라벨도 없는 선택 상자였다 — 스크린리더에도, 처음 보는 사람에게도
  // 이게 브랜치라는 정보가 없었다.
  branchSel.setAttribute("aria-label", "현재 작업 브랜치");

  const statusPill = document.createElement("span");
  statusPill.className = "gc-status-chip";

  // 병합 관리자 배지 — .gpconfig의 브랜치별 관리자 지정을 보여준다.
  const managerBadge = document.createElement("span");
  managerBadge.className = "gc-badge gc-badge--neutral";
  managerBadge.style.display = "none";

  meta.appendChild(branchSel);
  meta.appendChild(statusPill);
  meta.appendChild(managerBadge);

  // ── Sync — pull latest base into the current branch (step 3 of the flow) ─
  const syncBtn = document.createElement("button");
  syncBtn.className = "gc-button-secondary inline-flex items-center gap-1";
  syncBtn.appendChild(icon("arrow-right", 14));
  const syncLabel = document.createElement("span");
  syncLabel.textContent = "동기화";
  syncBtn.appendChild(syncLabel);
  syncBtn.addEventListener("click", async () => {
    const current = branchSel.value.replace(/^origin\//, "");
    const base = repo.default_branch || projectCfg?.config?.default_base_branch || "main";
    const confirmed = await confirmDialog({
      title: "동기화",
      message: `현재 브랜치(${current})에 origin/${base}의 최신 내용을 병합합니다.`,
      confirmLabel: "동기화",
    });
    if (!confirmed) return;
    setBusy(syncBtn, true, "동기화 중…");
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
      description: "현재 브랜치에서 작업 브랜치를 만들어 전환합니다. 팀원이 push하는 브랜치 이름과 겹치지 않게 정하세요.",
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
          await loadBranches();
          close();
          applyStatus(await ipc.status(repoId).catch(() => null));
          projectCfg = await ipc.projectConfigGet(repoId).catch(() => null);
          refreshManagerBadge();
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

  // ── Commit preview card ───────────────────────────────────────────────────
  const commitCard = document.createElement("div");
  commitCard.className = "gc-card flex flex-col gap-3";
  commitCard.innerHTML = `
    <div class="flex items-center justify-between">
      <div class="text-display-md font-medium">이 저장소에서 할 일</div>
      <span id="upstream-pill" class="inline-flex items-center gap-1"></span>
    </div>
    <div class="gc-action-bar gc-action-bar--4col">
      <button id="btn-commit" class="gc-action-cell"></button>
      <button id="btn-push" class="gc-action-cell"></button>
      <button id="btn-pull" class="gc-action-cell"></button>
      <button id="btn-stash" class="gc-action-cell"></button>
    </div>
  `;
  main.appendChild(commitCard);

  // Fill button icons + labels (avoid HTML-entity parsing pitfalls for innerHTML).
  function fillBtn(el: HTMLButtonElement, name: Parameters<typeof icon>[0], label: string) {
    el.appendChild(icon(name, 16));
    const s = document.createElement("span");
    s.textContent = label;
    el.appendChild(s);
  }
  fillBtn(commitCard.querySelector<HTMLButtonElement>("#btn-commit")!, "commit", "커밋");
  fillBtn(commitCard.querySelector<HTMLButtonElement>("#btn-push")!, "push", "푸시");
  fillBtn(commitCard.querySelector<HTMLButtonElement>("#btn-pull")!, "pull", "풀");
  fillBtn(commitCard.querySelector<HTMLButtonElement>("#btn-stash")!, "stash", "스태시");

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
    main.insertBefore(banner, commitCard);
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
  }

  // Lightweight polling — teammates' commits/pushes surface without
  // a manual refresh. Skipped while a modal is open or an input is focused.
  let polling = false;
  window.setInterval(async () => {
    if (polling || !main.isConnected) return;
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
      for (const b of branches) {
        const opt = document.createElement("option");
        opt.value = b.name;
        opt.textContent = b.name + (b.is_remote ? " (remote)" : "");
        if (repo && b.name === repo.working_branch) opt.selected = true;
        branchSel.appendChild(opt);
      }
      // working_branch 가 비어 있으면(등록 직후) 지금 체크아웃된 브랜치를 고른다.
      if (!repo?.working_branch && currentStatus?.branch) {
        branchSel.value = currentStatus.branch;
      }
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

  function renderStatusTable() {
    if (!currentStatus) {
      table.innerHTML = `<div class="text-display-sm text-[color:var(--color-ink-muted)]">상태를 불러올 수 없습니다</div>`;
      statusPill.textContent = "?";
      return;
    }
    const { ahead, behind, files } = currentStatus;
    // 예전에는 `" 1개 파일"` 처럼 앞에 빈 칸이 남았고, 변경이 없을 때도
    // "0개 파일" 이라고 했다. 상태 한 줄은 그 자체로 읽혀야 한다.
    const parts: string[] = [];
    if (ahead > 0) parts.push(`↑${ahead}`);
    if (behind > 0) parts.push(`↓${behind}`);
    parts.push(files.length === 0 ? "변경 없음" : `변경 ${files.length}개`);
    statusPill.textContent = parts.join(" ");
    statusPill.title =
      (ahead > 0 ? `푸시하지 않은 커밋 ${ahead}개. ` : "") +
      (behind > 0 ? `아직 받지 않은 커밋 ${behind}개. ` : "") +
      (files.length === 0 ? "커밋할 변경이 없습니다." : `커밋하지 않은 파일 ${files.length}개.`);
    // Upstream pill near commit row.
    const pill = commitCard.querySelector<HTMLElement>("#upstream-pill")!;
    pill.innerHTML = "";
    if (ahead > 0) {
      const a = document.createElement("span");
      a.className = "gc-badge gc-badge--success";
      a.textContent = `↑${ahead}`;
      pill.appendChild(a);
    }
    if (behind > 0) {
      const b = document.createElement("span");
      b.className = "gc-badge gc-badge--muted";
      b.textContent = `↓${behind}`;
      pill.appendChild(b);
    }
    // 커밋할 것이 없을 때 '커밋'을 누르면 메시지를 다 쓴 뒤에야 실패한다.
    // 눌리지 않게 하고, 무엇을 하면 눌리는지 툴팁에 적는다.
    const commitBtnEl = commitCard.querySelector<HTMLButtonElement>("#btn-commit")!;
    commitBtnEl.disabled = files.length === 0;
    commitBtnEl.title = files.length === 0
      ? "커밋할 변경이 없습니다. 파일을 고치면 아래 목록에 나타납니다."
      : "";
    if (files.length === 0) {
      table.innerHTML = `<div class="text-display-sm text-[color:var(--color-ink-muted)]">변경 사항 없음</div>`;
      return;
    }
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
  }

  renderStatusTable();
  await loadBranches();

  // ── 병합 관리자 (프로젝트 설정 .gpconfig) ────────────────────────────────
  // 브랜치별 관리자를 표시하고, 명시된 관리자가 아닌 로그인 사용자의 푸시를 잠근다.
  let projectCfg = await ipc.projectConfigGet(repoId).catch(() => null);
  const pushBtnRef = () => commitCard.querySelector<HTMLButtonElement>("#btn-push")!;
  /** 원격이 없으면 푸시·풀·동기화는 무엇을 해도 실패한다 — 아래 두 곳이 함께 본다. */
  const noRemote = !repo?.remote_url;
  const noRemoteWhy =
    "이 저장소에는 원격(origin)이 없어 주고받을 곳이 없습니다.\n" +
    "터미널에서 등록하세요:  git remote add origin <저장소 주소>";

  function refreshManagerBadge() {
    const branch = branchSel.value.replace(/^origin\//, "");
    const managerEmail = projectCfg?.config?.merge_managers?.[branch];
    if (!managerEmail) {
      managerBadge.style.display = "none";
      // 관리자 잠금이 없다고 해서 푸시를 무조건 열면 안 된다 — 원격이 없는
      // 저장소에서 브랜치를 전환하거나 로그인/로그아웃할 때마다 여기가
      // 불려서, 눌러 보면 실패하는 버튼이 되살아났다.
      pushBtnRef().disabled = noRemote;
      pushBtnRef().title = noRemote ? noRemoteWhy : "";
      return;
    }
    const member = projectCfg?.config?.members.find((x) => x.email.toLowerCase() === managerEmail.toLowerCase());
    const name = member?.name ?? managerEmail;
    const me = getSession();
    const isAdmin = me
      ? (projectCfg?.config?.members ?? []).some(
          (x) =>
            x.email.toLowerCase() === me.email.toLowerCase() &&
            x.role === "admin",
        )
      : false;
    const isManager = me && me.email.toLowerCase() === managerEmail.toLowerCase();
    managerBadge.style.display = "";
    managerBadge.textContent = `병합 관리자: ${name}${isManager ? " (나)" : ""}`;
    // 명시된 관리자가 있고, 로그인 계정이 그 관리자(또는 admin)가 아니면 푸시 잠금.
    const blocked = !!me && !isManager && !isAdmin;
    const btn = pushBtnRef();
    btn.disabled = blocked || noRemote;
    btn.title = blocked
      ? `이 브랜치의 병합 관리자는 ${name}님입니다. 푸시는 관리자만 할 수 있습니다.`
      : noRemote
        ? noRemoteWhy
        : "";
  }

  window.addEventListener("gc-account-changed", refreshManagerBadge);
  refreshManagerBadge();

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
  }
  refreshRemoteDependentButtons();

  // ── Branch change ─────────────────────────────────────────────────────────
  branchSel.addEventListener("change", async () => {
    // 원격 트래킹 항목(origin/…)을 선택한 경우 로컬 브랜치 이름으로 정규화해 전환한다.
    const branch = branchSel.value.replace(/^origin\//, "");
    branchSel.disabled = true;
    setBusy(statusPill, true, "전환 중…");
    try {
      await ipc.checkoutBranch(repoId, branch);
      await ipc.updateRepository(repoId, { working_branch: branch });
      toast("브랜치 전환 완료", "success");
      applyStatus(await ipc.status(repoId).catch(() => null));
      projectCfg = await ipc.projectConfigGet(repoId).catch(() => null);
      refreshManagerBadge();
    } catch (e) {
      toast(`브랜치 전환 실패: ${(e as Error).message ?? e}`, "error");
    } finally {
      branchSel.disabled = false;
      setBusy(statusPill, false);
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
          await ipc.commit(repoId, msg, stageAll);
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
        <input type="checkbox" id="stage-all" checked />
        <span>바뀐 파일 전부 커밋 (끄면 위 목록에서 체크한 파일만)</span>
      </label>
    `;
  });

  // ── Push ─────────────────────────────────────────────────────────────────
  const pushBtn = commitCard.querySelector<HTMLButtonElement>("#btn-push")!;
  pushBtn.addEventListener("click", async () => {
    if (pushBtn.disabled) return;
    setBusy(pushBtn, true, "푸시 중…");
    try {
      const currentBranch = branchSel.value.replace(/^origin\//, "") || null;
      const outcome = await openPushCredentialFlow(repo, currentBranch);
      if (outcome === "ok") {
        toast("푸시 완료", "success");
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
