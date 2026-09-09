"use strict";
/* ── 공용 상태 ─────────────────────────────────────────────────────────── */
const TOKEN_KEY = "gc-dash-token";
const $ = (id) => document.getElementById(id);
const state = { tab: "overview", me: null, data: {} };
let inflight = false;

/* ── 조각들 ────────────────────────────────────────────────────────────── */
function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}
function rel(iso) {
  if (!iso) return "—";
  const s = Math.max(0, (Date.now() - new Date(iso).getTime()) / 1000);
  if (s < 60) return "방금";
  if (s < 3600) return Math.floor(s / 60) + "분 전";
  if (s < 86400) return Math.floor(s / 3600) + "시간 전";
  return Math.floor(s / 86400) + "일 전";
}
function laneColor(id) {
  // 앱과 같은 6색 차선 — 같은 사람이면 어느 화면에서든 같은 색.
  const lanes = ["#2c4b8f", "#3e7a5e", "#9a6b26", "#66758f", "#b04938", "#7c7466"];
  const n = parseInt((id || "0").slice(0, 2), 16);
  return lanes[(isNaN(n) ? 0 : n) % 6];
}
function toast(text) {
  const t = $("toast");
  t.textContent = text;
  t.classList.add("is-on");
  clearTimeout(toast._h);
  toast._h = setTimeout(() => t.classList.remove("is-on"), 2600);
}
function confirmBox(title, msg, okLabel, danger) {
  return new Promise((resolve) => {
    const d = $("confirm");
    $("cf-title").textContent = title;
    $("cf-msg").textContent = msg;
    const ok = $("cf-ok"), cancel = $("cf-cancel");
    ok.textContent = okLabel || "확인";
    ok.className = danger ? "btn btn--danger" : "btn btn--primary";
    const done = (v) => { d.close(); ok.onclick = cancel.onclick = null; resolve(v); };
    ok.onclick = () => done(true);
    cancel.onclick = () => done(false);
    d.oncancel = (e) => { e.preventDefault(); done(false); };
    d.showModal();
    ok.focus();
  });
}

/* ── API — 같은 서버 같은 출처라 상대 경로로 충분하다 ──────────────────── */
async function api(path, { method = "GET", body, token = sessionStorage.getItem(TOKEN_KEY) } = {}) {
  const headers = {};
  if (token) headers.Authorization = "Bearer " + token;
  if (body !== undefined) headers["Content-Type"] = "application/json";
  let res;
  try {
    res = await fetch(path, { method, headers, body: body !== undefined ? JSON.stringify(body) : undefined });
  } catch {
    throw new Error("서버에 연결할 수 없습니다. 네트워크를 확인하세요.");
  }
  if (res.status === 401 && token) {
    // 살아 있던 세션이 만료된 것 — 게이트로 돌려 보낸다. 단 로그인 시도 자체의
    // 401(비밀번호 틀림)은 세션 만료가 아니므로 token 이 있을 때만 여기로 온다.
    toGate("세션이 만료되었습니다. 다시 로그인하세요.");
    throw new Error("unauthorized");
  }
  let data = null;
  const text = await res.text();
  if (text) { try { data = JSON.parse(text); } catch { /* 빈 응답 */ } }
  if (!res.ok) {
    const msg = (data && data.detail) ? data.detail : "요청 실패 (" + res.status + ")";
    const err = new Error(msg);
    err.status = res.status;
    throw err;
  }
  return data;
}

/* ── 게이트 ────────────────────────────────────────────────────────────── */
function toGate(message) {
  sessionStorage.removeItem(TOKEN_KEY);
  state.me = null;
  $("dash").hidden = true;
  $("gate").hidden = false;
  $("btn-refresh").hidden = true;
  $("btn-logout").hidden = true;
  $("stamp").hidden = true;
  const err = $("gate-error");
  if (message) { err.textContent = message; err.classList.add("is-on"); }
  else err.classList.remove("is-on");
}
function toDash(me) {
  state.me = me;
  $("gate").hidden = true;
  $("dash").hidden = false;
  $("btn-refresh").hidden = false;
  $("btn-logout").hidden = false;
  $("stamp").hidden = false;
  $("head-sub").textContent = me.name + "(" + me.username + ") — 관리자";
  $("dash").classList.remove("panel-enter");
  void $("dash").offsetWidth; // 애니메이션 재시작
  $("dash").classList.add("panel-enter");
  refreshAll();
}

