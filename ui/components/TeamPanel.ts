// 알림 화면 — "팀원의 push·병합 소식이 나에게 오게 하는" 곳.
//
// 이 화면은 **알림 배달망 설정**이다. 사람의 권한(누가 어느 브랜치로 병합할 수
// 있는가)은 여기가 아니라 저장소 안에 커밋되는 `.gpconfig`(저장소 → 설정 탭)가
// 정한다. 예전에는 두 곳에 각각 "팀원" 목록이 있어서 같은 사람을 두 번
// 등록해야 했고, 어느 쪽이 무엇을 결정하는지 화면만 봐서는 알 수 없었다.
//
// 그래서 이렇게 정리했다:
//   - 기본 화면은 **받은 알림** 하나. (예전엔 프로젝트/만들기/참여하기/알림 4탭)
//   - 배달망 설정(백엔드 주소·프로젝트·수신자)은 접히는 섹션 하나로 내렸다.
//   - 수신자는 `.gpconfig` 구성원에서 **동기화** 버튼으로 가져온다 — 이메일을
//     두 번 입력하지 않는다.
//   - 저장소는 체크박스로 켜고 끈다 (드롭다운 + 연결/해제 버튼 대신).
import {
  ipc,
  ipc_peer,
  type MemberInfo,
  type PeerProjectInfo,
  type Repo,
  type RepoLinkSummary,
} from "../lib/ipc";
import { openModal, confirmDialog } from "./Modal";
import { toast } from "./Toast";
import { icon } from "./Icon";
import { setBusy } from "./Busy";
import { renderInboxList } from "./TeamInbox";
import { getSession } from "../lib/session";
import { openAccountModal, openRegisterModal } from "./AccountModal";
import type { Page } from "./Sidebar";

/** 로그아웃 상태에서 이 화면을 열었을 때 — 무엇이 필요한지 한 장으로. */
function signInInvite(): HTMLElement {
  const box = document.createElement("div");
  box.className = "gc-card flex flex-col items-start gap-3 max-w-xl";
  const title = document.createElement("div");
  title.className = "text-display-lg font-medium";
  title.textContent = "알림을 받으려면 로그인이 필요합니다";
  box.appendChild(title);
  const desc = document.createElement("div");
  desc.className = "text-display-sm text-[color:var(--color-ink-muted)] whitespace-pre-line";
  desc.textContent =
    "팀원이 브랜치를 push했을 때 알림을 받으려면 누가 누구인지 서버가 알아야 합니다.\n" +
    "왼쪽 아래 내 이름 자리에서 로그인하면 이 화면이 곧바로 채워집니다.";
  box.appendChild(desc);
  const row = document.createElement("div");
  row.className = "flex gap-2";
  const login = document.createElement("button");
  login.className = "gc-button-primary";
  login.textContent = "로그인";
  login.addEventListener("click", () => openAccountModal());
  row.appendChild(login);
  const reg = document.createElement("button");
  reg.className = "gc-button-secondary";
  reg.textContent = "계정 만들기";
  reg.addEventListener("click", () => openRegisterModal());
  row.appendChild(reg);
  box.appendChild(row);
  return box;
}

