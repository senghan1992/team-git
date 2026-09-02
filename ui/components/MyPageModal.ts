// 마이페이지 — 로그인한 상태에서 사이드바의 내 이름을 누르면 열린다.
//
// 예전에는 로그인 폼과 "내 계정" 카드와 "계정 전환/삭제" 목록이 한 모달에
// 섞여 있었다. 로그인한 뒤에도 아이디/비밀번호 입력칸이 그대로 보였고, 이
// 컴퓨터에서 한 번이라도 로그인한 사람들이 삭제 버튼과 함께 나열됐다.
// 어느 앱에서도 마이페이지가 그렇게 생기지 않았으니 낯설 수밖에 없다.
//
// 그래서 익숙한 순서로 다시 짰다:
//   프로필 헤더(아바타·이름·아이디·이메일) → 내 정보 수정 → 비밀번호 변경
//   → 로그아웃 → (맨 아래, 위험 구역) 회원 탈퇴
//
// 계정 전환은 "로그아웃 후 다시 로그인"이다. 계정 목록은 서버가 소유하므로
// 이 컴퓨터가 기억할 이유가 없다.
import { ipc, type Account } from "../lib/ipc";
import { openModal, confirmDialog } from "./Modal";
import { toast } from "./Toast";
import { setBusy } from "./Busy";
import { refreshSession, setSession } from "../lib/session";
import { openAccountModal } from "./AccountModal";

/** yyyy년 M월 d일 — 가입일처럼 한 번 읽고 마는 값에는 이 형태가 읽기 쉽다. */
function formatJoined(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "-";
  return `${d.getFullYear()}년 ${d.getMonth() + 1}월 ${d.getDate()}일`;
}

export function openMyPageModal(): void {
  void (async () => {
    let me = await ipc.accountCurrent().catch(() => null);
    if (!me) {
      // 세션이 없으면 마이페이지가 아니라 로그인 화면이 맞다.
      openAccountModal();
      return;
    }

    const m = openModal({ title: "내 정보", hideFooter: true });
    m.el.addEventListener("close", () => {
      void refreshSession();
    });

    const root = document.createElement("div");
    root.className = "flex flex-col gap-5";
    m.body.appendChild(root);

    // 서버에서 최신 정보를 한 번 더 읽는다 (다른 기기에서 바꿨을 수 있다).
    // 오프라인이면 캐시가 그대로 오므로 화면이 비지 않는다.
    //
    // 여기서 `setSession` 을 부르면 안 된다: 세션 이벤트는 앱 전체를 다시
    // 그리고, 그 과정에서 열려 있는 dialog 를 닫아 버린다(고아 dialog 방지
    // 코드). 사이드바 갱신은 이 모달이 닫힐 때 `refreshSession` 이 한다.
    void ipc
      .accountRefresh()
      .then((fresh) => {
        if (!fresh) return;
        me = fresh;
        render();
      })
      .catch(() => undefined);

    function render() {
      if (!me) return;
      root.innerHTML = "";
      root.appendChild(profileHeader(me));
      root.appendChild(
        profileForm(me, (updated) => {
          me = updated;
          render();
        }),
      );
      root.appendChild(passwordForm());
      root.appendChild(sessionRow(m.close));
      root.appendChild(dangerZone(me, m.close));
    }

    render();
  })();
}

// ─── 프로필 헤더 ─────────────────────────────────────────────────────────────

function profileHeader(me: Account): HTMLElement {
  const box = document.createElement("div");
  box.className = "flex items-center gap-3";

  const avatar = document.createElement("span");
  avatar.className =
    "inline-flex items-center justify-center w-12 h-12 rounded-full bg-[color:var(--color-primary)] text-white text-display-lg font-semibold shrink-0";
  avatar.textContent = (me.name || me.username || "?").trim().charAt(0).toUpperCase();
  box.appendChild(avatar);

  const text = document.createElement("div");
  text.className = "min-w-0 flex flex-col";
  const name = document.createElement("div");
  name.className = "text-display-lg font-medium truncate";
  name.textContent = me.name;
  text.appendChild(name);
  const handle = document.createElement("div");
  handle.className = "text-display-sm text-[color:var(--color-ink-muted)] truncate";
  handle.textContent = `@${me.username} · ${me.email}`;
  text.appendChild(handle);
  const joined = document.createElement("div");
  joined.className = "text-display-xs text-[color:var(--color-ink-muted)]";
  joined.textContent = `${formatJoined(me.created_at)} 가입`;
  text.appendChild(joined);
  box.appendChild(text);

  return box;
}

// ─── 내 정보 수정 ────────────────────────────────────────────────────────────

