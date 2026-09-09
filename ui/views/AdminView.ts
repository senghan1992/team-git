// AdminView — 서버 운영자의 관리 화면.
//
// "누가 이 앱을 쓰고, 무엇이 일어나고 있는가"를 한 화면에서 보고(추적),
// 문제가 되는 계정·프로젝트를 즉시 다룰 수 있게(제어) 한다.
//
// 청화백자 세계에서 이 화면의 정체성은 **원장(ledger)** 이다 — 정확한 수치,
// 한 줄씩 읽히는 목록, 색은 상태 신호에만. 제어는 파괴적인 동작일수록
// 확인 대화상자를 거치고, 자기 계정에 대한 제어는 서버가 막는다.
import { ipc_peer, type AdminEventRow, type AdminProject, type AdminUser } from "../lib/ipc";
import { confirmDialog } from "../components/Modal";
import { toast } from "../components/Toast";
import { icon } from "../components/Icon";
import { setBusy } from "../components/Busy";
import { getSession } from "../lib/session";

type AdminTab = "overview" | "users" | "projects" | "events";

function relative(iso: string | null): string {
  if (!iso) return "—";
  const diff = Math.max(0, Date.now() - new Date(iso).getTime()) / 1000;
  if (diff < 60) return "방금";
  if (diff < 3600) return `${Math.floor(diff / 60)}분 전`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}시간 전`;
  return `${Math.floor(diff / 86400)}일 전`;
}

/** 이벤트 종류 → 사람이 읽는 라벨. 앱 전역에서 같은 말을 쓴다. */
const KIND_LABEL: Record<string, string> = {
  main_push: "병합 반영",
  branch_push: "브랜치 push",
  merge_request: "병합 요청",
  release: "릴리스",
};

export async function renderAdminView(): Promise<HTMLElement> {
  const main = document.createElement("main");
  main.className = "flex-1 overflow-y-auto p-8 flex flex-col gap-5";

  // ── 헤더 — 프로젝트명 행과 같은 문법: 좌측 제목, 우측 행동 ────────────
  const headRow = document.createElement("div");
  headRow.className = "flex flex-wrap items-center justify-between gap-4";
  const head = document.createElement("div");
  head.className = "min-w-0";
  const title = document.createElement("div");
  title.className = "gc-page-head__title";
  title.textContent = "서버 관리";
  head.appendChild(title);
  const sub = document.createElement("div");
  sub.className = "gc-page-head__sub";
  sub.textContent = `${getSession()?.name ?? ""} — 이 서버의 사용자와 프로젝트를 추적하고 제어합니다`;
  head.appendChild(sub);
  headRow.appendChild(head);
  const refreshBtn = document.createElement("button");
  refreshBtn.className = "gc-button-secondary inline-flex items-center gap-1";
  refreshBtn.appendChild(icon("refresh", 14));
  const refreshLabel = document.createElement("span");
  refreshLabel.textContent = "새로고침";
  refreshBtn.appendChild(refreshLabel);
  headRow.appendChild(refreshBtn);
  main.appendChild(headRow);

  // ── 탭 ───────────────────────────────────────────────────────────────
  const tabs = document.createElement("div");
  tabs.className = "gc-tabs";
  const tabDefs: { key: AdminTab; label: string; ico: Parameters<typeof icon>[0] }[] = [
    { key: "overview", label: "개요", ico: "info" },
    { key: "users", label: "사용자", ico: "users" },
    { key: "projects", label: "프로젝트", ico: "folder" },
    { key: "events", label: "활동", ico: "log" },
  ];
  let tab: AdminTab = "overview";
  const tabBtns: Record<AdminTab, HTMLButtonElement> = {} as Record<AdminTab, HTMLButtonElement>;
  for (const def of tabDefs) {
    const b = document.createElement("button");
    b.className = "gc-tab";
    b.appendChild(icon(def.ico, 14));
    const l = document.createElement("span");
    l.textContent = def.label;
    b.appendChild(l);
    b.addEventListener("click", () => {
      tab = def.key;
      paintTabs();
      void load();
    });
    tabs.appendChild(b);
    tabBtns[def.key] = b;
  }
  main.appendChild(tabs);

  const content = document.createElement("div");
  content.className = "flex flex-col gap-4";
  main.appendChild(content);

  function paintTabs() {
    for (const def of tabDefs) {
      tabBtns[def.key].classList.toggle("is-active", tab === def.key);
    }
  }

  // ── 상태 캐시 — 탭을 옮겨 다닐 때마다 다시 불러오지 않게 ─────────────
  let overview: Awaited<ReturnType<typeof ipc_peer.adminOverview>> | null = null;
  let users: AdminUser[] | null = null;
  let projects: AdminProject[] | null = null;
  let events: AdminEventRow[] | null = null;

  const me = getSession();

  function emptyBox(text: string): HTMLElement {
    const e = document.createElement("div");
    e.className = "gc-empty";
    const iw = document.createElement("span");
    iw.className = "gc-empty__icon";
    iw.appendChild(icon("inbox", 28));
    e.appendChild(iw);
    const t = document.createElement("div");
    t.className = "gc-empty__title";
    t.textContent = text;
    e.appendChild(t);
    return e;
  }

  // ── 개요 ─────────────────────────────────────────────────────────────
  function renderOverview(): void {
    content.innerHTML = "";
    if (!overview) return;
    const grid = document.createElement("div");
    grid.className = "grid grid-cols-2 lg:grid-cols-4 gap-3";
    const stats: [string, string, Parameters<typeof icon>[0]][] = [
      ["사용자", `${overview.users}${overview.disabled_users > 0 ? ` · 정지 ${overview.disabled_users}` : ""}`, "users"],
      ["로그인 세션", String(overview.sessions), "user"],
      ["프로젝트", String(overview.projects), "folder"],
      ["24시간 이벤트", `${overview.events_24h} · 주간 ${overview.events_7d}`, "log"],
    ];
    for (const [label, value, ico] of stats) {
      const card = document.createElement("div");
      card.className = "gc-card flex flex-col gap-1.5 p-4";
      const head = document.createElement("div");
      head.className = "flex items-center gap-2 text-display-sm text-[color:var(--color-ink-muted)]";
      head.appendChild(icon(ico, 14));
      head.appendChild(document.createTextNode(label));
      card.appendChild(head);
      const v = document.createElement("div");
      v.className = "font-mono text-display-xl font-semibold tracking-[-0.02em]";
      v.textContent = value;
      card.appendChild(v);
      grid.appendChild(card);
    }
    content.appendChild(grid);

    // 최근 활동 미리보기 — 활동 탭의 첫 5건을 얇게 보여 준다.
    const card = document.createElement("div");
    card.className = "gc-card flex flex-col gap-2";
    const head = document.createElement("div");
    head.className = "font-medium";
    head.textContent = "최근 활동";
    card.appendChild(head);
    const recent = (events ?? []).slice(0, 5);
    if (recent.length === 0) {
      card.appendChild(emptyBox("아직 팀 활동이 없습니다"));
    } else {
      for (const ev of recent) card.appendChild(eventRow(ev));
    }
    const more = document.createElement("button");
    more.className = "gc-button-secondary self-start text-display-sm";
    more.textContent = "활동 전체 보기";
    more.addEventListener("click", () => {
      tab = "events";
      paintTabs();
      void load();
    });
    card.appendChild(more);
    content.appendChild(card);
  }

  // ── 사용자 ───────────────────────────────────────────────────────────
  function renderUsers(): void {
    content.innerHTML = "";
    if (!users) return;
    const card = document.createElement("div");
    card.className = "gc-card flex flex-col divide-y divide-[color:var(--color-hairline)]";
    if (users.length === 0) {
      card.appendChild(emptyBox("가입한 사용자가 없습니다"));
      content.appendChild(card);
      return;
    }
    for (const u of users) {
      const row = document.createElement("div");
      row.className = "flex flex-wrap items-center gap-3 px-4 py-3";

      const avatar = document.createElement("span");
      avatar.className = "inline-flex items-center justify-center w-9 h-9 rounded-full text-white font-medium shrink-0";
      const laneIdx = (parseInt(u.id.slice(0, 2), 16) % 6) + 1;
      avatar.style.background = `var(--lane-${laneIdx})`;
      avatar.textContent = (u.name || "?").trim().charAt(0).toUpperCase();
      row.appendChild(avatar);

      const who = document.createElement("div");
      who.className = "min-w-0 flex-1";
      const nameRow = document.createElement("div");
      nameRow.className = "flex items-center gap-2 flex-wrap";
      const name = document.createElement("span");
      name.className = "font-medium";
      name.textContent = u.name;
      nameRow.appendChild(name);
      if (u.is_admin) {
        const badge = document.createElement("span");
        badge.className = "gc-badge gc-badge--info";
        badge.textContent = "관리자";
        nameRow.appendChild(badge);
      }
      if (u.disabled) {
        const badge = document.createElement("span");
        badge.className = "gc-badge gc-badge--danger";
        badge.textContent = "정지";
        nameRow.appendChild(badge);
      }
      who.appendChild(nameRow);
      const meta = document.createElement("div");
      meta.className = "text-display-sm text-[color:var(--color-ink-muted)] truncate";
      meta.textContent = `${u.email} · 마지막 활동 ${relative(u.last_seen)} · 세션 ${u.sessions} · 기기 ${u.devices} · 프로젝트 ${u.projects}`;
      who.appendChild(meta);
      row.appendChild(who);

      // 제어 — 자기 계정에는 잠긴 버튼 대신 안내를 남긴다 (서버도 거부한다).
      const controls = document.createElement("div");
      controls.className = "flex items-center gap-2 shrink-0";
      const isSelf = me?.id === u.id;
      const statusBtn = document.createElement("button");
      statusBtn.className = u.disabled ? "gc-button-secondary" : "gc-button-secondary text-[color:var(--color-danger)]";
      statusBtn.textContent = u.disabled ? "정지 해제" : "계정 정지";
      if (isSelf) {
        statusBtn.disabled = true;
        statusBtn.title = "자기 계정은 정지할 수 없습니다.";
      } else {
        statusBtn.addEventListener("click", () => void toggleUser(u, statusBtn));
      }
      controls.appendChild(statusBtn);
      const logoutBtn = document.createElement("button");
      logoutBtn.className = "gc-button-secondary";
      logoutBtn.textContent = "강제 로그아웃";
      logoutBtn.disabled = u.sessions === 0;
      logoutBtn.title = u.sessions === 0 ? "활성 세션이 없습니다." : "모든 세션 토큰을 지웁니다";
      logoutBtn.addEventListener("click", () => void forceLogout(u, logoutBtn));
      controls.appendChild(logoutBtn);
      row.appendChild(controls);
      card.appendChild(row);
    }
    content.appendChild(card);
  }

  async function toggleUser(u: AdminUser, btn: HTMLButtonElement): Promise<void> {
    const next = !u.disabled;
    const ok = await confirmDialog({
      title: next ? "계정 정지" : "정지 해제",
      message: next
        ? `${u.name} (${u.email}) 계정을 정지합니다.\n로그인·세션·기기 폴링이 즉시 막히고, 세션은 모두 삭제됩니다.`
        : `${u.name} (${u.email}) 계정의 정지를 해제합니다. 다시 로그인할 수 있습니다.`,
      confirmLabel: next ? "정지" : "해제",
      destructive: next,
    });
    if (!ok) return;
    setBusy(btn, true, "처리 중…");
    try {
      await ipc_peer.adminSetUserStatus(u.id, next);
      toast(next ? `${u.name} 계정을 정지했습니다.` : `${u.name} 계정의 정지를 해제했습니다.`, "success");
      users = null;
      overview = null;
      await load();
    } catch (e) {
      toast(`처리 실패: ${(e as Error).message}`, "error");
    } finally {
      setBusy(btn, false);
    }
  }

  async function forceLogout(u: AdminUser, btn: HTMLButtonElement): Promise<void> {
    const ok = await confirmDialog({
      title: "강제 로그아웃",
      message: `${u.name} (${u.email}) 의 모든 세션 토큰을 지웁니다. 다음 요청부터 다시 로그인해야 합니다.`,
      confirmLabel: "로그아웃",
    });
    if (!ok) return;
    setBusy(btn, true, "처리 중…");
    try {
      const r = await ipc_peer.adminForceLogout(u.id);
      toast(`세션 ${r.sessions_revoked}개를 지웠습니다.`, "success");
      users = null;
      await load();
    } catch (e) {
      toast(`처리 실패: ${(e as Error).message}`, "error");
    } finally {
      setBusy(btn, false);
    }
  }

  // ── 프로젝트 ─────────────────────────────────────────────────────────
  function renderProjects(): void {
    content.innerHTML = "";
    if (!projects) return;
    if (projects.length === 0) {
      content.appendChild(emptyBox("만들어진 프로젝트가 없습니다"));
      return;
    }
    const card = document.createElement("div");
    card.className = "gc-card flex flex-col divide-y divide-[color:var(--color-hairline)]";
    for (const p of projects) {
      const row = document.createElement("div");
      row.className = "flex flex-col gap-2 px-4 py-3";

      const head = document.createElement("div");
      head.className = "flex flex-wrap items-center gap-3";
      const nameWrap = document.createElement("div");
      nameWrap.className = "min-w-0 flex-1";
      const name = document.createElement("div");
      name.className = "font-medium flex items-center gap-2";
      name.textContent = p.display_name;
      const active = document.createElement("span");
      active.className = p.events_24h > 0 ? "gc-badge gc-badge--success" : "gc-badge gc-badge--muted";
      active.textContent = p.events_24h > 0 ? `활발 · 24시간 ${p.events_24h}건` : "조용함";
      name.appendChild(active);
      nameWrap.appendChild(name);
      const meta = document.createElement("div");
      meta.className = "text-display-sm text-[color:var(--color-ink-muted)]";
      meta.textContent = `멤버 ${p.member_count} · 누적 이벤트 ${p.events_total} · 마지막 활동 ${relative(p.last_event_at)}`;
      nameWrap.appendChild(meta);
      head.appendChild(nameWrap);
      const delBtn = document.createElement("button");
      delBtn.className = "gc-button-secondary text-[color:var(--color-danger)]";
      delBtn.textContent = "프로젝트 삭제";
      delBtn.addEventListener("click", () => void deleteProject(p, delBtn));
      head.appendChild(delBtn);
      row.appendChild(head);

      // 멤버 한 줄씩 — 기기와 사람, 마지막 접속. 제거 버튼.
      const memberList = document.createElement("div");
      memberList.className = "flex flex-col gap-1 pl-1";
      for (const m of p.members) {
        const mrow = document.createElement("div");
        mrow.className = "flex items-center gap-2 text-display-sm";
        const dot = document.createElement("span");
        dot.className = "w-1.5 h-1.5 rounded-full shrink-0";
        dot.style.background = m.role === "owner" ? "var(--color-primary)" : "var(--color-ink-muted)";
        mrow.appendChild(dot);
        const label = document.createElement("span");
        label.className = "min-w-0 truncate";
        label.textContent = `${m.user_name ?? m.device_name}${m.email ? ` (${m.email})` : ""} · ${m.role} · 마지막 접속 ${relative(m.last_seen)}`;
        mrow.appendChild(label);
        if (m.role !== "owner") {
          const rm = document.createElement("button");
          rm.className = "text-display-xs text-[color:var(--color-ink-muted)] hover:text-[color:var(--color-danger)] cursor-pointer";
          rm.textContent = "제거";
          rm.addEventListener("click", () => void removeMember(p, m.device_id, m.user_name ?? m.device_name, rm));
          mrow.appendChild(rm);
        }
        memberList.appendChild(mrow);
      }
      row.appendChild(memberList);
      card.appendChild(row);
    }
    content.appendChild(card);
  }

  async function removeMember(
    p: AdminProject,
    deviceId: string,
    name: string,
    btn: HTMLElement,
  ): Promise<void> {
    const ok = await confirmDialog({
      title: "멤버 제거",
      message: `${p.display_name} 프로젝트에서 ${name} 의 기기를 제거합니다.\n이 프로젝트의 이벤트를 더 이상 받지 못합니다.`,
      confirmLabel: "제거",
      destructive: true,
    });
    if (!ok) return;
    setBusy(btn, true, "처리 중…");
    try {
      await ipc_peer.adminRemoveMember(p.id, deviceId);
      toast(`${name} 을(를) ${p.display_name} 에서 제거했습니다.`, "success");
      projects = null;
      overview = null;
      await load();
    } catch (e) {
      toast(`제거 실패: ${(e as Error).message}`, "error");
    } finally {
      setBusy(btn, false);
    }
  }

  async function deleteProject(p: AdminProject, btn: HTMLButtonElement): Promise<void> {
    const ok = await confirmDialog({
      title: "프로젝트 삭제",
      message: `${p.display_name} 프로젝트를 삭제합니다.\n멤버·초대·이벤트 기록이 모두 지워지고 팀원에게 더 이상 알림이 가지 않습니다.\n저장소 파일과 커밋은 건드리지 않습니다. 되돌릴 수 없습니다.`,
      confirmLabel: "삭제",
      destructive: true,
    });
    if (!ok) return;
    setBusy(btn, true, "삭제 중…");
    try {
      await ipc_peer.adminDeleteProject(p.id);
      toast(`${p.display_name} 프로젝트를 삭제했습니다.`, "success");
      projects = null;
      overview = null;
      events = null;
      await load();
    } catch (e) {
      toast(`삭제 실패: ${(e as Error).message}`, "error");
    } finally {
      setBusy(btn, false);
    }
  }

  // ── 활동 피드 ────────────────────────────────────────────────────────
  function eventRow(ev: AdminEventRow): HTMLElement {
    const row = document.createElement("div");
    row.className = "flex items-start gap-2.5 min-w-0";
    const kind = document.createElement("span");
    kind.className =
      "gc-badge shrink-0 " +
      (ev.kind === "main_push"
        ? "gc-badge--success"
        : ev.kind === "merge_request"
          ? "gc-badge--info"
          : "gc-badge--muted");
    kind.textContent = KIND_LABEL[ev.kind] ?? ev.kind;
    row.appendChild(kind);
    const body = document.createElement("div");
    body.className = "min-w-0 flex-1";
    const line = document.createElement("div");
    line.className = "text-display-sm truncate";
    line.textContent = `${ev.sender_user ?? ev.sender_device} · ${ev.repo_name}${ev.message ? ` — ${ev.message}` : ""}`;
    line.title = line.textContent;
    body.appendChild(line);
    const meta = document.createElement("div");
    meta.className = "text-display-xs text-[color:var(--color-ink-muted)]";
    meta.textContent = relative(ev.created_at);
    body.appendChild(meta);
    row.appendChild(body);
    return row;
  }

  function renderEvents(): void {
    content.innerHTML = "";
    if (!events) return;
    const card = document.createElement("div");
    card.className = "gc-card flex flex-col gap-3";
    const head = document.createElement("div");
    head.className = "font-medium";
    head.textContent = `최근 활동 ${events.length}건`;
    card.appendChild(head);
    if (events.length === 0) {
      card.appendChild(emptyBox("아직 팀 활동이 없습니다"));
    } else {
      const list = document.createElement("div");
      list.className = "flex flex-col divide-y divide-[color:var(--color-hairline)]";
      for (const ev of events) {
        const wrap = document.createElement("div");
        wrap.className = "py-2";
        wrap.appendChild(eventRow(ev));
        list.appendChild(wrap);
      }
      card.appendChild(list);
    }
    content.appendChild(card);
  }

  // ── 로드 & 그리기 ────────────────────────────────────────────────────
  let loading = false;
  async function load(): Promise<void> {
    if (loading) return;
    loading = true;
    try {
      if (tab === "overview" && !overview) overview = await ipc_peer.adminOverview();
      if (tab === "users" && !users) users = await ipc_peer.adminUsers();
      if (tab === "projects" && !projects) projects = await ipc_peer.adminProjects();
      if (tab === "events" && !events) events = await ipc_peer.adminEvents(60);
      // 개요의 "최근 활동" 미리보기와 제어 후 갱신을 위해 활동도 가끔 읽는다.
      if (!events) void ipc_peer.adminEvents(60).then((e) => { events = e; }).catch(() => undefined);
    } catch (e) {
      content.innerHTML = "";
      const err = document.createElement("div");
      err.className = "gc-banner gc-banner--warning";
      const body = document.createElement("span");
      body.className = "gc-banner__body flex-1";
      body.textContent = `불러오기 실패: ${(e as Error).message}`;
      err.appendChild(body);
      content.appendChild(err);
      loading = false;
      return;
    }
    if (tab === "overview") renderOverview();
    else if (tab === "users") renderUsers();
    else if (tab === "projects") renderProjects();
    else renderEvents();
    loading = false;
  }

  refreshBtn.addEventListener("click", () => {
    overview = users = projects = events = null;
    void load();
  });

  // 20초마다 조용히 갱신 — 대화상자가 열려 있으면 건너뛴다.
  const poll = window.setInterval(() => {
    if (!main.isConnected) {
      window.clearInterval(poll);
      return;
    }
    if (document.querySelector("dialog[open]")) return;
    overview = users = projects = events = null;
    void load();
  }, 20_000);

  paintTabs();
  await load();
  return main;
}