$("gate-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const btn = $("g-submit");
  const id = $("g-id").value.trim();
  const pw = $("g-pw").value;
  if (!id || !pw) return;
  btn.disabled = true;
  btn.textContent = "로그인 중…";
  try {
    const res = await api("/auth/login", { method: "POST", body: { username: id, password: pw }, token: null });
    if (!res.user || !res.user.is_admin) {
      // 관리자가 아닌 계정으로 열린 토큰은 즉시 거둔다 — 유효한 세션을 남겨 두지 않는다.
      await api("/auth/logout", { method: "POST", token: res.token }).catch(() => undefined);
      throw new Error("관리자 계정이 아닙니다. 서버 운영자에게 관리자 지정을 요청하세요.");
    }
    sessionStorage.setItem(TOKEN_KEY, res.token);
    toDash(res.user);
  } catch (err) {
    if (err.message !== "unauthorized") {
      const box = $("gate-error");
      box.textContent = err.message;
      box.classList.add("is-on");
    }
  } finally {
    btn.disabled = false;
    btn.textContent = "로그인";
    $("g-pw").value = "";
  }
});

$("btn-logout").addEventListener("click", async () => {
  const token = sessionStorage.getItem(TOKEN_KEY);
  if (token) await api("/auth/logout", { method: "POST", token }).catch(() => undefined);
  toGate();
});
$("btn-refresh").addEventListener("click", () => refreshAll(true));

/* ── 불러오기 & 그리기 ─────────────────────────────────────────────────── */
async function refreshAll(manual) {
  if (inflight) return;
  inflight = true;
  const dot = $("live-dot");
  dot.classList.remove("is-off", "is-on");
  try {
    // 매 번 새로 읽는다 — 운영 화면은 조금 낡은 수치도 거짓말이 된다.
    const [overview, users, projects, events] = await Promise.all([
      api("/admin/overview"),
      api("/admin/users"),
      api("/admin/projects"),
      api("/admin/events?limit=60"),
    ]);
    state.data = { overview, users, projects, events };
    $("stamp-text").textContent = "방금 갱신 · " + new Date().toLocaleTimeString("ko-KR", { hour: "2-digit", minute: "2-digit" });
    dot.classList.add("is-on");
    paintCounts();
    render();
  } catch (err) {
    if (err.status === 403) { toGate("관리자 계정이 아닙니다. 서버 운영자에게 관리자 지정을 요청하세요."); return; }
    if (err.message === "unauthorized") return;
    dot.classList.add("is-off");
    toast(err.message);
  } finally {
    inflight = false;
  }
}

function paintCounts() {
  const d = state.data;
  $("cnt-users").textContent = d.users ? String(d.users.length) : "";
  $("cnt-projects").textContent = d.projects ? String(d.projects.length) : "";
  $("cnt-events").textContent = d.events ? String(d.events.length) : "";
}

function render() {
  const panel = $("panel");
  panel.innerHTML = "";
  panel.classList.remove("panel-enter");
  void panel.offsetWidth;
  panel.classList.add("panel-enter");
  if (state.tab === "overview") renderOverview(panel);
  else if (state.tab === "users") renderUsers(panel);
  else if (state.tab === "projects") renderProjects(panel);
  else renderEvents(panel);
}

