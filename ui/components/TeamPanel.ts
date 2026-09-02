import { ipc, ipc_peer, type PeerProjectInfo, type Repo, type RepoLinkSummary } from "../lib/ipc";
import { openModal, confirmDialog } from "./Modal";
import { toast } from "./Toast";
import { icon } from "./Icon";
import { renderPageLoading } from "./Busy";
import { renderInboxList } from "./TeamInbox";
import type { Page } from "./Sidebar";

export type TeamTab = "projects" | "create" | "join" | "inbox";

export interface TeamPanelOpts {
  unread?: number;
  onNav?: (p: Page) => void;
}

export async function renderTeamPanel(
  activeTab: TeamTab,
  onTab: (t: TeamTab) => void,
  opts: TeamPanelOpts = {},
): Promise<HTMLElement> {
  const wrap = document.createElement("div");
  wrap.className = "flex flex-col h-full";

  const tabs: { id: TeamTab; label: string }[] = [
    { id: "projects", label: "프로젝트" },
    { id: "create", label: "만들기" },
    { id: "join", label: "참여하기" },
    { id: "inbox", label: "알림" },
  ];

  wrap.innerHTML = `
    <div class="px-6 pt-6 pb-4">
      <div class="gc-tabs">
        ${tabs.map((t) => `
          <button data-tab="${t.id}" class="gc-tab ${activeTab === t.id ? "is-active" : ""}">
            ${t.label}${t.id === "inbox" && (opts.unread ?? 0) > 0 ? `<span class="gc-badge gc-badge--danger">${String(opts.unread)}</span>` : ""}
          </button>
        `).join("")}
      </div>
    </div>
    <div id="panel-body" class="flex-1 overflow-y-auto p-6"></div>
  `;

  for (const t of tabs) {
    const btn = wrap.querySelector<HTMLButtonElement>(`[data-tab="${t.id}"]`)!;
    btn.addEventListener("click", () => onTab(t.id));
  }

  const body = wrap.querySelector<HTMLDivElement>("#panel-body")!;
  body.appendChild(await renderTab(activeTab, onTab, opts));

  return wrap;
}

async function renderTab(
  tab: TeamTab,
  onTab: (t: TeamTab) => void,
  opts: TeamPanelOpts = {},
): Promise<HTMLElement> {
  if (tab === "projects") return renderProjectsTab(onTab);
  if (tab === "create") return renderCreateTab();
  if (tab === "join") return renderJoinTab();
  return renderInboxList(opts.onNav ?? (() => {}));
}

// ─── Projects tab ────────────────────────────────────────────────────────────

