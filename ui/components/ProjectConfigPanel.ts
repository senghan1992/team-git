// 프로젝트 설정 탭 (.gpconfig 관리)
// - 구성원: 가입한(등록된) 계정만 검색 → 추가, 삭제 가능 (새 계정 가입은 불가)
// - 브랜치별 병합 관리자 지정 (그 사람이 해당 브랜치의 커밋/푸시 담당)
// - 알림 수신자 지정 / 기본 베이스 브랜치
// 저장하면 저장소 루트의 `.gpconfig`에 기록되고 자동 커밋되어, 다른 참여자가
// 풀하면 동일한 설정을 보게 된다.
import { ipc, type Account, type ProjectConfig, type Repo } from "../lib/ipc";
import { toast } from "./Toast";
import { setBusy } from "./Busy";
import { openAccountModal } from "./AccountModal";
import { getSession } from "../lib/session";
import { openPushCredentialFlow } from "./PushButton";
import { icon } from "./Icon";

export async function renderProjectConfigPanel(repo: Repo): Promise<HTMLElement> {
  const el = document.createElement("div");
  el.className = "flex flex-col gap-5";

  const summary = await ipc.projectConfigGet(repo.id).catch(() => null);
  const me = getSession();
  const config: ProjectConfig = summary?.config ?? {
    gpconfig_version: 2,
    default_base_branch: repo.default_branch || "main",
    members: [],
    merge_managers: {},
    merge_targets: [],
    notify_recipients: [],
    notify: { on_branch_ready: false, on_merge_complete: false },
  };
  const fileExists = summary?.exists ?? false;

  // 로그아웃 상태에서도 이 탭은 쓸 수 있어야 한다. `.gpconfig` 는 저장소에
  // 커밋되는 파일이고 팀 서버와 아무 상관이 없다 — 예전에는 이 화면 전체를
  // "로그인이 필요합니다" 한 장으로 덮어서, 혼자 쓰는 사람은 병합 대상 브랜치
  // 하나를 정하려고 서버를 띄우고 회원가입을 해야 했다. 로그인이 실제로
  // 필요한 것은 **사람을 이름으로 찾는 일**(팀 서버의 사용자 목록)뿐이다.
  if (!me) {
    const banner = document.createElement("div");
    banner.className = "gc-banner gc-banner--info";
    banner.innerHTML = `<div class="gc-banner__body flex-1 flex flex-col gap-2">
      <div class="gc-banner__title">로그인 없이 쓰는 중입니다</div>
      <div class="text-display-sm text-[color:var(--color-ink-muted)]">
        병합 대상 브랜치와 기본 베이스 브랜치는 지금 그대로 정하고 저장할 수 있습니다
        (저장소의 <code>.gpconfig</code>에 기록됩니다).
        이름으로 팀원을 찾아 구성원으로 추가하는 것만 로그인이 필요합니다.
      </div>
      <div><button class="gc-button-secondary text-display-sm" id="gpc-login">로그인</button></div>
    </div>`;
    banner.querySelector<HTMLButtonElement>("#gpc-login")!.addEventListener("click", () => openAccountModal());
    el.appendChild(banner);
  }

  // 새로고침 (계정 변경 시)
  window.addEventListener("gc-account-changed", () => {
    el.dispatchEvent(new CustomEvent("gc-rerender"));
  });

  // ── 저장 바 (변경 상태 + 저장/푸시) — 섹션보다 먼저 만들어 핸들러가 참조한다 ──
  const saveBar = document.createElement("div");
  saveBar.className = "gc-savebar gc-savebar--sticky";
  const statusLeft = document.createElement("div");
  statusLeft.className = "gc-savebar__status";
  const statusIcon = icon("check", 14);
  statusIcon.style.color = "var(--color-success)";
  const statusLabel = document.createElement("span");
  statusLabel.textContent = "변경 사항이 없습니다";
  statusLeft.appendChild(statusIcon);
  statusLeft.appendChild(statusLabel);

  const actions = document.createElement("div");
  actions.className = "flex items-center gap-2";
  const pushBtn = document.createElement("button");
  pushBtn.className = "gc-button-secondary";
  pushBtn.textContent = "origin에 푸시";
  pushBtn.disabled = true;
  // 원격이 없는 저장소에서는 푸시가 예외 없이 실패한다 — 저장(커밋)까지만 하면
  // 되고, 왜 눌릴 수 없는지 툴팁으로 남긴다. (작업 탭의 푸시·풀과 같은 규칙)
  const noRemote = !repo.remote_url;
  if (noRemote) {
    pushBtn.title =
      "이 저장소에는 원격(origin)이 없어 보낼 곳이 없습니다.\n" +
      ".gpconfig 는 저장(커밋)까지만 해 두면 됩니다.";
  }
  const saveBtn = document.createElement("button");
  saveBtn.className = "gc-button-primary";
  saveBtn.textContent = "저장 (.gpconfig 커밋)";
  actions.appendChild(pushBtn);
  actions.appendChild(saveBtn);
  saveBar.appendChild(statusLeft);
  saveBar.appendChild(actions);

  let dirty = false;
  function markDirty() {
    if (dirty) return;
    dirty = true;
    statusLabel.textContent = "변경 사항이 저장되지 않았습니다";
    statusIcon.style.color = "var(--color-accent-warn)";
    statusIcon.replaceChildren(...icon("edit", 14).childNodes);
  }
  function markClean() {
    dirty = false;
    statusLabel.textContent = "변경 사항이 없습니다";
    statusIcon.style.color = "var(--color-success)";
    statusIcon.replaceChildren(...icon("check", 14).childNodes);
  }

  // ── 소개 ─────────────────────────────────────────────────────────────
  const intro = document.createElement("div");
  intro.className = "gc-card";
  const introRow = document.createElement("div");
  introRow.className = "flex items-center gap-2";
  const introTitle = document.createElement("h2");
  introTitle.className = "text-display-md font-semibold";
  introTitle.textContent = "프로젝트 설정";
  const introBadge = document.createElement("span");
  introBadge.className = fileExists ? "gc-badge gc-badge--success" : "gc-badge gc-badge--muted";
  introBadge.textContent = fileExists ? ".gpconfig 기록됨" : ".gpconfig 아직 없음";
  introRow.appendChild(introTitle);
  introRow.appendChild(introBadge);
  intro.appendChild(introRow);
  const introSub = document.createElement("div");
  introSub.className = "text-display-sm text-[color:var(--color-ink-muted)] mt-1";
  introSub.textContent = fileExists
    ? "이 설정은 저장소 루트의 .gpconfig에 기록됩니다. 다른 참여자도 동일한 설정을 팀원들과 공유합니다."
    : "아직 .gpconfig가 없습니다. 저장하면 이 저장소에 기록되고 커밋되어 팀원들과 공유됩니다.";
  intro.appendChild(introSub);
  el.appendChild(intro);

  // ── 구성원 섹션 ──────────────────────────────────────────────────────
  const memberCard = document.createElement("section");
  memberCard.className = "gc-card flex flex-col gap-4";
  el.appendChild(memberCard);

  const mHead = document.createElement("div");
  mHead.className = "gc-card__head";
  const mHeadRow = document.createElement("div");
  mHeadRow.className = "gc-card__head-row";
  const mTitle = document.createElement("h3");
  mTitle.className = "text-display-md font-medium";
  mTitle.textContent = "구성원";
  const mCount = document.createElement("span");
  mCount.id = "gpc-member-count";
  mCount.className = "gc-badge gc-badge--num";
  mHeadRow.appendChild(mTitle);
  mHeadRow.appendChild(mCount);
  mHead.appendChild(mHeadRow);
  const mDesc = document.createElement("p");
  mDesc.className = "text-display-sm text-[color:var(--color-ink-muted)]";
  mDesc.textContent = "등록된 구성원은 이 프로젝트에서 검색해 추가할 수 있고, 병합 관리자를 맡을 수 있습니다.";
  mHead.appendChild(mDesc);
  memberCard.appendChild(mHead);

  const memberList = document.createElement("div");
  memberList.className = "gc-list";
  memberCard.appendChild(memberList);

  function renderMembers() {
    memberList.innerHTML = "";
    const countEl = memberCard.querySelector<HTMLElement>("#gpc-member-count");
    if (countEl) countEl.textContent = `${config.members.length}명`;
    renderRecipients();
    if (config.members.length === 0) {
      const empty = document.createElement("div");
      empty.className = "gc-empty-inline";
      empty.appendChild(icon("users", 16));
      const t = document.createElement("span");
      t.textContent = me
        ? "아직 구성원이 없습니다. 아래 검색으로 가입한 사람을 추가할 수 있습니다."
        : "아직 구성원이 없습니다. 혼자 쓰는 중이라면 비워 둬도 됩니다 — 병합 대상 브랜치만 정하면 바로 쓸 수 있습니다.";
      empty.appendChild(t);
      memberList.appendChild(empty);
      return;
    }
    for (const member of config.members) {
      const row = document.createElement("div");
      row.className = "gc-list__row";
      const avatar = document.createElement("div");
      avatar.className = "gc-avatar";
      avatar.textContent = member.name.slice(0, 2);
      row.appendChild(avatar);
      const label = document.createElement("div");
      label.className = "flex-1 min-w-0 flex items-center gap-2";
      const name = document.createElement("span");
      name.className = "text-display-sm font-medium truncate";
      name.textContent = member.name;
      label.appendChild(name);
      if (me && member.email === me.email) {
        const meBadge = document.createElement("span");
        meBadge.className = "gc-badge gc-badge--info";
        meBadge.textContent = "나";
        label.appendChild(meBadge);
      }
      const email = document.createElement("div");
      email.className = "text-display-xs text-[color:var(--color-ink-muted)] truncate";
      email.textContent = member.email;
      label.appendChild(email);
      row.appendChild(label);
      const roleSel = document.createElement("select");
      roleSel.className = "gc-input gc-input--sm w-28 text-display-sm";
      roleSel.setAttribute("aria-label", `${member.name} 역할`);
      // 화면 전체가 한국어인데 이 칸만 member/admin 이라 무슨 차이인지 읽히지
      // 않았다. 값(저장되는 문자열)은 그대로 두고 보이는 말만 바꾼다.
      roleSel.innerHTML = `<option value="member">구성원</option><option value="admin">관리자</option>`;
      roleSel.title = "관리자는 병합 관리자가 따로 지정된 브랜치에도 병합할 수 있습니다.";
      roleSel.value = member.role;
      roleSel.addEventListener("change", () => {
        member.role = roleSel.value;
        markDirty();
      });
      row.appendChild(roleSel);
      const del = document.createElement("button");
      del.className = "gc-btn-sm gc-btn-sm--danger";
      del.appendChild(icon("trash", 13));
      const delLabel = document.createElement("span");
      delLabel.textContent = "삭제";
      del.appendChild(delLabel);
      del.addEventListener("click", () => {
        config.members = config.members.filter((x) => x.email !== member.email);
        config.merge_managers = Object.fromEntries(
          Object.entries(config.merge_managers).filter(([, e]) => e !== member.email),
        );
        config.notify_recipients = config.notify_recipients.filter((e) => e !== member.email);
        renderMembers();
        renderTargets();
        markDirty();
      });
      row.appendChild(del);
      memberList.appendChild(row);
    }
  }

  // 검색 (등록된 계정에서 아직 구성원이 아닌 사람)
  const searchField = document.createElement("div");
  searchField.className = "gc-field";
  const searchLabel = document.createElement("label");
  searchLabel.className = "gc-input-label";
  searchLabel.htmlFor = "gpc-search";
  searchLabel.textContent = "구성원 검색";
  const searchWrap = document.createElement("div");
  searchWrap.className = "gc-search";
  searchWrap.appendChild(icon("search", 14));
  const searchInput = document.createElement("input");
  searchInput.id = "gpc-search";
  searchInput.className = "gc-input";
  searchInput.type = "search";
  searchInput.placeholder = "이름 또는 이메일";
  searchWrap.appendChild(searchInput);
  searchField.appendChild(searchLabel);
  searchField.appendChild(searchWrap);

  const results = document.createElement("div");
  results.className = "gc-list";

  // 검색만 팀 서버를 본다 — 로그아웃이면 검색칸 대신 이유를 적어 둔다.
  // 빈 검색칸을 놔두고 아무 결과도 안 주면 고장 난 것처럼 보인다.
  if (me) {
    memberCard.appendChild(searchField);
    memberCard.appendChild(results);
  } else {
    const note = document.createElement("div");
    note.className = "gc-empty-inline";
    note.appendChild(icon("users", 16));
    const t = document.createElement("span");
    t.textContent = "구성원 검색은 팀 서버의 계정 목록을 봅니다 — 로그인하면 이름으로 팀원을 찾아 추가할 수 있습니다.";
    note.appendChild(t);
    const go = document.createElement("button");
    go.className = "gc-btn-sm";
    go.textContent = "로그인";
    go.addEventListener("click", () => openAccountModal());
    note.appendChild(go);
    memberCard.appendChild(note);
  }

  /** 검색 요청 순서가 뒤바뀌어 옛 결과가 새 결과를 덮는 것을 막는다. */
  let searchSeq = 0;

  function inlineNote(text: string) {
    results.innerHTML = "";
    const box = document.createElement("div");
    box.className = "gc-empty-inline";
    box.appendChild(icon("users", 16));
    const t = document.createElement("span");
    t.textContent = text;
    box.appendChild(t);
    results.appendChild(box);
  }

  async function renderSearch(query: string) {
    const q = query.trim();
    const seq = ++searchSeq;
    if (q.length < 2) {
      results.innerHTML = "";
      return;
    }
    // 구성원 검색은 팀 서버의 사용자 디렉터리를 본다. 예전에는 이 컴퓨터의
    // 로컬 계정 파일을 뒤졌기 때문에, 여기서 로그인한 적 없는 팀원은 아무리
    // 검색해도 나오지 않았다.
    let accounts: Account[];
    try {
      accounts = await ipc.accountSearch(q);
    } catch (e) {
      if (seq !== searchSeq) return;
      inlineNote(`검색 실패: ${(e as Error).message ?? e}`);
      return;
    }
    if (seq !== searchSeq) return; // 더 최신 검색이 진행 중
    const found = accounts.filter(
      (a) => !config.members.some((x) => x.email.toLowerCase() === a.email.toLowerCase()),
    );
    results.innerHTML = "";
    if (found.length === 0) {
      inlineNote("검색 결과가 없습니다. 팀원이 먼저 회원가입해야 찾을 수 있습니다.");
      return;
    }
    for (const a of found) {
      const row = document.createElement("div");
      row.className = "gc-list__row";
      const avatar = document.createElement("div");
      avatar.className = "gc-avatar";
      avatar.textContent = a.name.slice(0, 2);
      row.appendChild(avatar);
      const label = document.createElement("span");
      label.className = "flex-1 truncate text-display-sm";
      label.textContent = `${a.name} (${a.email})`;
      row.appendChild(label);
      const add = document.createElement("button");
      add.className = "gc-btn-sm";
      add.appendChild(icon("plus", 13));
      const addLabel = document.createElement("span");
      addLabel.textContent = "추가";
      add.appendChild(addLabel);
      add.addEventListener("click", () => {
        config.members.push({ id: a.id, name: a.name, email: a.email, role: "member" });
        renderMembers();
        renderTargets();
        void renderSearch(q);
        markDirty();
      });
      row.appendChild(add);
      results.appendChild(row);
    }
  }
  // 타이핑마다 서버를 부르지 않도록 잠깐 기다린다.
  let searchTimer: number | undefined;
  searchInput.addEventListener("input", () => {
    window.clearTimeout(searchTimer);
    searchTimer = window.setTimeout(() => void renderSearch(searchInput.value), 250);
  });

  // ── 병합 대상 브랜치 섹션 ────────────────────────────────────────────
  // 이 브랜치들로만 병합할 수 있다. main 외에도 원하는 브랜치를 자유롭게
  // 지정할 수 있고, 언제든 바꿔서 저장(.gpconfig 커밋)할 수 있다.
  const targetCard = document.createElement("section");
  targetCard.className = "gc-card flex flex-col gap-4";
  el.appendChild(targetCard);

  const tHead = document.createElement("div");
  tHead.className = "gc-card__head";
  const tHeadRow = document.createElement("div");
  tHeadRow.className = "gc-card__head-row";
  const tTitle = document.createElement("h3");
  tTitle.className = "text-display-md font-medium";
  tTitle.textContent = "병합 대상 브랜치";
  const tCount = document.createElement("span");
  tCount.id = "gpc-target-count";
  tCount.className = "gc-badge gc-badge--num";
  tHeadRow.appendChild(tTitle);
  tHeadRow.appendChild(tCount);
  tHead.appendChild(tHeadRow);
  const tSub = document.createElement("p");
  tSub.className = "text-display-sm text-[color:var(--color-ink-muted)]";
  tSub.textContent =
    "이 브랜치들로만 병합할 수 있습니다 (main 외에도 release/1.0 같은 브랜치를 지정할 수 있습니다). 각 브랜치의 병합 관리자는 그 브랜치로의 병합·푸시를 담당합니다.";
  tHead.appendChild(tSub);
  targetCard.appendChild(tHead);

  // 기본 베이스 브랜치 — 병합 대상의 기준이 되는 브랜치
  const baseRow = document.createElement("div");
  baseRow.className = "flex items-center gap-3 flex-wrap";
  const baseLabel = document.createElement("label");
  baseLabel.className = "gc-input-label w-32 shrink-0";
  baseLabel.htmlFor = "gpc-base";
  baseLabel.textContent = "기본 베이스 브랜치";
  const baseInput = document.createElement("input");
  baseInput.id = "gpc-base";
  baseInput.className = "gc-input w-52 font-mono";
  baseInput.value = config.default_base_branch || repo.default_branch || "main";
  baseInput.addEventListener("input", () => {
    config.default_base_branch = baseInput.value.trim();
    markDirty();
  });
  baseRow.appendChild(baseLabel);
  baseRow.appendChild(baseInput);
  targetCard.appendChild(baseRow);

  const targetDivider = document.createElement("div");
  targetDivider.className = "gc-hdivider";
  targetCard.appendChild(targetDivider);

  const targetList = document.createElement("div");
  targetList.className = "flex flex-col gap-2";
  targetCard.appendChild(targetList);

  function renderTargets() {
    targetList.innerHTML = "";
    const countEl = targetCard.querySelector<HTMLElement>("#gpc-target-count");
    if (countEl) countEl.textContent = `${config.merge_targets.length}개`;
    if (config.merge_targets.length === 0) {
      const empty = document.createElement("div");
      empty.className = "gc-empty-inline";
      empty.appendChild(icon("branch", 16));
      const t = document.createElement("span");
      t.textContent = `병합 대상이 지정되지 않았습니다. 기본 베이스 브랜치(${config.default_base_branch || "main"})로만 병합할 수 있습니다.`;
      empty.appendChild(t);
      targetList.appendChild(empty);
      return;
    }
    config.merge_targets.forEach((branch, i) => {
      const group = document.createElement("div");
      group.className = "gc-rowgroup";
      const input = document.createElement("input");
      input.className = "gc-input--ghost font-mono flex-1";
      input.value = branch;
      input.setAttribute("aria-label", `병합 대상 브랜치 ${i + 1}`);
      input.addEventListener("change", () => {
        const v = input.value.trim();
        if (!v || v === branch) return;
        // 브랜치 이름을 바꾸면 병합 관리자 매핑도 따라간다.
        const mgr = config.merge_managers[branch];
        if (mgr) {
          delete config.merge_managers[branch];
          config.merge_managers[v] = mgr;
        }
        config.merge_targets[i] = v;
        renderTargets();
        markDirty();
      });
      group.appendChild(input);
      const vdiv = document.createElement("div");
      vdiv.className = "gc-vdivider";
      group.appendChild(vdiv);
      const sel = document.createElement("select");
      sel.className = "gc-input--ghost";
      sel.setAttribute("aria-label", `${branch} 병합 관리자`);
      // 내가 구성원이 아니라도 나 자신을 병합 관리자로 지정할 수 있어야 한다.
      const meNotMember = me && !config.members.some((x) => x.email === me.email);
      sel.innerHTML =
        `<option value="">관리자 지정 안 함</option>` +
        (meNotMember ? `<option value="${escape(me!.email)}">나 (${escape(me!.email)})</option>` : "") +
        config.members
          .map((x) => {
            const isMe = me && x.email === me.email;
            return `<option value="${escape(x.email)}">${escape(x.name)}${isMe ? " (나)" : ""} (${escape(x.email)})</option>`;
          })
          .join("");
      sel.value = config.merge_managers[branch] ?? "";
      sel.addEventListener("change", () => {
        if (sel.value) config.merge_managers[branch] = sel.value;
        else delete config.merge_managers[branch];
        markDirty();
      });
      group.appendChild(sel);
      const del = document.createElement("button");
      del.className = "gc-btn-sm gc-btn-sm--danger";
      del.appendChild(icon("trash", 13));
      const delLabel = document.createElement("span");
      delLabel.textContent = "삭제";
      del.appendChild(delLabel);
      del.addEventListener("click", () => {
        config.merge_targets = config.merge_targets.filter((_, j) => j !== i);
        delete config.merge_managers[branch];
        renderTargets();
        markDirty();
      });
      group.appendChild(del);
      targetList.appendChild(group);
    });
  }

  const addTargetRow = document.createElement("div");
  addTargetRow.className = "flex items-center gap-2";
  const targetField = document.createElement("div");
  targetField.className = "gc-field flex-1";
  const targetLabel = document.createElement("label");
  targetLabel.className = "gc-input-label";
  targetLabel.htmlFor = "gpc-target";
  targetLabel.textContent = "새 병합 대상 추가";
  const targetInput = document.createElement("input");
  targetInput.id = "gpc-target";
  targetInput.className = "gc-input font-mono";
  targetInput.placeholder = "예: release/1.0";
  targetField.appendChild(targetLabel);
  targetField.appendChild(targetInput);
  const addTargetBtn = document.createElement("button");
  addTargetBtn.id = "gpc-add-target";
  addTargetBtn.className = "gc-button-secondary shrink-0 self-end";
  addTargetBtn.appendChild(icon("plus", 14));
  // 라벨은 gc-button 기본 크기(본문 14px)를 그대로 쓴다 — 입력칸 높이(40px)와
  // 짝이 맞고, 위쪽 목록 행의 작은 액션(gc-btn-sm, 13px)과 위계가 갈린다.
  const addTargetLabel = document.createElement("span");
  addTargetLabel.textContent = "대상 추가";
  addTargetBtn.appendChild(addTargetLabel);
  addTargetBtn.addEventListener("click", () => {
    const v = targetInput.value.trim();
    if (!v) {
      toast("브랜치 이름을 입력하세요.", "error");
      return;
    }
    if (config.merge_targets.includes(v)) {
      toast(`이미 병합 대상입니다: ${v}`, "info");
      return;
    }
    config.merge_targets.push(v);
    targetInput.value = "";
    renderTargets();
    markDirty();
    toast(`${v} 브랜치를 병합 대상으로 추가했습니다. 저장(.gpconfig 커밋)하면 병합 센터에 반영됩니다.`, "success");
  });
  addTargetRow.appendChild(targetField);
  addTargetRow.appendChild(addTargetBtn);
  targetCard.appendChild(addTargetRow);

  // ── 알림 섹션 ─────────────────────────────────────────────────────────
  const notifyCard = document.createElement("section");
  notifyCard.className = "gc-card flex flex-col gap-4";
  el.appendChild(notifyCard);

  const nHead = document.createElement("div");
  nHead.className = "gc-card__head";
  const nTitle = document.createElement("h3");
  nTitle.className = "text-display-md font-medium";
  nTitle.textContent = "알림";
  nHead.appendChild(nTitle);
  const nDesc = document.createElement("p");
  nDesc.className = "text-display-sm text-[color:var(--color-ink-muted)]";
  nDesc.textContent = "브랜치가 병합 준비되거나 병합이 완료되면 어느 사람에게 알림을 보낼지 정합니다.";
  nHead.appendChild(nDesc);
  notifyCard.appendChild(nHead);

  const recLabel = document.createElement("div");
  recLabel.className = "gc-input-label";
  recLabel.textContent = "알림을 받을 사람";
  notifyCard.appendChild(recLabel);

  const recipients = document.createElement("div");
  recipients.className = "gc-list";
  notifyCard.appendChild(recipients);

  function renderRecipients() {
    recipients.innerHTML = "";
    if (config.members.length === 0) {
      const empty = document.createElement("div");
      empty.className = "gc-empty-inline";
      empty.appendChild(icon("inbox", 16));
      const t = document.createElement("span");
      t.textContent = "구성원을 먼저 추가하세요.";
      empty.appendChild(t);
      recipients.appendChild(empty);
      return;
    }
    for (const member of config.members) {
      const row = document.createElement("label");
      row.className = "gc-list__row cursor-pointer";
      const cb = document.createElement("input");
      cb.type = "checkbox";
      cb.className = "accent-[color:var(--color-primary)]";
      cb.checked = config.notify_recipients.includes(member.email);
      cb.addEventListener("change", () => {
        if (cb.checked) {
          if (!config.notify_recipients.includes(member.email)) config.notify_recipients.push(member.email);
        } else {
          config.notify_recipients = config.notify_recipients.filter((e) => e !== member.email);
        }
        markDirty();
      });
      row.appendChild(cb);
      const t = document.createElement("span");
      t.className = "text-display-sm";
      t.textContent = member.name;
      row.appendChild(t);
      recipients.appendChild(row);
    }
  }
  renderRecipients();

  const nDivider = document.createElement("div");
  nDivider.className = "gc-hdivider";
  notifyCard.appendChild(nDivider);

  const flagLabel = document.createElement("div");
  flagLabel.className = "gc-input-label";
  flagLabel.textContent = "알림 시점";
  notifyCard.appendChild(flagLabel);

  const notifyFlags = document.createElement("div");
  notifyFlags.className = "gc-list";
  for (const [key, label] of [
    ["on_branch_ready", "브랜치의 머지 준비가 되었을 때 병합 관리자에게 알림"],
    ["on_merge_complete", "병합이 완료되었을 때 병합 관리자에게 알림"],
  ] as const) {
    const row = document.createElement("label");
    row.className = "gc-list__row cursor-pointer";
    const cb = document.createElement("input");
    cb.type = "checkbox";
    cb.className = "accent-[color:var(--color-primary)]";
    cb.checked = config.notify[key];
    cb.addEventListener("change", () => {
      config.notify[key] = cb.checked;
      markDirty();
    });
    row.appendChild(cb);
    const t = document.createElement("span");
    t.className = "text-display-sm";
    t.textContent = label;
    row.appendChild(t);
    notifyFlags.appendChild(row);
  }
  notifyCard.appendChild(notifyFlags);

  // ── 저장 ─────────────────────────────────────────────────────────────
  el.appendChild(saveBar);

  saveBtn.addEventListener("click", async () => {
    setBusy(saveBtn, true, "저장 중…");
    try {
      const result = await ipc.projectConfigSet(repo.id, config, true);
      const committed = result.commit?.ok;
      // 서버 정규화(나 자동 추가 등) 결과를 패널 상태에 반영한다.
      Object.assign(config, result.config);
      renderMembers();
      renderTargets();
      markClean();
      if (committed) {
        toast("설정을 저장하고 .gpconfig를 커밋했습니다.", "success");
        pushBtn.disabled = noRemote;
      } else {
        toast(".gpconfig 파일은 저장했지만 커밋에 실패했습니다: " + (result.commit?.message ?? ""), "error");
      }
    } catch (e) {
      toast(`저장 실패: ${(e as Error).message ?? e}`, "error");
    } finally {
      setBusy(saveBtn, false);
    }
  });

  pushBtn.addEventListener("click", async () => {
    setBusy(pushBtn, true, "푸시 중…");
    try {
      const outcome = await openPushCredentialFlow(repo, null);
      if (outcome === "ok") toast(".gpconfig 푸시 완료. 팀원들이 풀하면 동일한 설정을 볼 수 있습니다.", "success");
      else if (outcome === "cancelled") toast("푸시를 취소했습니다.", "info");
      // 예전에는 결과 객체를 그대로 넣어 "푸시 실패: [object Object]" 가 떴다.
      else toast(`푸시 실패: ${outcome.message || "알 수 없는 오류"}`, "error");
    } finally {
      setBusy(pushBtn, false);
    }
  });

  renderMembers();
  renderTargets();
  return el;
}

export function escape(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}