/* ── 개요 ──────────────────────────────────────────────────────────────── */
function renderOverview(panel) {
  const o = state.data.overview;
  if (!o) return;
  const ledger = el("div", "plaque ledger");
  const cells = [
    ["사용자", String(o.users), o.disabled_users > 0 ? "정지 " + o.disabled_users + "명 포함" : "전체 정상", o.disabled_users > 0],
    ["로그인 세션", String(o.sessions), "기기 " + o.devices + "대", false],
    ["프로젝트", String(o.projects), o.projects > 0 ? "연동된 팀 프로젝트" : "아직 없음", false],
    ["24시간 활동", String(o.events_24h), "주간 " + o.events_7d + "건", false],
  ];
  for (const [label, value, note, warn] of cells) {
    const c = el("div", "ledger-cell");
    c.appendChild(el("div", "ledger-label", label));
    const v = el("div", "ledger-value" + (warn ? " is-warn" : ""), value);
    c.appendChild(v);
    c.appendChild(el("div", "ledger-note", note));
    ledger.appendChild(c);
  }
  panel.appendChild(ledger);

  if (o.disabled_users > 0) {
    const b = el("div", "banner banner--warn");
    b.appendChild(el("span", null, "정지된 계정 " + o.disabled_users + "명이 있습니다 — 사용자 탭에서 확인하세요."));
    panel.appendChild(b);
  }

  const head = el("div", "section-head");
  head.appendChild(el("div", "section-title", "최근 활동"));
  const more = el("button", "link-btn", "활동 전체 보기");
  more.addEventListener("click", () => selectTab("events"));
  head.appendChild(more);
  panel.appendChild(head);

  const list = el("div", "plaque");
  const recent = (state.data.events || []).slice(0, 6);
  if (recent.length === 0) {
    list.appendChild(emptyBox("아직 팀 활동이 없습니다", "팀원이 앱에서 push하면 여기에 기록됩니다."));
  } else {
    recent.forEach((ev, i) => list.appendChild(feedRow(ev, i === 0)));
  }
  panel.appendChild(list);
}

/* ── 사용자 ────────────────────────────────────────────────────────────── */
function renderUsers(panel) {
  const users = state.data.users || [];
  const wrap = el("div", "plaque table-wrap");
  if (users.length === 0) {
    wrap.appendChild(emptyBox("가입한 사용자가 없습니다", "팀원이 앱에서 가입하면 이 목록에 나타납니다."));
    panel.appendChild(wrap);
    return;
  }
  const table = el("table");
  const thead = el("thead");
  const hr = el("tr");
  for (const h of ["사용자", "이메일", "마지막 활동", "마지막 로그인", "세션·기기", "프로젝트", ""]) hr.appendChild(el("th", null, h));
  // 마지막 열(관리)은 빈 제목 — 행의 오른쪽 끝이라는 위치가 말해 준다.
  hr.lastChild.removeAttribute("aria-label");
  thead.appendChild(hr);
  table.appendChild(thead);
  const tbody = el("tbody");
  for (const u of users) tbody.appendChild(userRow(u));
  table.appendChild(tbody);
  wrap.appendChild(table);
  panel.appendChild(wrap);
}

function userRow(u) {
  const tr = el("tr");
  const tdUser = el("td");
  const cu = el("div", "cell-user");
  const av = el("span", "avatar", (u.name || "?").trim().charAt(0).toUpperCase());
  av.style.background = laneColor(u.id);
  cu.appendChild(av);
  const nameWrap = el("div");
  const nameLine = el("div", "cell-name", u.name);
  if (u.is_admin) nameLine.appendChild(el("span", "badge badge--cobalt", "관리자"));
  if (u.disabled) nameLine.appendChild(el("span", "badge badge--copper", "정지"));
  nameWrap.appendChild(nameLine);
  nameWrap.appendChild(el("div", "cell-email", u.email));
  cu.appendChild(nameWrap);
  tdUser.appendChild(cu);
  tr.appendChild(tdUser);

  tr.appendChild(el("td", "cell-email", u.email));
  tr.appendChild(el("td", "cell-dim", rel(u.last_seen)));
  tr.appendChild(el("td", "cell-dim", rel(u.last_login_at)));
  tr.appendChild(el("td", "num", u.sessions + "개 · " + u.devices + "대"));
  tr.appendChild(el("td", "num", String(u.projects)));

  const tdAct = el("td");
  const acts = el("div", "cell-actions");
  const isSelf = state.me && state.me.id === u.id;
  const bStatus = el("button", "btn btn--sm" + (u.disabled ? "" : " btn--danger"), u.disabled ? "정지 해제" : "계정 정지");
  if (isSelf) {
    bStatus.disabled = true;
    bStatus.title = "자기 계정은 정지할 수 없습니다.";
  } else {
    bStatus.addEventListener("click", () => toggleUser(u));
  }
  acts.appendChild(bStatus);
  const bOut = el("button", "btn btn--sm", "강제 로그아웃");
  bOut.disabled = u.sessions === 0;
  bOut.title = u.sessions === 0 ? "활성 세션이 없습니다." : "모든 세션 토큰을 지웁니다";
  bOut.addEventListener("click", () => forceLogout(u));
  acts.appendChild(bOut);
  tdAct.appendChild(acts);
  tr.appendChild(tdAct);
  return tr;
}