async function renderProjectsTab(onTab: (t: TeamTab) => void): Promise<HTMLElement> {
  const el = document.createElement("div");
  el.className = "flex flex-col gap-4";

  const pageHead = document.createElement("div");
  pageHead.className = "gc-page-head";
  const h = document.createElement("div");
  h.className = "gc-page-head__title";
  h.textContent = "팀";
  pageHead.appendChild(h);
  const s = document.createElement("div");
  s.className = "gc-page-head__sub";
  s.textContent = "팀 프로젝트를 만들고 공유하세요.";
  pageHead.appendChild(s);
  el.appendChild(pageHead);

  let projects: PeerProjectInfo[] = [];
  try {
    projects = await ipc_peer.listProjects();
  } catch {
    const banner = document.createElement("div");
    banner.className = "gc-banner gc-banner--warning";
    const iw = document.createElement("span");
    iw.className = "gc-banner__icon";
    iw.appendChild(icon("users", 20));
    banner.appendChild(iw);
    const inner = document.createElement("div");
    inner.className = "flex flex-col gap-3 flex-1";
    const t = document.createElement("div");
    t.className = "gc-banner__title";
    t.textContent = "피어 백엔드에 연결할 수 없습니다";
    inner.appendChild(t);
    const d = document.createElement("div");
    d.className = "gc-banner__body text-display-sm";
    d.textContent = "팀 프로젝트 공유·알림은 백엔드 연결이 필요합니다.";
    inner.appendChild(d);
    const form = document.createElement("form");
    form.className = "flex gap-2";
    form.innerHTML = `
      <input id="peer-backend-url" class="gc-input" type="text" placeholder="http://127.0.0.1:8000" />
      <button type="submit" class="gc-button-primary">연결</button>
    `;
    const errBox = document.createElement("div");
    errBox.className = "text-display-sm text-[color:var(--color-danger)]";
    errBox.style.display = "none";
    const hint = document.createElement("div");
    hint.className = "text-display-sm text-[color:var(--color-ink-muted)]";
    form.appendChild(errBox);
    hint.style.display = "none";
    hint.textContent = "백엔드 주소를 입력하세요.";
    inner.appendChild(form);
    inner.appendChild(hint);
    banner.appendChild(inner);
    el.appendChild(banner);

    form.addEventListener("submit", async (ev) => {
      ev.preventDefault();
      const url = (form.querySelector<HTMLInputElement>("#peer-backend-url")!).value.trim();
      errBox.style.display = "none";
      hint.style.display = "none";
      if (!url) {
        hint.style.display = "block";
        return;
      }
      try {
        await ipc_peer.setBackendUrl(url);
        onTab("projects");
      } catch (e) {
        errBox.textContent = `연결 실패: ${(e as Error).message ?? e}`;
        errBox.style.display = "block";
      }
    });
    return el;
  }

  if (projects.length === 0) {
    const empty = document.createElement("div");
    empty.className = "gc-empty";
    const iw = document.createElement("span");
    iw.className = "gc-empty__icon";
    iw.appendChild(icon("users", 32));
    empty.appendChild(iw);
    const t = document.createElement("div");
    t.className = "gc-empty__title";
    t.textContent = "참여 중인 프로젝트가 없습니다";
    empty.appendChild(t);
    const d = document.createElement("div");
    d.className = "gc-empty__desc";
    d.textContent = "새 프로젝트를 만들거나 참여 코드로 합류하세요.";
    empty.appendChild(d);
    const btns = document.createElement("div");
    btns.className = "flex gap-2";
    const mk = document.createElement("button");
    mk.className = "gc-button-secondary";
    mk.textContent = "프로젝트 만들기";
    mk.addEventListener("click", () => onTab("create"));
    const jn = document.createElement("button");
    jn.className = "gc-button-secondary";
    jn.textContent = "코드로 참여하기";
    jn.addEventListener("click", () => onTab("join"));
    btns.appendChild(mk);
    btns.appendChild(jn);
    empty.appendChild(btns);
    el.appendChild(empty);
    return el;
  }

  const grid = document.createElement("div");
  grid.className = "flex flex-col gap-4";
  for (const p of projects) {
    const card = await projectCard(p);
    grid.appendChild(card);
  }
  el.appendChild(grid);

  return el;
}