/** 접히는 "알림 설정" 섹션이 열려 있는지. 화면을 다시 그려도 유지된다. */
export type TeamTab = "inbox" | "settings";

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
  wrap.className = "flex-1 overflow-y-auto p-8 flex flex-col gap-6";

  // 로그인하지 않은 상태로도 앱을 쓸 수 있으므로, 이 화면은 "왜 로그인이
  // 필요한지"를 설명하는 자리가 된다. 빈 목록이나 오류를 보여 주면 사용자는
  // 기능이 고장 난 줄 안다.
  if (!getSession()) {
    wrap.appendChild(signInInvite());
    return wrap;
  }

  // ── 헤더 ────────────────────────────────────────────────────────────────
  const head = document.createElement("div");
  head.className = "gc-page-head";
  const title = document.createElement("div");
  title.className = "gc-page-head__title";
  title.textContent = "알림";
  head.appendChild(title);
  const sub = document.createElement("div");
  sub.className = "gc-page-head__sub";
  sub.textContent =
    "팀원이 브랜치를 push하거나 병합이 반영되면 여기로 옵니다. 병합 권한은 저장소 → 설정 탭에서 정합니다.";
  head.appendChild(sub);
  wrap.appendChild(head);

  // ── 받은 알림 ───────────────────────────────────────────────────────────
  wrap.appendChild(await renderInboxList(opts.onNav ?? (() => {})));

  // ── 알림 설정 (접힘) ────────────────────────────────────────────────────
  const open = activeTab === "settings";
  const disclosure = document.createElement("section");
  disclosure.className = "gc-card flex flex-col gap-3";

  const toggle = document.createElement("button");
  toggle.className = "flex items-center gap-2 text-left w-full";
  const chev = document.createElement("span");
  chev.className = "text-[color:var(--color-ink-muted)] transition-transform";
  chev.style.transform = open ? "rotate(90deg)" : "none";
  chev.appendChild(icon("arrow-right", 14));
  toggle.appendChild(chev);
  const toggleLabel = document.createElement("span");
  toggleLabel.className = "text-display-md font-medium flex-1";
  toggleLabel.textContent = "알림 설정";
  toggle.appendChild(toggleLabel);
  const toggleHint = document.createElement("span");
  toggleHint.className = "text-display-sm text-[color:var(--color-ink-muted)]";
  toggleHint.textContent = "누가 어떤 저장소의 소식을 받을지";
  toggle.appendChild(toggleHint);
  toggle.addEventListener("click", () => onTab(open ? "inbox" : "settings"));
  disclosure.appendChild(toggle);

  if (open) {
    disclosure.appendChild(await renderNotifySettings());
  }
  wrap.appendChild(disclosure);

  return wrap;
}

// ─── 알림 설정 본문 ──────────────────────────────────────────────────────────

async function renderNotifySettings(): Promise<HTMLElement> {
  const el = document.createElement("div");
  el.className = "flex flex-col gap-4 pt-1";

  let projects: PeerProjectInfo[];
  try {
    projects = await ipc_peer.listProjects();
  } catch {
    el.appendChild(renderBackendSetup());
    return el;
  }

  if (projects.length === 0) {
    el.appendChild(renderNoProject());
    return el;
  }

  for (const p of projects) {
    el.appendChild(await renderProjectBlock(p));
  }
  return el;
}

/** 백엔드에 연결할 수 없을 때 — 무엇이 안 되는지와 무엇을 하면 되는지 한 곳에. */
function renderBackendSetup(): HTMLElement {
  const box = document.createElement("div");
  box.className = "flex flex-col gap-3";

  const explain = document.createElement("div");
  explain.className = "text-display-sm text-[color:var(--color-ink-muted)] whitespace-pre-line";
  explain.textContent =
    "알림을 주고받으려면 팀이 공유하는 알림 서버가 필요합니다. 서버 없이도 커밋·푸시·병합은 모두 그대로 동작하고, 알림만 오지 않습니다.\n" +
    "서버는 이 저장소의 backend/ 를 띄우면 됩니다:  cd backend && uvicorn app.main:app";
  box.appendChild(explain);

  const form = document.createElement("form");
  form.className = "flex gap-2";
  const input = document.createElement("input");
  input.className = "gc-input flex-1";
  input.type = "text";
  input.placeholder = "http://127.0.0.1:8000";
  form.appendChild(input);
  const submit = document.createElement("button");
  submit.type = "submit";
  submit.className = "gc-button-primary shrink-0";
  submit.textContent = "연결";
  form.appendChild(submit);
  box.appendChild(form);

  const err = document.createElement("div");
  err.className = "text-display-sm text-[color:var(--color-danger)]";
  err.style.display = "none";
  box.appendChild(err);

  // 저장된 주소를 채워 준다 — 대개 오타 하나를 고치는 상황이다.
  void ipc_peer
    .getConfig()
    .then((c) => {
      if (c.backend_url) input.value = c.backend_url;
    })
    .catch(() => undefined);

  form.addEventListener("submit", async (ev) => {
    ev.preventDefault();
    const url = input.value.trim();
    err.style.display = "none";
    if (!url) {
      err.textContent = "알림 서버 주소를 입력하세요.";
      err.style.display = "";
      return;
    }
    setBusy(submit, true, "연결 중…");
    try {
      await ipc_peer.setBackendUrl(url);
      await ipc_peer.listProjects();
      toast("알림 서버에 연결했습니다.", "success");
      window.dispatchEvent(new Event("gc-account-changed"));
    } catch (e) {
      err.textContent = `연결 실패: ${(e as Error).message ?? e}`;
      err.style.display = "";
    } finally {
      setBusy(submit, false);
    }
  });

  return box;
}