function profileForm(me: Account, onSaved: (a: Account) => void): HTMLElement {
  const section = document.createElement("form");
  section.className = "gc-card flex flex-col gap-3";
  section.innerHTML = `
    <div class="text-display-md font-medium">프로필</div>
    <label class="flex flex-col gap-1">
      <span class="text-display-sm text-[color:var(--color-ink-muted)]">이름</span>
      <input id="mp-name" class="gc-input" type="text" autocomplete="name" />
    </label>
    <label class="flex flex-col gap-1">
      <span class="text-display-sm text-[color:var(--color-ink-muted)]">이메일</span>
      <input id="mp-email" class="gc-input" type="email" autocomplete="email" />
      <span class="text-display-xs text-[color:var(--color-ink-muted)]">
        팀 구성원·병합 관리자는 이메일로 매칭됩니다. 바꾸면 저장소의
        <code>.gpconfig</code>에 적힌 이메일도 같이 고쳐야 합니다.
      </span>
    </label>
    <label class="flex flex-col gap-1">
      <span class="text-display-sm text-[color:var(--color-ink-muted)]">아이디</span>
      <input class="gc-input" type="text" value="${me.username}" disabled />
      <span class="text-display-xs text-[color:var(--color-ink-muted)]">아이디는 변경할 수 없습니다.</span>
    </label>
    <div id="mp-profile-msg" class="text-display-xs" hidden></div>
    <div class="flex justify-end">
      <button id="mp-profile-save" type="submit" class="gc-button-primary">저장</button>
    </div>
  `;
  const nameInput = section.querySelector<HTMLInputElement>("#mp-name")!;
  const emailInput = section.querySelector<HTMLInputElement>("#mp-email")!;
  nameInput.value = me.name;
  emailInput.value = me.email;
  const msg = section.querySelector<HTMLDivElement>("#mp-profile-msg")!;
  const save = section.querySelector<HTMLButtonElement>("#mp-profile-save")!;

  function show(text: string, kind: "error" | "ok") {
    msg.textContent = text;
    msg.className =
      "text-display-xs " +
      (kind === "error"
        ? "text-[color:var(--color-danger)]"
        : "text-[color:var(--color-success)]");
    msg.hidden = false;
  }

  section.addEventListener("submit", async (ev) => {
    ev.preventDefault();
    const name = nameInput.value.trim();
    const email = emailInput.value.trim();
    msg.hidden = true;
    if (!name || !email) {
      show("이름과 이메일을 입력하세요.", "error");
      return;
    }
    // 바뀐 것이 없으면 서버를 부르지 않는다.
    if (name === me.name && email.toLowerCase() === me.email) {
      show("변경된 내용이 없습니다.", "ok");
      return;
    }
    setBusy(save, true, "저장 중…");
    try {
      const updated = await ipc.accountUpdateProfile(name, email);
      toast("내 정보를 저장했습니다.", "success");
      onSaved(updated);
    } catch (e) {
      show((e as Error).message ?? String(e), "error");
    } finally {
      setBusy(save, false);
    }
  });

  return section;
}

// ─── 비밀번호 변경 ───────────────────────────────────────────────────────────

function passwordForm(): HTMLElement {
  const section = document.createElement("form");
  section.className = "gc-card flex flex-col gap-3";
  section.innerHTML = `
    <div class="text-display-md font-medium">비밀번호 변경</div>
    <label class="flex flex-col gap-1">
      <span class="text-display-sm text-[color:var(--color-ink-muted)]">현재 비밀번호</span>
      <input id="mp-pw-cur" class="gc-input" type="password" autocomplete="current-password" />
    </label>
    <label class="flex flex-col gap-1">
      <span class="text-display-sm text-[color:var(--color-ink-muted)]">새 비밀번호 (8자 이상)</span>
      <input id="mp-pw-new" class="gc-input" type="password" autocomplete="new-password" />
    </label>
    <label class="flex flex-col gap-1">
      <span class="text-display-sm text-[color:var(--color-ink-muted)]">새 비밀번호 확인</span>
      <input id="mp-pw-new2" class="gc-input" type="password" autocomplete="new-password" />
    </label>
    <div id="mp-pw-msg" class="text-display-xs" hidden></div>
    <div class="flex justify-end">
      <button id="mp-pw-save" type="submit" class="gc-button-secondary">비밀번호 변경</button>
    </div>
  `;
  const cur = section.querySelector<HTMLInputElement>("#mp-pw-cur")!;
  const next = section.querySelector<HTMLInputElement>("#mp-pw-new")!;
  const again = section.querySelector<HTMLInputElement>("#mp-pw-new2")!;
  const msg = section.querySelector<HTMLDivElement>("#mp-pw-msg")!;
  const save = section.querySelector<HTMLButtonElement>("#mp-pw-save")!;

  function show(text: string, kind: "error" | "ok") {
    msg.textContent = text;
    msg.className =
      "text-display-xs " +
      (kind === "error"
        ? "text-[color:var(--color-danger)]"
        : "text-[color:var(--color-success)]");
    msg.hidden = false;
  }

  section.addEventListener("submit", async (ev) => {
    ev.preventDefault();
    msg.hidden = true;
    if (!cur.value || !next.value) {
      show("현재 비밀번호와 새 비밀번호를 입력하세요.", "error");
      return;
    }
    // 확인란 불일치는 서버까지 갈 필요가 없다 — 가장 흔한 실수라 즉시 알린다.
    if (next.value !== again.value) {
      show("새 비밀번호가 서로 다릅니다.", "error");
      return;
    }
    if (next.value.length < 8) {
      show("새 비밀번호는 8자 이상이어야 합니다.", "error");
      return;
    }
    setBusy(save, true, "변경 중…");
    try {
      await ipc.accountChangePassword(cur.value, next.value);
      cur.value = next.value = again.value = "";
      show("비밀번호를 변경했습니다.", "ok");
      toast("비밀번호를 변경했습니다.", "success");
    } catch (e) {
      show((e as Error).message ?? String(e), "error");
    } finally {
      setBusy(save, false);
    }
  });

  return section;
}