async function projectCard(p: PeerProjectInfo): Promise<HTMLElement> {
  const card = document.createElement("div");
  card.className = "gc-card flex flex-col gap-3";

  const header = document.createElement("div");
  header.className = "flex items-start justify-between gap-3";
  header.innerHTML = `
    <div>
      <div class="text-display-md font-medium">${escape(p.display_name)}</div>
      <div class="text-display-sm text-[color:var(--color-ink-muted)] mt-1">${p.role}</div>
    </div>
    <div class="text-display-sm font-mono text-[color:var(--color-ink-muted)]">${formatCode(p.join_code)}</div>
  `;
  card.appendChild(header);

  const actions = document.createElement("div");
  actions.className = "flex gap-2";
  actions.innerHTML = `
    <button class="gc-button-secondary text-display-sm" data-copy>코드 복사</button>
    <button class="gc-button-secondary text-display-sm text-[color:var(--color-danger)]" data-leave>탈퇴</button>
  `;
  actions.querySelector<HTMLButtonElement>("[data-copy]")!.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(p.join_code);
      toast("참여 코드를 클립보드에 복사했습니다", "info");
    } catch {
      toast("복사 실패", "error");
    }
  });
  actions.querySelector<HTMLButtonElement>("[data-leave]")!.addEventListener("click", async () => {
    const confirmed = await confirmDialog({
      title: "프로젝트 탈퇴",
      message: `"${p.display_name}" 프로젝트에서 탈퇴하시겠습니까?`,
      confirmLabel: "탈퇴",
      destructive: true,
    });
    if (!confirmed) return;
    try {
      await ipc_peer.leaveProject(p.id);
      toast("프로젝트를 탈퇴했습니다", "info");
    } catch (e) {
      toast(`탈퇴 실패: ${(e as Error).message}`, "error");
    }
  });
  card.appendChild(actions);

  // ── 연결된 저장소 — 프로젝트에 내 저장소를 묶는다 (push 알림 수신 대상) ──
  const repoSection = document.createElement("div");
  repoSection.className = "flex flex-col gap-2 border-t border-[color:var(--color-hairline)] pt-3";
  const repoTitle = document.createElement("div");
  repoTitle.className = "text-display-sm font-medium";
  repoTitle.textContent = "연결된 저장소";
  repoSection.appendChild(repoTitle);
  const repoDesc = document.createElement("div");
  repoDesc.className = "text-display-sm text-[color:var(--color-ink-muted)]";
  repoDesc.textContent = "팀원 push·병합 알림을 받을 내 저장소를 연결하세요.";
  repoSection.appendChild(repoDesc);
  const repoList = document.createElement("div");
  repoList.className = "flex flex-col gap-1";
  repoSection.appendChild(repoList);
  const linkRow = document.createElement("div");
  linkRow.className = "flex items-center gap-2";
  const linkSel = document.createElement("select");
  linkSel.className = "gc-input flex-1 min-w-0 text-display-sm";
  linkRow.appendChild(linkSel);
  const linkBtn = document.createElement("button");
  linkBtn.className = "gc-button-secondary text-display-sm shrink-0";
  linkBtn.textContent = "연결";
  linkBtn.addEventListener("click", async () => {
    const rid = linkSel.value;
    if (!rid) return;
    linkBtn.disabled = true;
    try {
      await ipc_peer.linkRepo(rid, p.id);
      toast("저장소를 프로젝트에 연결했습니다.", "success");
      await refreshRepos();
    } catch (e) {
      toast(`연결 실패: ${(e as Error).message}`, "error");
    } finally {
      linkBtn.disabled = false;
    }
  });
  linkRow.appendChild(linkBtn);
  repoSection.appendChild(linkRow);

  async function refreshRepos() {
    const [linked, all] = await Promise.all([
      ipc_peer.reposForProject(p.id).catch(() => [] as RepoLinkSummary[]),
      ipc.listRepositories().catch(() => [] as Repo[]),
    ]);
    repoList.innerHTML = "";
    if (linked.length === 0) {
      const empty = document.createElement("div");
      empty.className = "text-display-sm text-[color:var(--color-ink-muted)]";
      empty.textContent = "연결된 저장소가 없습니다.";
      repoList.appendChild(empty);
    } else {
      for (const l of linked) {
        const row = document.createElement("div");
        row.className = "flex items-center gap-2 text-display-sm";
        const name = document.createElement("span");
        name.className = "flex-1 min-w-0 truncate";
        name.textContent = l.display_name;
        name.title = l.path;
        row.appendChild(name);
        const unlink = document.createElement("button");
        unlink.className = "gc-button-secondary text-display-sm text-[color:var(--color-danger)] shrink-0";
        unlink.textContent = "연결 해제";
        unlink.addEventListener("click", async () => {
          const confirmed = await confirmDialog({
            title: "저장소 연결 해제",
            message: `"${l.display_name}"을(를) 프로젝트에서 연결 해제하시겠습니까?`,
            confirmLabel: "연결 해제",
            destructive: true,
          });
          if (!confirmed) return;
          try {
            await ipc_peer.unlinkRepo(l.repo_id, p.id);
            toast("저장소 연결을 해제했습니다.", "success");
            await refreshRepos();
          } catch (e) {
            toast(`연결 해제 실패: ${(e as Error).message}`, "error");
          }
        });
        row.appendChild(unlink);
        repoList.appendChild(row);
      }
    }
    // 연결되지 않은 저장소만 선택지로 보여준다.
    linkSel.innerHTML = `<option value="">저장소 선택…</option>`;
    const linkedIds = new Set(linked.map((l) => l.repo_id));
    for (const r of all) {
      if (linkedIds.has(r.id)) continue;
      const opt = document.createElement("option");
      opt.value = r.id;
      opt.textContent = r.display_name;
      linkSel.appendChild(opt);
    }
  }
  await refreshRepos();
  card.appendChild(repoSection);

  // ── Invite button ─────────────────────────────────────────────────────
  const inviteSection = document.createElement("div");
  inviteSection.className = "flex flex-col gap-2 border-t border-[color:var(--color-hairline)] pt-3";

  const inviteTitle = document.createElement("div");
  inviteTitle.className = "text-display-sm font-medium";
  inviteTitle.textContent = "팀원 추가";
  inviteSection.appendChild(inviteTitle);

  const addBtn = document.createElement("button");
  addBtn.className = "gc-button-primary self-start text-display-sm";
  addBtn.textContent = "팀원 초대";
  addBtn.addEventListener("click", () => openInviteModal(p.id, refreshMembers));
  inviteSection.appendChild(addBtn);

  // ── Member list ───────────────────────────────────────────────────────
  const memberList = document.createElement("div");
  memberList.className = "flex flex-col gap-1";

  async function refreshMembers() {
    memberList.innerHTML = "";
    memberList.appendChild(renderPageLoading("팀원 목록 불러오는 중…"));
    try {
      const members = await ipc_peer.listMembers(p.id);
      memberList.innerHTML = "";
      if (members.length === 0) {
        memberList.innerHTML = `<div class="text-display-sm text-[color:var(--color-ink-muted)]">아직 팀원이 없습니다.</div>`;
        return;
      }
      for (const m of members) {
        const row = document.createElement("div");
        row.className = "flex items-center gap-2 text-display-sm";
        const label = m.email
          ? `${m.email}${m.name ? ` (${m.name})` : ""} — ${m.role}`
          : `${m.name ?? m.device_id} — ${m.role}`;
        row.innerHTML = `
          <span class="flex-1">${escape(label)}</span>
          ${m.email ? `<button class="gc-button-secondary text-display-sm text-[color:var(--color-danger)]" data-remove>취소</button>` : ""}
        `;
        if (m.email) {
          row.querySelector<HTMLButtonElement>("[data-remove]")!.addEventListener("click", async () => {
            const confirmed = await confirmDialog({
              title: "초대 취소",
              message: ` "${m.email}" 초대를 취소하시겠습니까?`,
              confirmLabel: "취소",
              destructive: true,
            });
            if (!confirmed) return;
            try {
              await ipc_peer.removeEmailInvite(p.id, m.email!);
              refreshMembers();
            } catch (e) {
              toast(`취소 실패: ${(e as Error).message}`, "error");
            }
          });
        }
        memberList.appendChild(row);
      }
    } catch (e) {
      memberList.innerHTML = `<div class="text-display-sm text-[color:var(--color-danger)]">팀원 목록 로딩 실패</div>`;
    }
  }

  await refreshMembers();

  inviteSection.appendChild(memberList);
  card.appendChild(inviteSection);

  return card;
}