/** 서버는 붙었지만 아직 팀 프로젝트가 없을 때. */
function renderNoProject(): HTMLElement {
  const box = document.createElement("div");
  box.className = "flex flex-col gap-3";

  const desc = document.createElement("div");
  desc.className = "text-display-sm text-[color:var(--color-ink-muted)] whitespace-pre-line";
  desc.textContent =
    "알림을 받을 팀을 하나 만들고, 참여 코드를 팀원에게 알려 주세요.\n" +
    "팀원은 그 코드로 합류하면 서로의 push 소식을 받습니다.";
  box.appendChild(desc);

  const row = document.createElement("div");
  row.className = "flex gap-2";
  const mk = document.createElement("button");
  mk.className = "gc-button-primary";
  mk.textContent = "팀 만들기";
  mk.addEventListener("click", () => openCreateModal());
  row.appendChild(mk);
  const jn = document.createElement("button");
  jn.className = "gc-button-secondary";
  jn.textContent = "참여 코드로 합류";
  jn.addEventListener("click", () => openJoinModal());
  row.appendChild(jn);
  box.appendChild(row);

  return box;
}

// ─── 프로젝트 한 덩어리 ──────────────────────────────────────────────────────

async function renderProjectBlock(p: PeerProjectInfo): Promise<HTMLElement> {
  const box = document.createElement("div");
  box.className = "flex flex-col gap-4";

  // ── 팀 이름 + 참여 코드 ─────────────────────────────────────────────────
  const head = document.createElement("div");
  head.className = "flex items-center gap-3 flex-wrap";
  const name = document.createElement("span");
  name.className = "text-display-md font-medium";
  name.textContent = p.display_name;
  head.appendChild(name);
  const codeChip = document.createElement("button");
  codeChip.className = "gc-badge gc-badge--muted font-mono inline-flex items-center gap-1";
  codeChip.title = "참여 코드 복사";
  codeChip.textContent = p.join_code;
  codeChip.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(p.join_code);
      toast("참여 코드를 복사했습니다. 팀원에게 알려 주세요.", "info");
    } catch {
      toast(`복사 실패 — 코드: ${p.join_code}`, "error");
    }
  });
  head.appendChild(codeChip);
  const leave = document.createElement("button");
  leave.className = "gc-button-secondary text-display-sm ml-auto";
  leave.textContent = "팀 나가기";
  leave.addEventListener("click", async () => {
    const ok = await confirmDialog({
      title: "팀 나가기",
      message: `"${p.display_name}"에서 나가면 팀원의 push 알림을 더 이상 받지 않습니다.\n저장소와 커밋에는 영향이 없습니다.`,
      confirmLabel: "나가기",
      destructive: true,
    });
    if (!ok) return;
    try {
      await ipc_peer.leaveProject(p.id);
      toast("팀에서 나갔습니다.", "info");
      window.dispatchEvent(new Event("gc-account-changed"));
    } catch (e) {
      toast(`실패: ${(e as Error).message}`, "error");
    }
  });
  head.appendChild(leave);
  box.appendChild(head);

  // ── 알림 받을 저장소 — 체크박스 하나로 켜고 끈다 ────────────────────────
  box.appendChild(await renderRepoToggles(p));

  // ── 수신자 ──────────────────────────────────────────────────────────────
  box.appendChild(await renderRecipients(p));

  return box;
}