// ─── 로그아웃 / 계정 전환 ────────────────────────────────────────────────────

function sessionRow(close: () => void): HTMLElement {
  const row = document.createElement("div");
  row.className = "flex flex-wrap items-center gap-2";

  const logout = document.createElement("button");
  logout.className = "gc-button-secondary";
  logout.textContent = "로그아웃";
  logout.addEventListener("click", async () => {
    setBusy(logout, true, "로그아웃 중…");
    try {
      await ipc.accountLogout();
      close();
      await setSession(null);
      toast("로그아웃했습니다.", "info");
    } catch (e) {
      toast(`로그아웃 실패: ${(e as Error).message ?? e}`, "error");
    } finally {
      setBusy(logout, false);
    }
  });
  row.appendChild(logout);

  // "계정 전환"은 결국 로그아웃 후 다시 로그인이다. 목록을 보여 주는 대신
  // 그 두 단계를 한 번에 해 준다.
  const switchBtn = document.createElement("button");
  switchBtn.className = "gc-button-secondary";
  switchBtn.textContent = "다른 계정으로 로그인";
  switchBtn.addEventListener("click", async () => {
    setBusy(switchBtn, true, "전환 중…");
    try {
      await ipc.accountLogout();
      close();
      await setSession(null);
      openAccountModal();
    } catch (e) {
      toast(`전환 실패: ${(e as Error).message ?? e}`, "error");
    } finally {
      setBusy(switchBtn, false);
    }
  });
  row.appendChild(switchBtn);

  return row;
}

// ─── 위험 구역 ───────────────────────────────────────────────────────────────

function dangerZone(me: Account, close: () => void): HTMLElement {
  // 되돌릴 수 없는 동작은 맨 아래에, 시각적으로 분리해서 둔다.
  const box = document.createElement("details");
  box.className = "gc-danger";
  const summary = document.createElement("summary");
  summary.className = "text-display-sm cursor-pointer";
  summary.textContent = "회원 탈퇴";
  box.appendChild(summary);

  const body = document.createElement("div");
  body.className = "flex flex-col gap-2 pt-2";
  const desc = document.createElement("div");
  desc.className = "text-display-xs text-[color:var(--color-ink-muted)] whitespace-pre-line";
  desc.textContent =
    "계정과 로그인 기록이 서버에서 삭제되고 즉시 로그아웃됩니다. 되돌릴 수 없습니다.\n" +
    "등록한 저장소와 커밋은 지워지지 않습니다 — 이 앱의 계정만 삭제됩니다.";
  body.appendChild(desc);

  const btn = document.createElement("button");
  btn.className = "gc-button-secondary self-start text-[color:var(--color-danger)]";
  btn.textContent = "계정 삭제";
  btn.addEventListener("click", async () => {
    const ok = await confirmDialog({
      title: "회원 탈퇴",
      message: `${me.name} (${me.email}) 계정을 삭제합니다.\n되돌릴 수 없습니다. 계속하시겠습니까?`,
      confirmLabel: "탈퇴",
      destructive: true,
    });
    if (!ok) return;
    setBusy(btn, true, "삭제 중…");
    try {
      await ipc.accountDeleteSelf();
      close();
      await setSession(null);
      toast("계정을 삭제했습니다.", "info");
    } catch (e) {
      toast(`탈퇴 실패: ${(e as Error).message ?? e}`, "error");
    } finally {
      setBusy(btn, false);
    }
  });
  body.appendChild(btn);
  box.appendChild(body);

  return box;
}
