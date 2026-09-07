// 로그인 / 회원가입 모달.
//
// 계정은 팀 서버의 `users` 테이블이 소유한다 (`backend/app/routes/auth.py`).
// 예전에는 이 앱의 설정 파일에 계정이 쌓여서, 로그인 모달이 "이 컴퓨터에서
// 로그인한 적 있는 사람 목록 + 삭제 버튼"까지 보여 줬다. 이제 그 목록은
// 존재하지 않고, 이 모달은 로그인 하나만 한다.
//
// 로그인한 뒤 내 정보를 보는 화면은 `MyPageModal.ts` 다.
import { ipc, ipc_peer, type Account } from "../lib/ipc";
import { openModal } from "./Modal";
import { toast } from "./Toast";
import { setBusy } from "./Busy";
import { refreshSession, setSession } from "../lib/session";

/**
 * Google 로그인 버튼 + "또는" 구분선. 서버가 Google OAuth 를 지원하지 않아도
 * 버튼은 보이고, 누르면 서버가 알려 주는 사유(설정 안 됨 등)를 그대로 보여
 * 준다 — 감춰 버리면 "왜 다른 팀원은 되는데 나는 안 되지?"를 알 수 없다.
 */
function googleSignInRow(done: (me: Account) => void, errBox: HTMLElement): HTMLElement {
  const box = document.createElement("div");
  box.className = "flex flex-col gap-2";

  const divider = document.createElement("div");
  divider.className = "flex items-center gap-3";
  const bar = document.createElement("span");
  bar.className = "h-px flex-1 bg-[var(--color-hairline)]";
  const label = document.createElement("span");
  label.className = "text-display-xs text-[color:var(--color-ink-faint)]";
  label.textContent = "또는";
  divider.append(bar, label, bar.cloneNode());
  box.appendChild(divider);

  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = "gc-google-button w-full";
  // Google 4색 G — 외부 인증(G)은 색을 억누르지 않고 그대로 둔다.
  btn.innerHTML = `
    <svg width="18" height="18" viewBox="0 0 48 48" aria-hidden="true">
      <path fill="#FFC107" d="M43.6 20.1H42V20H24v8h11.3c-1.6 4.7-5.9 8-11.3 8-6.6 0-12-5.4-12-12s5.4-12 12-12c3.1 0 5.9 1.2 8 3l5.7-5.7C34.5 6.1 29.5 4 24 4 13 4 4 13 4 24s9 20 20 20 20-9 20-20c0-1.3-.1-2.6-.4-3.9z"/>
      <path fill="#FF3D00" d="M6.3 14.7l6.6 4.8C14.7 15.1 19 12 24 12c3.1 0 5.9 1.2 8 3l5.7-5.7C34.5 6.1 29.5 4 24 4 16.3 4 9.7 8.3 6.3 14.7z"/>
      <path fill="#4CAF50" d="M24 44c5.2 0 9.9-2 13.4-5.2l-6.2-5.2C29.2 35.1 26.7 36 24 36c-5.2 0-9.6-3.3-11.3-8l-6.5 5C9.5 39.6 16.2 44 24 44z"/>
      <path fill="#1976D2" d="M43.6 20.1H42V20H24v8h11.3c-.8 2.2-2.2 4.2-4.1 5.6l6.2 5.2C36.9 39.2 44 34 44 24c0-1.3-.1-2.6-.4-3.9z"/>
    </svg>
    <span>Google로 로그인</span>
  `;
  btn.addEventListener("click", async () => {
    errBox.hidden = true;
    setBusy(btn, true, "Google 로그인 중…");
    try {
      const me = await ipc.googleLoginStart();
      done(me);
    } catch (e) {
      errBox.textContent = (e as Error).message ?? String(e);
      errBox.hidden = false;
    } finally {
      setBusy(btn, false);
    }
  });
  box.appendChild(btn);
  return box;
}