/**
 * 등록된 저장소를 체크박스로 나열한다. 체크 = 이 팀의 알림 대상.
 *
 * 예전에는 "저장소 선택" 드롭다운 + [연결] 버튼 + 목록 + [연결 해제] 버튼이
 * 따로 있었다. 실제로 필요한 것은 켜짐/꺼짐 하나뿐이다.
 */
async function renderRepoToggles(p: PeerProjectInfo): Promise<HTMLElement> {
  const box = document.createElement("div");
  box.className = "flex flex-col gap-2";

  const label = document.createElement("div");
  label.className = "text-display-sm font-medium";
  label.textContent = "알림 받을 저장소";
  box.appendChild(label);

  const hint = document.createElement("div");
  hint.className = "text-display-sm text-[color:var(--color-ink-muted)]";
  hint.textContent = "켜 둔 저장소에 push가 생기면 팀원에게 알림이 갑니다.";
  box.appendChild(hint);

  const list = document.createElement("div");
  list.className = "flex flex-col gap-1.5";
  box.appendChild(list);

  async function paint() {
    const [linked, all] = await Promise.all([
      ipc_peer.reposForProject(p.id).catch(() => [] as RepoLinkSummary[]),
      ipc.listRepositories().catch(() => [] as Repo[]),
    ]);
    const linkedIds = new Set(linked.map((l) => l.repo_id));
    list.innerHTML = "";
    if (all.length === 0) {
      const empty = document.createElement("div");
      empty.className = "text-display-sm text-[color:var(--color-ink-muted)]";
      empty.textContent = "등록된 저장소가 없습니다. 저장소 화면에서 먼저 추가하세요.";
      list.appendChild(empty);
      return;
    }
    for (const r of all) {
      const row = document.createElement("label");
      row.className = "gc-check";
      const box2 = document.createElement("input");
      box2.type = "checkbox";
      box2.checked = linkedIds.has(r.id);
      const text = document.createElement("span");
      text.className = "flex flex-col";
      const nm = document.createElement("span");
      nm.className = "text-display-sm";
      nm.textContent = r.display_name;
      text.appendChild(nm);
      const pth = document.createElement("span");
      pth.className = "text-display-xs text-[color:var(--color-ink-muted)] truncate max-w-md";
      pth.textContent = r.path;
      text.appendChild(pth);
      box2.addEventListener("change", async () => {
        box2.disabled = true;
        try {
          if (box2.checked) await ipc_peer.linkRepo(r.id, p.id);
          else await ipc_peer.unlinkRepo(r.id, p.id);
        } catch (e) {
          box2.checked = !box2.checked;
          toast(`변경 실패: ${(e as Error).message}`, "error");
        } finally {
          box2.disabled = false;
        }
      });
      row.appendChild(box2);
      row.appendChild(text);
      list.appendChild(row);
    }
  }
  await paint();
  return box;
}

/**
 * 알림 수신자. `.gpconfig` 구성원을 그대로 가져오는 동기화 버튼이 핵심 —
 * 사람 목록의 원본은 언제나 `.gpconfig` 한 곳이다.
 */