async function toggleUser(u) {
  const next = !u.disabled;
  const ok = await confirmBox(
    next ? "계정 정지" : "정지 해제",
    next
      ? u.name + " (" + u.email + ") 계정을 정지합니다.\n로그인·세션·기기 알림 수신이 즉시 막히고, 세션은 모두 삭제됩니다."
      : u.name + " (" + u.email + ") 계정의 정지를 해제합니다. 다시 로그인할 수 있습니다.",
    next ? "정지" : "해제",
    next
  );
  if (!ok) return;
  try {
    await api("/admin/users/" + u.id + "/status", { method: "POST", body: { disabled: next } });
    toast(next ? u.name + " 계정을 정지했습니다." : u.name + " 계정의 정지를 해제했습니다.");
    refreshAll();
  } catch (err) { if (err.message !== "unauthorized") toast("처리 실패: " + err.message); }
}

async function forceLogout(u) {
  const ok = await confirmBox(
    "강제 로그아웃",
    u.name + " (" + u.email + ") 의 모든 세션 토큰을 지웁니다.\n다음 요청부터 다시 로그인해야 합니다.",
    "로그아웃",
    false
  );
  if (!ok) return;
  try {
    const r = await api("/admin/users/" + u.id + "/logout", { method: "POST" });
    toast("세션 " + (r.sessions_revoked ?? 0) + "개를 지웠습니다.");
    refreshAll();
  } catch (err) { if (err.message !== "unauthorized") toast("처리 실패: " + err.message); }
}

/* ── 프로젝트 ──────────────────────────────────────────────────────────── */
function renderProjects(panel) {
  const projects = state.data.projects || [];
  if (projects.length === 0) {
    const p = el("div", "plaque");
    p.appendChild(emptyBox("연동된 프로젝트가 없습니다", "팀원이 앱에서 팀 만들기 또는 참여 코드 합류를 하면 나타납니다."));
    panel.appendChild(p);
    return;
  }
  const plaque = el("div", "plaque");
  for (const p of projects) plaque.appendChild(projRow(p));
  panel.appendChild(plaque);
}

function projRow(p) {
  const row = el("div", "proj");
  const head = el("div", "proj-head");
  head.appendChild(el("div", "proj-name", p.display_name));
  head.appendChild(el("span", "badge " + (p.events_24h > 0 ? "badge--celadon" : "badge--muted"),
    p.events_24h > 0 ? "활발 · 24시간 " + p.events_24h + "건" : "조용함"));
  const del = el("button", "btn btn--sm btn--danger", "프로젝트 삭제");
  del.addEventListener("click", () => deleteProject(p));
  head.appendChild(del);
  row.appendChild(head);
  row.appendChild(el("div", "proj-meta",
    "멤버 " + p.member_count + "명 · 누적 이벤트 " + p.events_total + "건 · 마지막 활동 " + rel(p.last_event_at)));

  const list = el("div", "members");
  if (p.members.length === 0) {
    list.appendChild(el("div", "member", "연결된 기기가 없습니다"));
  }
  for (const m of p.members) {
    const mrow = el("div", "member" + (m.role === "owner" ? " is-owner" : ""));
    mrow.appendChild(el("span", "member-dot"));
    const label = el("span");
    const name = el("span", "member-name", m.user_name ?? m.device_name);
    label.appendChild(name);
    if (m.email) label.appendChild(document.createTextNode(" (" + m.email + ")"));
    label.appendChild(document.createTextNode(" · " + (m.role === "owner" ? "소유자" : "팀원") + " · 마지막 접속 " + rel(m.last_seen)));
    mrow.appendChild(label);
    if (m.role !== "owner") {
      const rm = el("button", "link-btn", "제거");
      rm.addEventListener("click", () => removeMember(p, m));
      mrow.appendChild(rm);
    }
    list.appendChild(mrow);
  }
  row.appendChild(list);
  return row;
}