function openInviteModal(projectId: string, onSuccess: () => void): void {
  const m = openModal({
    title: "팀원 초대",
    submitLabel: "초대",
    onSubmit: async (close) => {
      const email = (m.body.querySelector<HTMLInputElement>("#invite-email")!).value.trim();
      const name = (m.body.querySelector<HTMLInputElement>("#invite-name")!).value.trim();
      const role = (m.body.querySelector<HTMLSelectElement>("#invite-role")!).value;
      if (!email) { m.setError("이메일을 입력하세요."); return; }
      m.setSubmitting(true);
      m.setError(null);
      try {
        await ipc_peer.inviteByEmail(projectId, email, name || null, role);
        toast("초대 완료", "success");
        onSuccess();
        close();
      } catch (e) {
        m.setError(`초대 실패: ${(e as Error).message ?? e}`);
      } finally {
        m.setSubmitting(false);
      }
    },
  });

  m.body.innerHTML = `
    <div class="flex flex-col gap-1">
      <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="invite-email">이메일 <span class="text-[color:var(--color-danger)]">*</span></label>
      <input id="invite-email" class="gc-input" type="email" placeholder="team@example.com" />
    </div>
    <div class="flex flex-col gap-1">
      <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="invite-name">이름 (선택)</label>
      <input id="invite-name" class="gc-input" type="text" placeholder="홍길동" />
    </div>
    <div class="flex flex-col gap-1">
      <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="invite-role">역할</label>
      <select id="invite-role" class="gc-input">
        <option value="member">member</option>
        <option value="admin">admin</option>
      </select>
    </div>
  `;
}

// ─── Create tab ──────────────────────────────────────────────────────────────