/** 서버 주소를 입력하는 접히는 줄. 로그인·회원가입 모달이 함께 쓴다. */
function serverRow(): HTMLElement {
  const box = document.createElement("details");
  box.className = "text-display-xs";
  const summary = document.createElement("summary");
  summary.className = "cursor-pointer text-[color:var(--color-ink-muted)]";
  summary.textContent = "서버 주소";
  box.appendChild(summary);

  const inner = document.createElement("div");
  inner.className = "flex flex-col gap-2 pt-2";
  const hint = document.createElement("div");
  hint.className = "text-[color:var(--color-ink-muted)]";
  hint.textContent =
    "계정은 팀이 함께 쓰는 서버에 저장됩니다. 서버는 이 저장소의 backend/ 를 띄우면 됩니다: cd backend && uvicorn app.main:app";
  inner.appendChild(hint);

  const row = document.createElement("div");
  row.className = "flex gap-2";
  const input = document.createElement("input");
  input.className = "gc-input flex-1";
  input.type = "text";
  input.placeholder = "http://127.0.0.1:8000";
  row.appendChild(input);
  const save = document.createElement("button");
  save.type = "button";
  save.className = "gc-button-secondary shrink-0";
  save.textContent = "저장";
  row.appendChild(save);
  inner.appendChild(row);

  const msg = document.createElement("div");
  msg.className = "text-[color:var(--color-ink-muted)]";
  inner.appendChild(msg);
  box.appendChild(inner);

  void ipc_peer
    .getConfig()
    .then((c) => {
      if (c.backend_url) input.value = c.backend_url;
      else box.open = true; // 주소가 없으면 펼쳐서 먼저 채우게 한다.
    })
    .catch(() => {
      box.open = true;
    });

  save.addEventListener("click", async () => {
    const url = input.value.trim();
    if (!url) {
      msg.textContent = "서버 주소를 입력하세요.";
      msg.className = "text-[color:var(--color-danger)]";
      return;
    }
    setBusy(save, true, "확인 중…");
    try {
      await ipc_peer.setBackendUrl(url);
      // 저장만 하고 "됐다"고 말하면 안 된다 — 오타 하나로 로그인이 계속
      // 실패하는데 화면은 성공이라고 하니 원인을 찾을 수 없다.
      const check = await ipc_peer.checkBackend(url).catch(() => ({
        ok: false,
        message: "연결을 확인할 수 없습니다.",
      }));
      msg.textContent = check.ok
        ? "서버에 연결됩니다. 이제 로그인하거나 계정을 만들 수 있습니다."
        : check.message;
      msg.className = check.ok
        ? "text-[color:var(--color-success)] whitespace-pre-line"
        : "text-[color:var(--color-danger)] whitespace-pre-line";
    } catch (e) {
      msg.textContent = `저장 실패: ${(e as Error).message ?? e}`;
      msg.className = "text-[color:var(--color-danger)] whitespace-pre-line";
    } finally {
      setBusy(save, false);
    }
  });

  return box;
}

// ── 로그인 ───────────────────────────────────────────────────────────
export function openAccountModal(): void {
  const m = openModal({
    title: "로그인",
    description: "아이디 또는 이메일로 로그인합니다.",
    hideFooter: true,
  });
  m.el.addEventListener("close", () => {
    void refreshSession();
  });

  const form = document.createElement("form");
  form.className = "flex flex-col gap-3";
  form.innerHTML = `
    <label class="flex flex-col gap-1">
      <span class="text-display-sm text-[color:var(--color-ink-muted)]">아이디 또는 이메일</span>
      <input id="acc-username" class="gc-input" type="text" autocomplete="username" />
    </label>
    <label class="flex flex-col gap-1">
      <span class="text-display-sm text-[color:var(--color-ink-muted)]">비밀번호</span>
      <input id="acc-password" class="gc-input" type="password" autocomplete="current-password" />
    </label>
    <div id="acc-login-error" class="text-display-xs text-[color:var(--color-danger)] whitespace-pre-line" hidden></div>
    <button id="acc-submit" type="submit" class="gc-button-primary">로그인</button>
    <div class="flex items-center gap-1 text-display-sm">
      <span class="text-[color:var(--color-ink-muted)]">계정이 없으신가요?</span>
      <a id="acc-goto-register" class="font-medium text-[color:var(--color-accent)] cursor-pointer hover:underline">회원가입</a>
    </div>
  `;
  m.body.appendChild(form);
  m.body.appendChild(serverRow());

  const submit = form.querySelector<HTMLButtonElement>("#acc-submit")!;
  const errBox = form.querySelector<HTMLDivElement>("#acc-login-error")!;

  // Google 버튼은 구분선 위에 놓기 위해 제출 버튼 바로 위(form 안)에 끼운다.
  form
    .querySelector<HTMLButtonElement>("#acc-submit")!
    .insertAdjacentElement("beforebegin", googleSignInRow(async (me) => {
      m.close();
      await setSession(me);
      toast(`${me.name}님, 환영합니다!`, "success");
    }, errBox));

  form.querySelector<HTMLAnchorElement>("#acc-goto-register")!.addEventListener("click", () => {
    m.close();
    openRegisterModal();
  });

  form.addEventListener("submit", async (ev) => {
    ev.preventDefault();
    const username = form.querySelector<HTMLInputElement>("#acc-username")!.value.trim();
    const password = form.querySelector<HTMLInputElement>("#acc-password")!.value;
    errBox.hidden = true;
    if (!username || !password) {
      errBox.textContent = "아이디와 비밀번호를 입력하세요.";
      errBox.hidden = false;
      return;
    }
    setBusy(submit, true, "로그인 중…");
    try {
      const me = await ipc.accountLoginByPassword(username, password);
      m.close();
      await setSession(me);
      toast(`${me.name}님, 환영합니다!`, "success");
    } catch (e) {
      errBox.textContent = (e as Error).message ?? String(e);
      errBox.hidden = false;
    } finally {
      setBusy(submit, false);
    }
  });

  form.querySelector<HTMLInputElement>("#acc-username")!.focus();
}