async function renderRecipients(p: PeerProjectInfo): Promise<HTMLElement> {
  const box = document.createElement("div");
  box.className = "flex flex-col gap-2";

  const label = document.createElement("div");
  label.className = "text-display-sm font-medium";
  label.textContent = "알림 수신자";
  box.appendChild(label);

  const hint = document.createElement("div");
  hint.className = "text-display-sm text-[color:var(--color-ink-muted)]";
  hint.textContent =
    "사람 목록의 원본은 저장소 → 설정 탭의 구성원(.gpconfig)입니다. 여기서는 그 목록을 알림 서버에 반영만 합니다.";
  box.appendChild(hint);

  const list = document.createElement("div");
  list.className = "flex flex-col gap-1";
  box.appendChild(list);

  const row = document.createElement("div");
  row.className = "flex gap-2 flex-wrap";
  const syncBtn = document.createElement("button");
  syncBtn.className = "gc-button-primary text-display-sm";
  syncBtn.textContent = "구성원 동기화";
  row.appendChild(syncBtn);
  const manualBtn = document.createElement("button");
  manualBtn.className = "gc-button-secondary text-display-sm";
  manualBtn.textContent = "직접 추가";
  row.appendChild(manualBtn);
  box.appendChild(row);

  async function paint() {
    list.innerHTML = "";
    let members: MemberInfo[] = [];
    try {
      members = await ipc_peer.listMembers(p.id);
    } catch {
      list.innerHTML = `<div class="text-display-sm text-[color:var(--color-danger)]">수신자 목록을 불러올 수 없습니다.</div>`;
      return;
    }
    if (members.length === 0) {
      list.innerHTML = `<div class="text-display-sm text-[color:var(--color-ink-muted)]">아직 수신자가 없습니다. 아래 ‘구성원 동기화’를 누르세요.</div>`;
      return;
    }
    for (const m of members) {
      const r = document.createElement("div");
      r.className = "flex items-center gap-2 text-display-sm";
      const who = document.createElement("span");
      who.className = "flex-1 min-w-0 truncate";
      who.textContent = m.email
        ? `${m.email}${m.name ? ` · ${m.name}` : ""}`
        : `${m.name ?? m.device_id ?? "(알 수 없음)"}`;
      r.appendChild(who);
      // joined_at 이 비어 있으면 초대만 된 상태 — 아직 이 앱으로 합류하지 않았다.
      if (!m.joined_at) {
        const tag = document.createElement("span");
        tag.className = "gc-badge gc-badge--muted shrink-0";
        tag.textContent = "미합류";
        tag.title = "초대했지만 아직 이 앱으로 합류하지 않았습니다.";
        r.appendChild(tag);
      }
      if (m.email) {
        const del = document.createElement("button");
        del.className = "gc-btn-sm gc-btn-sm--danger shrink-0";
        del.textContent = "제거";
        del.addEventListener("click", async () => {
          const ok = await confirmDialog({
            title: "수신자 제거",
            message: `${m.email} 에게 더 이상 알림을 보내지 않습니다.`,
            confirmLabel: "제거",
            destructive: true,
          });
          if (!ok) return;
          try {
            await ipc_peer.removeEmailInvite(p.id, m.email!);
            await paint();
          } catch (e) {
            toast(`제거 실패: ${(e as Error).message}`, "error");
          }
        });
        r.appendChild(del);
      }
      list.appendChild(r);
    }
  }

  syncBtn.addEventListener("click", async () => {
    setBusy(syncBtn, true, "동기화 중…");
    try {
      const result = await syncMembersFromGpconfig(p.id);
      if (result.total === 0) {
        toast(
          "이 팀에 연결된 저장소의 .gpconfig 에 구성원이 없습니다. 저장소 → 설정 탭에서 먼저 구성원을 등록하세요.",
          "info",
        );
      } else {
        toast(
          `구성원 ${result.total}명 중 ${result.added}명을 수신자로 추가했습니다.`,
          "success",
        );
      }
      await paint();
    } catch (e) {
      toast(`동기화 실패: ${(e as Error).message ?? e}`, "error");
    } finally {
      setBusy(syncBtn, false);
    }
  });

  manualBtn.addEventListener("click", () => openInviteModal(p.id, paint));

  await paint();
  return box;
}

/**
 * 이 팀에 연결된 저장소들의 `.gpconfig` 구성원을 알림 수신자로 등록한다.
 * 이미 수신자인 사람은 건너뛴다.
 */
async function syncMembersFromGpconfig(
  projectId: string,
): Promise<{ total: number; added: number }> {
  const linked = await ipc_peer.reposForProject(projectId);
  const wanted = new Map<string, { email: string; name: string; role: string }>();
  for (const l of linked) {
    const cfg = await ipc.projectConfigGet(l.repo_id).catch(() => null);
    for (const m of cfg?.config?.members ?? []) {
      const email = m.email.trim().toLowerCase();
      if (!email) continue;
      if (!wanted.has(email)) wanted.set(email, { email, name: m.name, role: m.role });
    }
  }
  const existing = new Set(
    (await ipc_peer.listMembers(projectId).catch(() => [] as MemberInfo[]))
      .map((m) => m.email?.trim().toLowerCase())
      .filter((e): e is string => !!e),
  );
  let added = 0;
  for (const m of wanted.values()) {
    if (existing.has(m.email)) continue;
    // 한 명이 실패해도 나머지는 계속 등록한다 — 부분 성공이 전부 실패보다 낫다.
    try {
      await ipc_peer.inviteByEmail(projectId, m.email, m.name || null, m.role || "member");
      added += 1;
    } catch {
      /* 다음 사람으로 */
    }
  }
  return { total: wanted.size, added };
}

