// 로그인 / 회원가입 모달 — UX 분리.
// - openAccountModal()  : 로그인만. 하단에 "회원가입" 이동 링크 + 계정 전환/삭제(계정 관리)
// - openRegisterModal() : 회원가입만. 상단에 "로그인" 이동 링크
import { ipc } from "../lib/ipc";
import { openModal, confirmDialog } from "./Modal";
import { toast } from "./Toast";
import { refreshSession } from "../lib/session";

// ── 로그인 모달 ──────────────────────────────────────────────────────
export function openAccountModal(): void {
  void (async () => {
    let accounts = await ipc.accountList().catch(() => []);
    let current = (await ipc.accountCurrent().catch(() => null)) ?? null;

    const m = openModal({
      title: "로그인",
      hideFooter: true,
    });
    // 어떤 경로로 닫히든 세션을 새로고침해 구독자(사이드바/뷰)를 갱신한다.
    m.el.addEventListener("close", () => { void refreshSession(); });

    const root = document.createElement("div");
    root.className = "flex flex-col gap-4";
    m.body.appendChild(root);

    function render() {
      root.innerHTML = "";

      // ── 로그인 폼 ────────────────────────────────────────────────
      const loginForm = document.createElement("form");
      loginForm.className = "flex flex-col gap-3";
      loginForm.innerHTML = `
        <label class="flex flex-col gap-1">
          <span class="text-display-sm text-[color:var(--color-ink-muted)]">아이디</span>
          <input id="acc-username" class="gc-input" type="text" placeholder="test" autocomplete="username" />
        </label>
        <label class="flex flex-col gap-1">
          <span class="text-display-sm text-[color:var(--color-ink-muted)]">비밀번호</span>
          <input id="acc-password" class="gc-input" type="password" placeholder="••••" autocomplete="current-password" />
        </label>
        <div id="acc-login-error" class="text-display-xs text-[color:var(--color-danger)]" hidden></div>
        <div class="flex items-center gap-3">
          <button type="submit" class="gc-button-primary">로그인</button>
          <span class="text-display-xs text-[color:var(--color-ink-muted)]">
            테스트 계정: <code>test</code>/<code>test</code>, <code>test2</code>/<code>test2</code>
          </span>
        </div>
      `;
      loginForm.addEventListener("submit", async (ev) => {
        ev.preventDefault();
        const username = loginForm.querySelector<HTMLInputElement>("#acc-username")!.value.trim();
        const password = loginForm.querySelector<HTMLInputElement>("#acc-password")!.value;
        const errBox = loginForm.querySelector<HTMLDivElement>("#acc-login-error")!;
        errBox.hidden = true;
        if (!username || !password) {
          errBox.textContent = "아이디와 비밀번호를 입력하세요.";
          errBox.hidden = false;
          return;
        }
        try {
          current = await ipc.accountLoginByPassword(username, password);
          toast(`${current.name}님, 환영합니다!`, "success");
          m.close();
        } catch (e) {
          errBox.textContent = (e as Error).message ?? String(e);
          errBox.hidden = false;
        }
      });
      root.appendChild(loginForm);

      // ── 회원가입 이동 링크 ────────────────────────────────────────
      const linkRow = document.createElement("div");
      linkRow.className = "flex items-center gap-1 text-display-sm";
      linkRow.innerHTML = `
        <span class="text-[color:var(--color-ink-muted)]">계정이 없으신가요?</span>
        <a id="acc-goto-register" class="font-medium text-[color:var(--color-accent)] cursor-pointer hover:underline">회원가입</a>
      `;
      linkRow.querySelector<HTMLAnchorElement>("#acc-goto-register")!.addEventListener("click", () => {
        m.close();
        openRegisterModal();
      });
      root.appendChild(linkRow);

      // ── 현재 로그인 상태 ─────────────────────────────────────────
      if (current) {
        const card = document.createElement("div");
        card.className = "gc-card flex flex-col gap-1";
        const head = document.createElement("div");
        head.className = "text-display-md font-medium";
        head.textContent = "내 계정";
        card.appendChild(head);
        const name = document.createElement("div");
        name.className = "text-display-sm font-medium";
        name.textContent = `${current.name}${current.username ? ` (@${current.username})` : ""}`;
        const email = document.createElement("div");
        email.className = "text-display-xs text-[color:var(--color-ink-muted)]";
        email.textContent = current.email;
        card.appendChild(name);
        card.appendChild(email);
        const logout = document.createElement("button");
        logout.className = "gc-button-secondary self-start mt-2 text-display-sm";
        logout.textContent = "로그아웃";
        logout.addEventListener("click", async () => {
          await ipc.accountLogout();
          current = null;
          toast("로그아웃했습니다.", "info");
          m.close();
        });
        card.appendChild(logout);
        root.appendChild(card);
      }

      // ── 계정 목록 (전환 / 삭제) ──────────────────────────────────
      if (accounts.length > 0) {
        const head = document.createElement("div");
        head.className = "text-display-md font-medium";
        head.textContent = current ? "계정 전환" : "계정 선택";
        root.appendChild(head);
        const list = document.createElement("div");
        list.className = "flex flex-col gap-2";
        root.appendChild(list);
        for (const a of accounts) {
          const row = document.createElement("div");
          row.className = "flex items-center gap-2 border border-[color:var(--color-hairline)] rounded-md px-3 py-2";
          const label = document.createElement("div");
          label.className = "flex-1 min-w-0";
          const name = document.createElement("div");
          name.className = "text-display-sm font-medium truncate";
          name.textContent = `${a.name}${a.username ? ` (@${a.username})` : ""}`;
          const email = document.createElement("div");
          email.className = "text-display-xs text-[color:var(--color-ink-muted)] truncate";
          email.textContent = a.email;
          label.appendChild(name);
          label.appendChild(email);
          row.appendChild(label);
          if (current?.id === a.id) {
            const mark = document.createElement("span");
            mark.className = "text-display-xs text-[color:var(--color-ink-muted)]";
            mark.textContent = "로그인됨";
            row.appendChild(mark);
          } else {
            const login = document.createElement("button");
            login.className = "gc-button-secondary text-display-sm";
            login.textContent = "로그인";
            login.addEventListener("click", async () => {
              current = await ipc.accountLogin(a.id);
              toast(`${current.name}님, 환영합니다!`, "success");
              m.close();
            });
            row.appendChild(login);
          }
          const del = document.createElement("button");
          del.className = "gc-button-secondary text-display-sm text-[color:var(--color-danger)]";
          del.textContent = "삭제";
          del.addEventListener("click", async () => {
            const ok = await confirmDialog({
              title: "계정 삭제",
              message: `${a.name} (${a.email}) 계정을 삭제하시겠습니까?`,
              confirmLabel: "삭제",
              destructive: true,
            });
            if (!ok) return;
            await ipc.accountDelete(a.id);
            accounts = accounts.filter((x) => x.id !== a.id);
            if (current?.id === a.id) current = null;
            render();
          });
          row.appendChild(del);
          list.appendChild(row);
        }
      }
    }

    render();
  })();
}

