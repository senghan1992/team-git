import { ipc, ipc_peer, type ProjectConfigResult, type Repo, type TeamEventRow } from "./ipc";
import { isMergeManagerFor } from "../components/nextAction";
import { repoForEvent } from "./repoMatch";
import { renderSidebar, type Page } from "../components/Sidebar";
import { renderHomeView } from "../views/HomeView";
import { renderRepoView } from "../views/RepoView";
import { renderSettingsView } from "../views/SettingsView";
import { renderTeamPanel, type TeamTab } from "../components/TeamPanel";
import { renderToasts, notify, toast } from "../components/Toast";
import { renderPageLoadingFill } from "../components/Busy";
import { refreshSession, getSession, ACCOUNT_EVENT } from "./session";
import { openAccountModal, openRegisterModal } from "../components/AccountModal";
import { icon } from "../components/Icon";

export async function createApp(root: HTMLElement) {
  root.className = "h-full flex flex-col overflow-hidden";
  const shell = document.createElement("div");
  shell.className = "flex-1 flex min-h-0";
  root.appendChild(shell);
  root.appendChild(renderToasts());

  let page: Page = { kind: "home" };
  let repos: Repo[] | null = null;
  let teamUnread = 0;
  // 알림 화면은 기본이 "받은 알림"이고, 배달망 설정은 접혀 있다.
  let teamTab: TeamTab = "inbox";

  void ipc.listRepositories().then((r) => { repos = r; rerender(); });

  // 로그인 상태가 바뀌면 (로그인/로그아웃/계정 삭제) 앱 전체를 다시 그린다.
  window.addEventListener(ACCOUNT_EVENT, () => { rerender(); });

  // 수신함에서 읽음 처리하면 사이드바 배지가 곧바로 따라온다.
  window.addEventListener("gc-team-read-changed", () => {
    void reloadTeamUnread().then(updateTeamBadge);
  });

  // 알림 개수는 로그인한 뒤에만 의미가 있다. 예전에는 로그아웃 상태에서도
  // 조회해서 사이드바에 "알림 2" 가 떴다 — 로그인도 안 했는데 읽지 않은
  // 알림이 있다고 하니 앱을 처음 켠 사람에게는 앞뒤가 맞지 않는다.
  void refreshSession().then(() => {
    if (getSession()) void reloadTeamUnread().then(updateTeamBadge);
  });

  async function reloadRepos() {
    repos = await ipc.listRepositories();
    rerender();
  }

  async function reloadTeamUnread() {
    if (!getSession()) {
      teamUnread = 0;
      return;
    }
    try {
      teamUnread = await ipc_peer.unreadCount();
    } catch {
      teamUnread = 0;
    }
  }

  // ── 팀 이벤트 → 우측 하단 알림 (member 시나리오) ───────────────────────
  // main에 병합이 푸시되면 어떤 등록 저장소인지 찾아 "내 브랜치에 동기화"
  // 액션을 가진 알림을 띄운다. 이미 본 이벤트는 다시 띄우지 않는다.
  const seenEvents = new Set<string>();

  // 매칭 열쇠는 payload의 remote URL — 폴더 이름은 사람마다 달라서
  // 이름만으로 찾으면 못 찾거나(다르게 clone) 엉뚱하게 찾는다(동명 저장소).
  function repoOfEvent(r: TeamEventRow): Repo | null {
    return repoForEvent(repos ?? [], r);
  }

  // 저장소별 .gpconfig 캐시 — 알림 판정마다 IPC(원격이면 SSH 왕복)를 때리지
  // 않기 위해 짧게 캐시한다. 관리자 지정이 바뀌면 1분 안에 반영된다.
  const PROJECT_CFG_TTL_MS = 60_000;
  const projectCfgCache = new Map<
    string,
    { cfg: ProjectConfigResult | null; at: number }
  >();
  async function projectCfgOf(repoId: string): Promise<ProjectConfigResult | null> {
    const hit = projectCfgCache.get(repoId);
    if (hit && Date.now() - hit.at < PROJECT_CFG_TTL_MS) return hit.cfg;
    const cfg = await ipc.projectConfigGet(repoId).catch(() => null);
    projectCfgCache.set(repoId, { cfg, at: Date.now() });
    return cfg;
  }

  /** 이벤트를 만든 사람이 나인지 — 내 푸시 알림을 나에게 다시 띄우지 않는다. */
  function isMyOwnEvent(r: TeamEventRow): boolean {
    const me = getSession();
    if (!me) return false;
    try {
      const payload = JSON.parse(r.payload) as { data?: { author?: string } };
      const author = payload.data?.author?.trim();
      return !!author && author === me.name.trim();
    } catch {
      return false;
    }
  }

  async function runSyncFromEvent(r: TeamEventRow, repo: Repo) {
    const markRead = () => {
      ipc_peer.markTeamRead(r.id).then(reloadTeamUnread).catch(() => undefined);
    };
    try {
      const res = await ipc.syncBranch(repo.id, repo.default_branch);
      markRead();
      if (res.conflicted) {
        toast(`충돌 ${res.files.length}개 발생 — 병합 센터에서 해결하세요.`, "info");
        page = { kind: "repo", repoId: repo.id, tab: "merge" };
      } else {
        toast("동기화 완료 — 최신 변경을 내 브랜치에 병합했습니다.", "success");
        page = { kind: "repo", repoId: repo.id };
      }
    } catch (e) {
      const msg = (e as Error).message ?? String(e);
      if (msg.includes("병합이 있습니다")) {
        markRead();
        toast("이미 진행 중인 병합이 있어 병합 센터로 이동합니다.", "info");
        page = { kind: "repo", repoId: repo.id, tab: "merge" };
      } else if (msg.includes("커밋하지 않은 변경")) {
        // 커밋/스태시 버튼이 있는 곳으로 데려다 준다 — 읽음 처리는 하지 않는다
        // (아직 동기화가 남은 할 일이므로 배지에 남긴다).
        toast(`동기화 실패: ${msg}`, "error");
        page = { kind: "repo", repoId: repo.id };
      } else {
        // 실패한 동기화는 읽음 처리하지 않는다 — 수신함에서 다시 시도할 수 있다.
        toast(`동기화 실패: ${msg}`, "error");
      }
    }
    rerender();
  }

  /** 이벤트 payload에서 브랜치 이름을 꺼낸다 (없으면 null). */
  function branchOfEvent(r: TeamEventRow): string | null {
    try {
      const payload = JSON.parse(r.payload) as { data?: { branch?: string } };
      const b = payload.data?.branch?.trim();
      return b || null;
    } catch {
      return null;
    }
  }

  async function pollTeamEvents() {
    if (!getSession()) return;
    // 모달(커밋 메시지 등)을 쓰는 중에는 액션 토스트를 띄우지 않는다 —
    // 모달의 top-layer 가 토스트를 덮어 버튼을 누를 수 없고, 12초 뒤 조용히
    // 사라진다. 이벤트를 소비하지 않고 통째로 미루면 다음 폴링(5초)이
    // 모달이 닫힌 뒤 그대로 이어받는다. 배지는 계속 갱신한다.
    if (document.querySelector("dialog[open]")) {
      await reloadTeamUnread();
      updateTeamBadge();
      return;
    }
    try {
      // 서버에 쌓인 이벤트를 로컬 수신함으로 끌어온다 — sidecar(푸시 배달)가
      // 없거나 포트 등록이 깨져 있어도 알림이 5초 안에 도착하는 폴백 경로.
      await ipc_peer.pollNow().catch(() => undefined);
      const rows = await ipc_peer.listTeamEvents(50, true);
      // 배지는 목록 길이(50에서 포화)가 아니라 실제 미읽음 총계를 쓴다.
      await reloadTeamUnread();
      for (const r of rows) {
        if (seenEvents.has(r.id)) continue;
        seenEvents.add(r.id);
        if (r.read) continue;
        // 오래된 백로그는 방해하지 않도록 최근 15분 이벤트만 알림으로 띄운다.
        const ageMs = Date.now() - new Date(r.received_at).getTime();
        if (ageMs > 15 * 60 * 1000) continue;

        const isMainPush =
          r.event_kind === "main_push" || r.event_kind.endsWith("main_push");
        const isBranchPush =
          r.event_kind === "branch_push" || r.event_kind.endsWith("branch_push");
        if (!isMainPush && !isBranchPush) continue;
        if (isMyOwnEvent(r)) continue;

        const repo = repoOfEvent(r);

        // ── 시나리오 6: 병합 브랜치에 푸시됨 → 팀원은 내 브랜치에 동기화한다.
        if (isMainPush) {
          if (repo) {
            notify(
              `${r.repo_name}에 새 병합이 반영되었습니다`,
              { label: "내 브랜치에 동기화", run: () => void runSyncFromEvent(r, repo) },
              `${repo.default_branch || "main"}에 최신 코드가 푸시되었습니다. 내 브랜치에도 반영하세요.`,
            );
          } else {
            notify(
              `${r.repo_name}에 새 병합이 반영되었습니다`,
              { label: "저장소 등록하기", run: () => { page = { kind: "home" }; rerender(); } },
              "등록된 저장소가 없어 동기화할 수 없습니다.",
            );
          }
          continue;
        }

        // ── 시나리오 7: 팀원이 자기 브랜치를 푸시함 → 병합 관리자에게만 알린다.
        //    관리자가 아닌 사람에게는 팀 수신함 배지로만 남는다.
        if (!repo) continue;
        const base = repo.default_branch || "main";
        const cfg = await projectCfgOf(repo.id);
        const me = getSession();
        // 관리자가 아직 지정되지 않았으면(설정 전 초기 상태) 알림으로 재촉하지
        // 않는다 — 모두에게 알림이 가면 소음이 된다.
        const assigned = cfg?.config?.merge_managers?.[base];
        if (!assigned) continue;
        if (!isMergeManagerFor(cfg, me?.email ?? null, base)) continue;

        const branch = branchOfEvent(r);
        notify(
          branch
            ? `${r.repo_name}: ${branch} 브랜치가 병합을 기다립니다`
            : `${r.repo_name}에 새 푸시가 있습니다`,
          {
            label: "병합하기",
            run: () => {
              ipc_peer.markTeamRead(r.id).then(reloadTeamUnread).catch(() => undefined);
              page = { kind: "repo", repoId: repo.id, tab: "merge" };
              rerender();
            },
          },
          `${base}(으)로 병합할 수 있습니다.`,
        );
      }
    } catch {
      teamUnread = 0;
    }
    updateTeamBadge();
  }

  function updateTeamBadge() {
    const badge = document.querySelector<HTMLSpanElement>("#team-badge");
    if (badge) {
      if (teamUnread > 0 && getSession()) {
        badge.textContent = String(teamUnread);
        badge.style.display = "";
      } else {
        badge.style.display = "none";
      }
    }
  }

  // ── 시작 화면 ────────────────────────────────────────────────────────────
  //
  // 로그인은 **선택**이다. 예전에는 로그인하지 않으면 여기서 더 나아갈 수
  // 없었는데, 계정이 팀 서버로 옮겨간 뒤에는 그 벽이 곧 "FastAPI 서버를 직접
  // 띄워야 앱을 열 수 있다"는 뜻이 됐다. 처음 git 을 쓰는 사람에게는 사실상
  // 사용 불가다.
  //
  // 커밋·푸시·병합·충돌 해결은 서버 없이 전부 동작한다. 계정이 실제로 필요한
  // 것은 팀 알림과 구성원 검색뿐이므로, 그때 그 자리에서 로그인을 권한다.
  const SKIP_LOGIN_KEY = "gc-skip-login";

  function hasDismissedLogin(): boolean {
    try {
      return localStorage.getItem(SKIP_LOGIN_KEY) === "1";
    } catch {
      // 저장소를 못 쓰는 환경(사생활 보호 모드 등)에서도 앱은 열려야 한다.
      return false;
    }
  }
  function dismissLogin() {
    try {
      localStorage.setItem(SKIP_LOGIN_KEY, "1");
    } catch {
      /* 이번 실행에만 적용된다 */
    }
    skippedThisRun = true;
  }
  let skippedThisRun = false;

  function renderWelcome() {
    shell.innerHTML = "";
    const gate = document.createElement("div");
    gate.className = "flex-1 flex items-center justify-center p-8";
    gate.id = "login-gate";
    const plaque = document.createElement("div");
    plaque.className =
      "gc-card flex flex-col items-center gap-4 text-center max-w-lg w-full px-10 py-12";
    const tile = document.createElement("div");
    tile.className =
      "inline-flex items-center justify-center w-11 h-11 rounded-[12px] bg-[color:var(--color-primary)] text-white shadow-[inset_0_1px_0_rgba(255,255,255,0.2),0_2px_6px_rgba(28,50,96,0.3)]";
    tile.appendChild(icon("commit", 20));
    plaque.appendChild(tile);
    const title = document.createElement("h1");
    title.className = "text-display-xl font-bold tracking-[-0.02em]";
    title.textContent = "Git Companion";
    plaque.appendChild(title);
    const desc = document.createElement("p");
    desc.className =
      "text-display-md text-[color:var(--color-ink-muted)] max-w-md whitespace-pre-line";
    desc.textContent =
      "팀이 하나의 프로젝트를 각자 브랜치로 나눠 작업할 때,\n커밋·푸시·병합을 터미널 없이 처리하는 앱입니다.";
    plaque.appendChild(desc);

    // 무엇이 로그인 없이 되고, 무엇이 안 되는지 미리 알려 준다.
    const table = document.createElement("div");
    table.className = "flex flex-col gap-1.5 text-display-sm w-full max-w-md mt-1";
    const rows: [string, string][] = [
      ["바로 사용", "저장소 등록 · 커밋 · 푸시 · 병합 · 충돌 해결"],
      ["로그인 필요", "팀원 push 알림 · 구성원 검색"],
    ];
    for (const [tag, what] of rows) {
      const row = document.createElement("div");
      row.className = "flex items-start gap-2 text-left";
      const chip = document.createElement("span");
      chip.className =
        "gc-badge shrink-0 " + (tag === "바로 사용" ? "gc-badge--success" : "gc-badge--muted");
      chip.textContent = tag;
      row.appendChild(chip);
      const txt = document.createElement("span");
      txt.className = "text-[color:var(--color-ink-muted)]";
      txt.textContent = what;
      row.appendChild(txt);
      table.appendChild(row);
    }
    plaque.appendChild(table);

    const btnRow = document.createElement("div");
    btnRow.className = "flex flex-wrap items-center justify-center gap-2 mt-3";
    const start = document.createElement("button");
    start.className = "gc-button-primary";
    start.id = "gate-start-btn";
    start.textContent = "저장소 열고 시작하기";
    start.addEventListener("click", () => {
      dismissLogin();
      page = { kind: "home" };
      rerender();
    });
    btnRow.appendChild(start);
    const btn = document.createElement("button");
    btn.className = "gc-button-secondary";
    btn.id = "gate-login-btn";
    btn.textContent = "로그인";
    btn.addEventListener("click", () => openAccountModal());
    btnRow.appendChild(btn);
    const regBtn = document.createElement("button");
    regBtn.className = "gc-button-secondary";
    regBtn.id = "gate-register-btn";
    regBtn.textContent = "계정 만들기";
    regBtn.addEventListener("click", () => openRegisterModal());
    btnRow.appendChild(regBtn);
    plaque.appendChild(btnRow);

    const note = document.createElement("p");
    note.className = "text-display-xs text-[color:var(--color-ink-muted)] max-w-md";
    note.textContent =
      "계정은 팀이 함께 쓰는 서버에 저장됩니다. 나중에 왼쪽 아래에서 언제든 로그인할 수 있습니다.";
    plaque.appendChild(note);

    gate.appendChild(plaque);
    shell.appendChild(gate);
    updateTeamBadge();
  }

  function rerender() {
    // 앱 사용은 로그인 필수 — 세션을 모르는 동안은 로딩, 미로그인은 게이트.
    const session = getSession();
    if (session === undefined) {
      shell.innerHTML = "";
      shell.appendChild(renderPageLoadingFill());
      return;
    }
    // 처음 실행에서 한 번만 소개 화면을 보여 준다. "시작하기"를 누른 뒤에는
    // 로그아웃 상태여도 곧바로 앱으로 들어간다.
    if (session === null && !hasDismissedLogin() && !skippedThisRun) {
      renderWelcome();
      return;
    }
    // Close any open dialogs before wiping the shell — they live in the top layer
    // and innerHTML does not remove them, so they would ghost across navigations.
    for (const d of document.querySelectorAll<HTMLDialogElement>('dialog[open]')) {
      d.close();
      d.remove();
    }
    shell.innerHTML = "";
    shell.appendChild(renderSidebar(page, repos ?? [], (p) => { page = p; rerender(); }));
    if (page.kind === "home") {
      if (repos === null) {
        shell.appendChild(renderPageLoadingFill());
      } else {
        shell.appendChild(renderHomeView(repos, (p) => { page = p; rerender(); }, reloadRepos));
      }
    } else if (page.kind === "repo") {
      const onTab = (t: "work" | "merge" | "config") => { const cur = page; if (cur.kind === "repo") { page = { ...cur, tab: t }; rerender(); } };
      const loading = renderPageLoadingFill();
      shell.appendChild(loading);
      const myPage = page;
      renderRepoView(
        myPage.repoId,
        myPage.tab ?? "work",
        onTab,
        () => { page = { kind: "settings" }; rerender(); },
      ).then((m) => {
        if (page !== myPage) return; // 로딩 중 다른 페이지로 이동 시 폐기
        loading.replaceWith(m);
      });
    } else if (page.kind === "team") {
      const main = document.createElement("main");
      main.className = "flex-1 overflow-hidden flex flex-col";
      shell.appendChild(main);
      const loading = renderPageLoadingFill();
      main.appendChild(loading);
      renderTeamPanel(
        teamTab,
        (t) => { teamTab = t; rerender(); },
        { unread: teamUnread, onNav: (p: Page) => { page = p; rerender(); } },
      ).then((m) => {
        if (page.kind !== "team") return;
        loading.replaceWith(m);
      });
    } else if (page.kind === "settings") {
      const loading = renderPageLoadingFill();
      shell.appendChild(loading);
      renderSettingsView().then((m) => {
        if (page.kind !== "settings") return;
        loading.replaceWith(m);
      });
    }
    updateTeamBadge();
  }

  rerender();

  // Periodically refresh team unread count + surface new main-push events.
  setInterval(() => { void pollTeamEvents(); }, 5000);
}
