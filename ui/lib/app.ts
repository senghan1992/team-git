import { ipc, ipc_peer, type Repo, type TeamEventRow } from "./ipc";
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
  let teamTab: TeamTab = "projects";

  void ipc.listRepositories().then((r) => { repos = r; rerender(); });

  void refreshSession();

  // 로그인 상태가 바뀌면 (로그인/로그아웃/계정 삭제) 앱 전체를 다시 그린다.
  window.addEventListener(ACCOUNT_EVENT, () => { rerender(); });

  void ipc_peer.unreadCount()
    .then((n) => { teamUnread = n; updateTeamBadge(); })
    .catch(() => { teamUnread = 0; });

  async function reloadRepos() {
    repos = await ipc.listRepositories();
    rerender();
  }

  async function reloadTeamUnread() {
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

  function repoByDisplayName(name: string): Repo | null {
    const matches = (repos ?? []).filter((r) => r.display_name === name);
    return matches.length === 1 ? matches[0]! : null;
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
      markRead();
      if (msg.includes("병합이 있습니다")) {
        toast("이미 진행 중인 병합이 있어 병합 센터로 이동합니다.", "info");
        page = { kind: "repo", repoId: repo.id, tab: "merge" };
      } else {
        toast(`동기화 실패: ${msg}`, "error");
      }
    }
    rerender();
  }

  async function pollTeamEvents() {
    if (!getSession()) return;
    try {
      const rows = await ipc_peer.listTeamEvents(50, true);
      teamUnread = rows.length;
      for (const r of rows) {
        if (seenEvents.has(r.id)) continue;
        seenEvents.add(r.id);
        const isMainPush =
          r.event_kind === "main_push" || r.event_kind.endsWith("main_push");
        if (!isMainPush || r.read) continue;
        // 오래된 백로그는 방해하지 않도록 최근 15분 이벤트만 알림으로 띄운다.
        const ageMs = Date.now() - new Date(r.received_at).getTime();
        if (ageMs > 15 * 60 * 1000) continue;
        const repo = repoByDisplayName(r.repo_name);
        if (repo) {
          notify(
            `${r.repo_name}에 새 병합이 반영되었습니다`,
            { label: "내 브랜치에 동기화", run: () => void runSyncFromEvent(r, repo) },
            "main에 최신 코드가 푸시되었습니다. 내 브랜치에도 반영하세요.",
          );
        } else {
          notify(
            `${r.repo_name}에 새 병합이 반영되었습니다`,
            { label: "저장소 등록하기", run: () => { page = { kind: "home" }; rerender(); } },
            "등록된 저장소가 없어 동기화할 수 없습니다.",
          );
        }
      }
    } catch {
      teamUnread = 0;
    }
    updateTeamBadge();
  }

  function updateTeamBadge() {
    const badge = document.querySelector<HTMLSpanElement>("#team-badge");
    if (badge) {
      if (teamUnread > 0) {
        badge.textContent = String(teamUnread);
        badge.style.display = "";
      } else {
        badge.style.display = "none";
      }
    }
  }

  function renderGate() {
    shell.innerHTML = "";
    const gate = document.createElement("div");
    gate.className = "flex-1 flex items-center justify-center p-8";
    gate.id = "login-gate";
    const plaque = document.createElement("div");
    plaque.className = "gc-card flex flex-col items-center gap-4 text-center max-w-md w-full px-10 py-12";
    const tile = document.createElement("div");
    tile.className = "inline-flex items-center justify-center w-11 h-11 rounded-[12px] bg-[color:var(--color-primary)] text-white shadow-[inset_0_1px_0_rgba(255,255,255,0.2),0_2px_6px_rgba(28,50,96,0.3)]";
    tile.appendChild(icon("lock", 20));
    plaque.appendChild(tile);
    const title = document.createElement("h1");
    title.className = "text-display-xl font-bold tracking-[-0.02em]";
    title.textContent = "Git Companion";
    plaque.appendChild(title);
    const desc = document.createElement("p");
    desc.className = "text-display-sm text-[color:var(--color-ink-muted)] max-w-sm whitespace-pre-line";
    desc.textContent =
      "이 앱을 사용하려면 로그인이 필요합니다. \n테스트 계정: test / test, test2 / test2";
    plaque.appendChild(desc);
    const btnRow = document.createElement("div");
    btnRow.className = "flex items-center gap-3 mt-2";
    const btn = document.createElement("button");
    btn.className = "gc-button-primary";
    btn.id = "gate-login-btn";
    btn.textContent = "로그인";
    btn.addEventListener("click", () => openAccountModal());
    const regBtn = document.createElement("button");
    regBtn.className = "gc-button-secondary";
    regBtn.id = "gate-register-btn";
    regBtn.textContent = "계정 만들기";
    regBtn.addEventListener("click", () => openRegisterModal());
    btnRow.appendChild(btn);
    btnRow.appendChild(regBtn);
    plaque.appendChild(btnRow);
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
    if (session === null) {
      renderGate();
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
      renderRepoView(myPage.repoId, myPage.tab ?? "work", onTab).then((m) => {
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