// ── 회원가입 모달 (로그인과 분리) ─────────────────────────────────────
export function openRegisterModal(): void {
  void (async () => {
    const m = openModal({
      title: "회원가입",
      hideFooter: true,
    });
    m.el.addEventListener("close", () => { void refreshSession(); });

    const root = document.createElement("div");
    root.className = "flex flex-col gap-4";
    m.body.appendChild(root);

    // 로그인으로 돌아가기
    const backRow = document.createElement("div");
    backRow.className = "flex items-center gap-1 text-display-sm";
    backRow.innerHTML = `
      <span class="text-[color:var(--color-ink-muted)]">이미 계정이 있으신가요?</span>
      <a id="reg-goto-login" class="font-medium text-[color:var(--color-accent)] cursor-pointer hover:underline">로그인</a>
    `;
    backRow.querySelector<HTMLAnchorElement>("#reg-goto-login")!.addEventListener("click", () => {
      m.close();
      openAccountModal();
    });
    root.appendChild(backRow);

    const form = document.createElement("form");
    form.className = "flex flex-col gap-3";
    form.innerHTML = `
      <label class="flex flex-col gap-1">
        <span class="text-display-sm text-[color:var(--color-ink-muted)]">이름</span>
        <input id="reg-name" class="gc-input" type="text" placeholder="예: 홍길동" />
      </label>
      <label class="flex flex-col gap-1">
        <span class="text-display-sm text-[color:var(--color-ink-muted)]">이메일 (팀 내 식별자)</span>
        <input id="reg-email" class="gc-input" type="email" placeholder="hong@example.com" />
      </label>
      <div class="grid grid-cols-2 gap-3">
        <label class="flex flex-col gap-1">
          <span class="text-display-sm text-[color:var(--color-ink-muted)]">아이디</span>
          <input id="reg-username" class="gc-input" type="text" placeholder="hong2" autocomplete="username" />
        </label>
        <label class="flex flex-col gap-1">
          <span class="text-display-sm text-[color:var(--color-ink-muted)]">비밀번호 (4자 이상)</span>
          <input id="reg-password" class="gc-input" type="password" placeholder="••••" autocomplete="new-password" />
        </label>
      </div>
      <div id="reg-error" class="text-display-xs text-[color:var(--color-danger)]" hidden></div>
      <div class="text-display-xs text-[color:var(--color-ink-muted)]">
        등록하면 바로 로그인됩니다. 프로젝트 구성원·병합 관리자는 이메일로 매칭됩니다. 아이디/비밀번호는 선택 입력입니다.
      </div>
      <button type="submit" class="gc-button-primary self-start">등록하고 로그인</button>
    `;
    form.addEventListener("submit", async (ev) => {
      ev.preventDefault();
      const name = form.querySelector<HTMLInputElement>("#reg-name")!.value.trim();
      const email = form.querySelector<HTMLInputElement>("#reg-email")!.value.trim();
      const username = form.querySelector<HTMLInputElement>("#reg-username")!.value.trim() || undefined;
      const password = form.querySelector<HTMLInputElement>("#reg-password")!.value || undefined;
      const errBox = form.querySelector<HTMLDivElement>("#reg-error")!;
      errBox.hidden = true;
      if (!name || !email) {
        errBox.textContent = "이름과 이메일을 입력하세요.";
        errBox.hidden = false;
        return;
      }
      try {
        const me = await ipc.accountRegister(name, email, username, password);
        toast(`${me.name}님, 환영합니다! 회원가입이 완료되었습니다.`, "success");
        m.close();
      } catch (e) {
        errBox.textContent = (e as Error).message ?? String(e);
        errBox.hidden = false;
      }
    });
    root.appendChild(form);
  })();
}