// ── 회원가입 ──────────────────────────────────────────────────────────
export function openRegisterModal(): void {
  const m = openModal({
    title: "회원가입",
    description: "팀 서버에 계정을 만듭니다. 등록하면 바로 로그인됩니다.",
    hideFooter: true,
  });
  m.el.addEventListener("close", () => {
    void refreshSession();
  });

  const form = document.createElement("form");
  form.className = "flex flex-col gap-3";
  form.innerHTML = `
    <label class="flex flex-col gap-1">
      <span class="text-display-sm text-[color:var(--color-ink-muted)]">이름</span>
      <input id="reg-name" class="gc-input" type="text" placeholder="예: 홍길동" autocomplete="name" />
    </label>
    <label class="flex flex-col gap-1">
      <span class="text-display-sm text-[color:var(--color-ink-muted)]">이메일</span>
      <input id="reg-email" class="gc-input" type="email" placeholder="hong@example.com" autocomplete="email" />
      <span class="text-display-xs text-[color:var(--color-ink-muted)]">
        팀 구성원·병합 관리자는 이메일로 매칭됩니다. 팀에서 쓰는 이메일을 넣으세요.
      </span>
    </label>
    <label class="flex flex-col gap-1">
      <span class="text-display-sm text-[color:var(--color-ink-muted)]">아이디</span>
      <input id="reg-username" class="gc-input" type="text" placeholder="hong" autocomplete="username" />
    </label>
    <label class="flex flex-col gap-1">
      <span class="text-display-sm text-[color:var(--color-ink-muted)]">비밀번호 (8자 이상)</span>
      <input id="reg-password" class="gc-input" type="password" autocomplete="new-password" />
    </label>
    <label class="flex flex-col gap-1">
      <span class="text-display-sm text-[color:var(--color-ink-muted)]">비밀번호 확인</span>
      <input id="reg-password2" class="gc-input" type="password" autocomplete="new-password" />
    </label>
    <div id="reg-error" class="text-display-xs text-[color:var(--color-danger)] whitespace-pre-line" hidden></div>
    <button id="reg-submit" type="submit" class="gc-button-primary">가입하고 로그인</button>
    <div class="flex items-center gap-1 text-display-sm">
      <span class="text-[color:var(--color-ink-muted)]">이미 계정이 있으신가요?</span>
      <a id="reg-goto-login" class="font-medium text-[color:var(--color-accent)] cursor-pointer hover:underline">로그인</a>
    </div>
  `;
  m.body.appendChild(form);
  m.body.appendChild(serverRow());

  const submit = form.querySelector<HTMLButtonElement>("#reg-submit")!;
  const errBox = form.querySelector<HTMLDivElement>("#reg-error")!;

  form
    .querySelector<HTMLButtonElement>("#reg-submit")!
    .insertAdjacentElement("beforebegin", googleSignInRow(async (me) => {
      m.close();
      await setSession(me);
      toast(`${me.name}님, 환영합니다!`, "success");
    }, errBox));

  form.querySelector<HTMLAnchorElement>("#reg-goto-login")!.addEventListener("click", () => {
    m.close();
    openAccountModal();
  });

  form.addEventListener("submit", async (ev) => {
    ev.preventDefault();
    const name = form.querySelector<HTMLInputElement>("#reg-name")!.value.trim();
    const email = form.querySelector<HTMLInputElement>("#reg-email")!.value.trim();
    const username = form.querySelector<HTMLInputElement>("#reg-username")!.value.trim();
    const password = form.querySelector<HTMLInputElement>("#reg-password")!.value;
    const password2 = form.querySelector<HTMLInputElement>("#reg-password2")!.value;
    errBox.hidden = true;
    const fail = (text: string) => {
      errBox.textContent = text;
      errBox.hidden = false;
    };
    if (!name || !email || !username || !password) {
      fail("모든 항목을 입력하세요.");
      return;
    }
    // 서버까지 가지 않아도 알 수 있는 것은 여기서 걸러 준다.
    if (password !== password2) {
      fail("비밀번호가 서로 다릅니다.");
      return;
    }
    if (password.length < 8) {
      fail("비밀번호는 8자 이상이어야 합니다.");
      return;
    }
    if (!/^[a-z0-9._-]{2,32}$/i.test(username)) {
      fail("아이디는 영문/숫자/._- 만 사용해 2~32자로 입력하세요.");
      return;
    }
    setBusy(submit, true, "가입 중…");
    try {
      const me = await ipc.accountRegister(name, email, username, password);
      m.close();
      await setSession(me);
      toast(`${me.name}님, 환영합니다! 회원가입이 완료되었습니다.`, "success");
    } catch (e) {
      fail((e as Error).message ?? String(e));
    } finally {
      setBusy(submit, false);
    }
  });

  form.querySelector<HTMLInputElement>("#reg-name")!.focus();
}