// ─── 모달 ────────────────────────────────────────────────────────────────────

function openCreateModal(): void {
  const m = openModal({
    title: "팀 만들기",
    description: "알림을 주고받을 팀입니다. 만들면 팀원에게 알려 줄 참여 코드가 나옵니다.",
    submitLabel: "만들기",
    onSubmit: async (close) => {
      const name = (m.body.querySelector<HTMLInputElement>("#team-name")!).value.trim();
      if (!name) {
        m.setError("팀 이름을 입력하세요.");
        return;
      }
      m.setSubmitting(true);
      m.setError(null);
      try {
        const created = await ipc_peer.createProject(name, null);
        close();
        toast(`팀 "${created.display_name}" 생성 — 참여 코드 ${created.join_code}`, "success");
        window.dispatchEvent(new Event("gc-account-changed"));
      } catch (e) {
        m.setError(`실패: ${(e as Error).message ?? e}`);
        m.setSubmitting(false);
      }
    },
  });
  m.body.innerHTML = `
    <div class="flex flex-col gap-1">
      <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="team-name">팀 이름</label>
      <input id="team-name" class="gc-input" type="text" placeholder="예: 결제팀" />
    </div>
  `;
  m.body.querySelector<HTMLInputElement>("#team-name")!.focus();
}

function openJoinModal(): void {
  const m = openModal({
    title: "참여 코드로 합류",
    description: "팀을 만든 사람에게 받은 참여 코드를 입력하세요.",
    submitLabel: "합류",
    onSubmit: async (close) => {
      const code = (m.body.querySelector<HTMLInputElement>("#join-code")!).value.trim();
      if (!code) {
        m.setError("참여 코드를 입력하세요.");
        return;
      }
      m.setSubmitting(true);
      m.setError(null);
      try {
        const joined = await ipc_peer.joinProject(code, null);
        close();
        toast(`"${joined.display_name}" 팀에 합류했습니다.`, "success");
        window.dispatchEvent(new Event("gc-account-changed"));
      } catch (e) {
        m.setError(`합류 실패: ${(e as Error).message ?? e}`);
        m.setSubmitting(false);
      }
    },
  });
  m.body.innerHTML = `
    <div class="flex flex-col gap-1">
      <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="join-code">참여 코드</label>
      <input id="join-code" class="gc-input font-mono" type="text" placeholder="TEAM-0001" autocomplete="off" />
    </div>
  `;
  m.body.querySelector<HTMLInputElement>("#join-code")!.focus();
}

function openInviteModal(projectId: string, onDone: () => void | Promise<void>): void {
  const m = openModal({
    title: "수신자 직접 추가",
    description:
      "보통은 ‘구성원 동기화’로 충분합니다. .gpconfig 에 없는 사람에게만 알림을 보낼 때 씁니다.",
    submitLabel: "추가",
    onSubmit: async (close) => {
      const email = (m.body.querySelector<HTMLInputElement>("#invite-email")!).value.trim();
      const name = (m.body.querySelector<HTMLInputElement>("#invite-name")!).value.trim();
      if (!email) {
        m.setError("이메일을 입력하세요.");
        return;
      }
      m.setSubmitting(true);
      m.setError(null);
      try {
        await ipc_peer.inviteByEmail(projectId, email, name || null, "member");
        close();
        toast("수신자를 추가했습니다.", "success");
        await onDone();
      } catch (e) {
        m.setError(`추가 실패: ${(e as Error).message ?? e}`);
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
  `;
  m.body.querySelector<HTMLInputElement>("#invite-email")!.focus();
}