async function removeMember(p, m) {
  const name = m.user_name ?? m.device_name;
  const ok = await confirmBox(
    "멤버 제거",
    p.display_name + " 프로젝트에서 " + name + " 의 기기를 제거합니다.\n이 프로젝트의 이벤트를 더 이상 받지 못합니다.",
    "제거",
    true
  );
  if (!ok) return;
  try {
    await api("/admin/projects/" + p.id + "/members/" + m.device_id, { method: "DELETE" });
    toast(name + " 을(를) " + p.display_name + " 에서 제거했습니다.");
    refreshAll();
  } catch (err) { if (err.message !== "unauthorized") toast("제거 실패: " + err.message); }
}

async function deleteProject(p) {
  const ok = await confirmBox(
    "프로젝트 삭제",
    p.display_name + " 프로젝트를 삭제합니다.\n멤버·초대·이벤트 기록이 모두 지워지고 팀원에게 더 이상 알림이 가지 않습니다.\n저장소 파일과 커밋은 건드리지 않습니다. 되돌릴 수 없습니다.",
    "삭제",
    true
  );
  if (!ok) return;
  try {
    await api("/admin/projects/" + p.id, { method: "DELETE" });
    toast(p.display_name + " 프로젝트를 삭제했습니다.");
    refreshAll();
  } catch (err) { if (err.message !== "unauthorized") toast("삭제 실패: " + err.message); }
}

/* ── 활동 피드 ─────────────────────────────────────────────────────────── */
const KIND = {
  main_push: ["병합 반영", "badge--celadon"],
  merge_request: ["병합 요청", "badge--cobalt"],
  branch_push: ["브랜치 push", "badge--muted"],
  release: ["릴리스", "badge--iron"],
};
function feedRow(ev, first) {
  const row = el("div", "feed-row" + (first ? "" : ""));
  const [label, cls] = KIND[ev.kind] || [ev.kind, "badge--muted"];
  row.appendChild(el("span", "badge " + cls, label));
  const body = el("div", "feed-body");
  const line = el("div", "feed-line");
  const who = el("span", "who", ev.sender_user ?? ev.sender_device);
  line.appendChild(who);
  line.appendChild(document.createTextNode(" · " + ev.repo_name + (ev.message ? " — " + ev.message : "")));
  body.appendChild(line);
  row.appendChild(body);
  row.appendChild(el("div", "feed-time", rel(ev.created_at)));
  return row;
}
function renderEvents(panel) {
  const events = state.data.events || [];
  const plaque = el("div", "plaque");
  if (events.length === 0) {
    plaque.appendChild(emptyBox("아직 팀 활동이 없습니다", "팀원이 push하면 누가 어느 저장소에 무엇을 했는지 여기에 기록됩니다."));
  } else {
    const feed = el("div", "feed");
    events.forEach((ev) => feed.appendChild(feedRow(ev)));
    plaque.appendChild(feed);
  }
  panel.appendChild(plaque);
}

function emptyBox(title, hint) {
  const e = el("div", "empty");
  e.appendChild(el("div", "empty-title", title));
  e.appendChild(el("div", "empty-hint", hint));
  return e;
}

/* ── 탭 ────────────────────────────────────────────────────────────────── */
function selectTab(tab) {
  state.tab = tab;
  document.querySelectorAll("#tabs .tab").forEach((b) => {
    const on = b.dataset.tab === tab;
    b.classList.toggle("is-active", on);
    b.setAttribute("aria-selected", on ? "true" : "false");
  });
  render();
}
document.querySelectorAll("#tabs .tab").forEach((b) => {
  b.addEventListener("click", () => selectTab(b.dataset.tab));
});

/* ── 기동: 살아 있는 토큰이면 곧바로 대시보드로 ─────────────────────────── */
(async function boot() {
  const token = sessionStorage.getItem(TOKEN_KEY);
  if (!token) return;
  try {
    const me = await api("/auth/me", { token });
    if (me && me.is_admin) toDash(me);
    else toGate("관리자 계정이 아닙니다. 서버 운영자에게 관리자 지정을 요청하세요.");
  } catch (err) {
    if (err.message !== "unauthorized") toGate(); // 조용히 게이트로
  }
})();

/* 20초마다 조용히 — 탭이 숨어 있거나 대화상자가 열려 있으면 쉰다. */
setInterval(() => {
  if (state.me && !document.hidden && !$("confirm").open) refreshAll();
}, 20000);