async function renderCreateTab(): Promise<HTMLElement> {
  const el = document.createElement("div");
  el.className = "flex flex-col gap-4";

  const header = document.createElement("div");
  header.className = "flex items-center justify-between";
  header.innerHTML = `<div class="text-display-md font-medium">팀 프로젝트 만들기</div>`;
  el.appendChild(header);

  const emptyCard = document.createElement("div");
  emptyCard.className = "gc-card text-center py-8";
  emptyCard.innerHTML = `
    <div class="text-display-sm text-[color:var(--color-ink-muted)] mb-4">아직 프로젝트가 없습니다.</div>
    <button class="gc-button-primary" id="btn-create-project">+ 새 프로젝트</button>
  `;
  el.appendChild(emptyCard);

  const status = document.createElement("div");
  status.className = "text-display-sm";
  el.appendChild(status);

  emptyCard.querySelector<HTMLButtonElement>("#btn-create-project")!.addEventListener("click", () => {
    const m = openModal({
      title: "팀 프로젝트 만들기",
      submitLabel: "만들기",
      onSubmit: async (close) => {
        const name = (m.body.querySelector<HTMLInputElement>("#proj-name")!).value.trim();
        if (!name) { m.setError("프로젝트 이름을 입력하세요."); return; }
        m.setSubmitting(true);
        m.setError(null);
        try {
          const repoId = (m.body.querySelector<HTMLSelectElement>("#repo-select")!).value || null;
          const info = await ipc_peer.createProject(name, repoId);
          toast(`프로젝트 "${info.display_name}"이(가) 생성되었습니다. 참여 코드: ${formatCode(info.join_code)}`, "info");
          close();
        } catch (e) {
          m.setError(`생성 실패: ${(e as Error).message ?? e}`);
        } finally {
          m.setSubmitting(false);
        }
      },
    });

    m.body.innerHTML = `
      <div class="flex flex-col gap-1">
        <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="proj-name">프로젝트 이름 <span class="text-[color:var(--color-danger)]">*</span></label>
        <input id="proj-name" class="gc-input" type="text" placeholder="내 프로젝트" />
      </div>
      <div class="flex flex-col gap-1">
        <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="repo-select">저장소 선택</label>
      </div>
    `;
    buildRepoSelect(m.body, "proj-name", "repo-select");
  });

  return el;
}

// ─── Join tab ────────────────────────────────────────────────────────────────

async function renderJoinTab(): Promise<HTMLElement> {
  const el = document.createElement("div");
  el.className = "flex flex-col gap-4";

  const header = document.createElement("div");
  header.className = "flex items-center justify-between";
  header.innerHTML = `<div class="text-display-md font-medium">팀 프로젝트 참여</div>`;
  el.appendChild(header);

  const emptyCard = document.createElement("div");
  emptyCard.className = "gc-card text-center py-8";
  emptyCard.innerHTML = `
    <div class="text-display-sm text-[color:var(--color-ink-muted)] mb-4">참여 코드를 입력하여 팀 프로젝트에 합류하세요.</div>
    <button class="gc-button-primary" id="btn-join-project">참여 코드로 합치기</button>
  `;
  el.appendChild(emptyCard);

  emptyCard.querySelector<HTMLButtonElement>("#btn-join-project")!.addEventListener("click", () => {
    const m = openModal({
      title: "팀 프로젝트 참여",
      submitLabel: "참여하기",
      onSubmit: async (close) => {
        let code = (m.body.querySelector<HTMLInputElement>("#join-code")!).value.trim().replace("-", "").toUpperCase();
        if (code.length !== 8) { m.setError("8자리 참여 코드를 입력하세요."); return; }
        m.setSubmitting(true);
        m.setError(null);
        try {
          const repoId = (m.body.querySelector<HTMLSelectElement>("#repo-select")!).value || null;
          const info = await ipc_peer.joinProject(code, repoId);
          toast(`"${info.display_name}"에 참여했습니다!`, "info");
          close();
        } catch (e) {
          m.setError(`참여 실패: ${(e as Error).message ?? e}`);
        } finally {
          m.setSubmitting(false);
        }
      },
    });

    m.body.innerHTML = `
      <div class="flex flex-col gap-1">
        <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="join-code">참여 코드 <span class="text-[color:var(--color-danger)]">*</span></label>
        <input id="join-code" class="gc-input font-mono" type="text" placeholder="ABCD-1234" maxlength="9" />
      </div>
    `;
    buildRepoSelect(m.body, "join-code", "repo-select");
  });

  return el;
}

// ─── Shared helpers ─────────────────────────────────────────────────────────

async function buildRepoSelect(container: HTMLElement, _focusId: string, selectId: string): Promise<void> {
  const repoSelect = document.createElement("select");
  repoSelect.id = selectId;
  repoSelect.className = "gc-input";
  repoSelect.innerHTML = `<option value="">저장소 선택 안 함</option>`;
  container.appendChild(repoSelect); // append immediately so querySelector finds it
  try {
    const repos = await ipc.listRepositories();
    for (const r of repos) {
      const opt = document.createElement("option");
      opt.value = r.id;
      opt.textContent = r.display_name;
      repoSelect.appendChild(opt);
    }
  } catch { /* repos unavailable */ }
}

function formatCode(code: string): string {
  if (code.length === 8) return `${code.slice(0, 4)}-${code.slice(4)}`;
  return code;
}

function escape(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}
