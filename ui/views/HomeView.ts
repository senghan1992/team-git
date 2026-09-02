import { ipc, type Repo, type SshDirListing, type SshTarget } from "../lib/ipc";
import { normalizePort, renderSshTestReport, runSshTest } from "../lib/sshTest";
import { renderRepoCard } from "../components/RepositoryCard";
import { openModal } from "../components/Modal";
import { icon } from "../components/Icon";
import { setBusy } from "../components/Busy";
import { toast } from "../components/Toast";
import type { Page } from "../components/Sidebar";

export function renderHomeView(
  repos: Repo[],
  onNav: (p: Page) => void,
  onReposChanged: () => void,
): HTMLElement {
  const main = document.createElement("main");
  main.className = "flex-1 overflow-y-auto p-8 flex flex-col gap-6";

  // ── 헤더 ────────────────────────────────────────────────────────────────
  //
  // 저장소가 하나라도 있으면 사용자가 여기 온 이유는 "내 저장소 상태 보기"다.
  // 그래서 큰 등록 CTA는 진짜 빈 상태에서만 화면을 차지하고, 그 뒤로는
  // 헤더 우측의 작은 버튼으로 물러난다.
  if (repos.length > 0) {
    const head = document.createElement("div");
    head.className = "flex items-end justify-between gap-4";
    const headText = document.createElement("div");
    headText.className = "gc-page-head";
    const title = document.createElement("div");
    title.className = "gc-page-head__title";
    title.textContent = "내 저장소";
    headText.appendChild(title);
    const sub = document.createElement("div");
    sub.className = "gc-page-head__sub";
    sub.textContent = "각 저장소가 지금 무엇을 기다리는지 아래에서 확인하세요.";
    headText.appendChild(sub);
    head.appendChild(headText);
    const addBtn = document.createElement("button");
    addBtn.id = "btn-add-project";
    addBtn.className = "gc-button-secondary shrink-0 inline-flex items-center gap-1";
    addBtn.appendChild(icon("plus", 14));
    const addLabel = document.createElement("span");
    addLabel.textContent = "저장소 추가";
    addBtn.appendChild(addLabel);
    addBtn.addEventListener("click", () => openAddProjectModal(onReposChanged, onNav));
    head.appendChild(addBtn);
    main.appendChild(head);
  } else {
    const ctaCard = document.createElement("div");
    ctaCard.className = "gc-cta";
    const ctaHead = document.createElement("div");
    ctaHead.className = "flex items-center gap-3";
    const ctaIconWrap = document.createElement("span");
    ctaIconWrap.className = "gc-cta__tile";
    ctaIconWrap.appendChild(icon("plus", 18));
    ctaHead.appendChild(ctaIconWrap);
    const ctaTitle = document.createElement("div");
    ctaTitle.className = "gc-cta__title";
    ctaTitle.textContent = "저장소를 등록하고 시작하세요";
    ctaHead.appendChild(ctaTitle);
    ctaCard.appendChild(ctaHead);
    const ctaDesc = document.createElement("p");
    ctaDesc.className = "gc-cta__desc";
    ctaDesc.textContent =
      "팀이 함께 쓰는 git 저장소의 경로를 넣으면, 내 작업 브랜치 만들기 · 커밋 · 푸시 · 병합 · 동기화를 이 앱에서 처리할 수 있습니다.";
    ctaCard.appendChild(ctaDesc);
    const ctaBtn = document.createElement("button");
    ctaBtn.id = "btn-add-project";
    ctaBtn.className = "gc-cta__btn";
    ctaBtn.textContent = "저장소 추가";
    ctaBtn.addEventListener("click", () => openAddProjectModal(onReposChanged, onNav));
    ctaCard.appendChild(ctaBtn);
    main.appendChild(ctaCard);
  }

  // ── Repo grid (shown when repos exist) ──────────────────────────────────
  if (repos.length > 0) {
    const grid = document.createElement("div");
    grid.className = "grid grid-cols-1 md:grid-cols-2 gap-4";
    for (const r of repos) {
      grid.appendChild(renderRepoCard(
        r,
        onReposChanged,
        (tab) => onNav({ kind: "repo", repoId: r.id, tab: tab ?? "work" }),
      ));
    }
    main.appendChild(grid);
  }

  return main;
}

function openAddProjectModal(onReposChanged: () => void, onNav: (p: Page) => void): void {
  const m = openModal({
    title: "저장소 추가",
    submitLabel: "등록",
    onSubmit: async (close) => {
      const ssh_host = (m.body.querySelector<HTMLInputElement>("#ssh-host")!).value.trim();
      const ssh_user = (m.body.querySelector<HTMLInputElement>("#ssh-user")!).value.trim();
      const ssh_key_path = (m.body.querySelector<HTMLInputElement>("#ssh-key")!).value.trim();
      const ssh_password = (m.body.querySelector<HTMLInputElement>("#ssh-password")!).value;
      const project_path = (m.body.querySelector<HTMLInputElement>("#proj-path")!).value.trim();
      const ssh_port = normalizePort(m.body.querySelector<HTMLInputElement>("#ssh-port")!.value);

      if (!project_path) {
        m.setError("저장소 폴더 경로를 입력하세요.");
        return;
      }

      m.setSubmitting(true);
      m.setError(null);

      let repo;
      try {
        repo = await ipc.registerRepository({ ssh_user, ssh_host, ssh_key_path, ssh_password, ssh_port, project_path });
      } catch (e) {
        m.setSubmitting(false);
        const raw = (e as Error).message ?? String(e);
        // 예전에는 어떤 실패든 "SSH 연결 실패"로 붙였다. 로컬 폴더를 잘못
        // 고른 사람에게 SSH 를 탓하면 원인을 엉뚱한 곳에서 찾게 된다.
        m.setError(ssh_host ? `서버에 연결하지 못했습니다: ${raw}` : raw);
        // git 저장소가 아닌 폴더라면 여기서 만들 수 있게 해 준다 — 처음
        // git 을 쓰는 사람이 가장 자주 막히는 지점이고, `.git` 폴더만
        // 생기므로 되돌릴 수 있는 동작이다.
        if (!ssh_host && raw.includes("git 저장소가 아닙니다")) {
          offerGitInit(m, project_path, onReposChanged, onNav);
        }
        return;
      }

      // Switch to branch-selection step inside the same modal
      const branchSelect = document.createElement("select");
      branchSelect.id = "branch-select";
      branchSelect.className = "gc-input";

      const labelDiv = document.createElement("div");
      labelDiv.className = "text-display-sm font-medium";
      labelDiv.textContent = "브랜치";

      const laterBtn = document.createElement("button");
      laterBtn.className = "gc-button-secondary";
      laterBtn.textContent = "나중에";
      laterBtn.addEventListener("click", () => {
        onReposChanged();
        close();
      });

      const confirmBtn = document.createElement("button");
      confirmBtn.className = "gc-button-primary";
      confirmBtn.textContent = "확인";

      const btnRow = document.createElement("div");
      btnRow.className = "flex gap-2";
      btnRow.appendChild(laterBtn);
      btnRow.appendChild(confirmBtn);

      // Clear body and rebuild for branch step
      m.body.innerHTML = "";
      m.body.appendChild(labelDiv);
      m.body.appendChild(branchSelect);
      m.body.appendChild(btnRow);

      // Hide submit/cancel buttons in the default footer (we handle confirmation manually)
      const footerBtns = m.el.querySelector(".gc-modal__footer .flex.gap-2") as HTMLElement;
      if (footerBtns) footerBtns.style.display = "none";

      // Load branches and show
      let branches: { name: string; is_remote: boolean }[] = [];
      try {
        branches = await ipc.listBranches(repo.id);
      } catch (e) {
        m.setError(`브랜치 목록 조회 실패: ${(e as Error).message ?? e}`);
        confirmBtn.disabled = true;
      }

      branchSelect.innerHTML = "";
      for (const b of branches) {
        const opt = document.createElement("option");
        opt.value = b.name;
        opt.textContent = b.name + (b.is_remote ? " (remote)" : "");
        branchSelect.appendChild(opt);
      }

      confirmBtn.addEventListener("click", async () => {
        confirmBtn.disabled = true;
        try {
          await ipc.updateRepository(repo.id, { working_branch: branchSelect.value });
          onReposChanged();
          close();
          onNav({ kind: "repo", repoId: repo.id });
        } catch (e) {
          m.setError(`브랜치 선택 실패: ${(e as Error).message ?? e}`);
          confirmBtn.disabled = false;
        }
      });
    },
  });

  // Build the initial body for step 1
  // 필요한 것은 경로 한 칸뿐이다. 예전에는 SSH 호스트·포트·사용자·키·비밀번호
  // 다섯 칸이 먼저 나오고 정작 필수인 경로가 맨 아래에 있었다. 로컬 저장소를
  // 등록하려던 사람은 쓸 일 없는 SSH 용어 다섯 개를 먼저 읽어야 했다.
  // 그래서 경로를 맨 위로 올리고, SSH 는 접어 둔다.
  m.body.innerHTML = `
    <div class="flex flex-col gap-1">
      <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="proj-path">저장소 폴더 경로 <span class="text-[color:var(--color-danger)]">*</span></label>
      <div class="flex gap-2">
        <input id="proj-path" class="gc-input flex-1 min-w-0 font-mono" type="text" placeholder="/home/me/projects/my-app" spellcheck="false" autocapitalize="off" />
        <button id="btn-browse" class="gc-button-secondary shrink-0" type="button">SSH로 찾아보기</button>
      </div>
      <span class="text-display-xs text-[color:var(--color-ink-muted)]">
        이미 <code>git clone</code> 해 둔 폴더를 고르세요. 폴더 안에 <code>.git</code>이 있으면 됩니다.
        <code>~</code>로 시작하는 경로도 됩니다.
      </span>
    </div>

    <details id="ssh-details" class="text-display-sm">
      <summary class="cursor-pointer text-[color:var(--color-ink-muted)]">저장소가 다른 서버에 있나요? (SSH)</summary>
      <div class="flex flex-col gap-3 pt-3">
        <span class="text-display-xs text-[color:var(--color-ink-muted)]">
          내 컴퓨터의 폴더라면 이 부분은 비워 두세요. 원격 서버에 있는 저장소를
          쓸 때만 채웁니다.
        </span>
        <div class="grid grid-cols-2 gap-3">
          <div class="flex flex-col gap-1">
            <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="ssh-host">호스트</label>
            <input id="ssh-host" class="gc-input" type="text" placeholder="dev.example.com" spellcheck="false" />
          </div>
          <div class="flex flex-col gap-1">
            <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="ssh-port">포트</label>
            <input id="ssh-port" class="gc-input" type="number" min="1" max="65535" placeholder="22" value="22" />
          </div>
        </div>
        <div class="flex flex-col gap-1">
          <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="ssh-user">사용자</label>
          <input id="ssh-user" class="gc-input" type="text" placeholder="ubuntu" spellcheck="false" />
        </div>
        <div class="flex flex-col gap-1">
          <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="ssh-key">SSH 키 경로</label>
          <input id="ssh-key" class="gc-input font-mono" type="text" placeholder="~/.ssh/id_ed25519" spellcheck="false" />
        </div>
        <div class="flex flex-col gap-1">
          <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="ssh-password">비밀번호 (키를 안 쓸 때)</label>
          <input id="ssh-password" class="gc-input" type="password" autocomplete="off" />
        </div>
        <button id="btn-test-connection" class="gc-button-secondary self-start" type="button">연결 테스트</button>
        <div id="ssh-test-result"></div>
      </div>
    </details>
  `;
  const testBtn = m.body.querySelector<HTMLButtonElement>("#btn-test-connection")!;
  testBtn.addEventListener("click", async () => {
    const host = m.body.querySelector<HTMLInputElement>("#ssh-host")!.value.trim();
    if (!host) {
      m.setError("호스트를 입력하세요.");
      return;
    }
    m.setError(null);
    testBtn.disabled = true;
    try {
      const report = await runSshTest({
        host,
        user: m.body.querySelector<HTMLInputElement>("#ssh-user")!.value.trim(),
        port: normalizePort(m.body.querySelector<HTMLInputElement>("#ssh-port")!.value),
        key_path: m.body.querySelector<HTMLInputElement>("#ssh-key")!.value.trim(),
        password: m.body.querySelector<HTMLInputElement>("#ssh-password")!.value,
      });
      renderSshTestReport(m.body.querySelector<HTMLElement>("#ssh-test-result")!, report);
    } catch (e) {
      m.setError(String(e));
    } finally {
      testBtn.disabled = false;
    }
  });

  // ── SSH directory browser (opens its own modal) ────────────────────────
  const sshTarget = (): SshTarget => ({
    ssh_user: m.body.querySelector<HTMLInputElement>("#ssh-user")!.value.trim(),
    ssh_host: m.body.querySelector<HTMLInputElement>("#ssh-host")!.value.trim(),
    ssh_key_path: m.body.querySelector<HTMLInputElement>("#ssh-key")!.value.trim(),
    ssh_password: m.body.querySelector<HTMLInputElement>("#ssh-password")!.value,
    ssh_port: normalizePort(m.body.querySelector<HTMLInputElement>("#ssh-port")!.value),
  });

  const browseBtn = m.body.querySelector<HTMLButtonElement>("#btn-browse")!;
  browseBtn.addEventListener("click", () => {
    const t = sshTarget();
    if (!t.ssh_host) {
      m.setError("SSH 브라우저를 쓰려면 SSH 호스트를 먼저 입력하세요.");
      return;
    }
    const projPath = m.body.querySelector<HTMLInputElement>("#proj-path")!;
    openSshBrowserModal(t, (path) => {
      projPath.value = path;
      m.setError(null);
    });
  });
}

/** 새 모달로 원격 파일 목록을 보여주며 폴더를 탐색하는 SSH 브라우저. */
function openSshBrowserModal(target: SshTarget, onPick: (path: string) => void): void {
  const portSuffix = target.ssh_port && target.ssh_port !== 22 ? `:${target.ssh_port}` : "";
  const hostDesc = target.ssh_host
    ? `${target.ssh_user ? `${target.ssh_user}@` : ""}${target.ssh_host}${portSuffix}`
    : "";
  const dialog = document.createElement("dialog");
  dialog.className = "gc-modal";
  dialog.innerHTML = `
    <div class="gc-modal__panel">
      <div class="gc-modal__header">
        <div class="gc-modal__title">SSH로 찾아보기</div>
        <div class="gc-modal__description">${escapeHtml(hostDesc)}</div>
      </div>
      <div class="gc-modal__body">
        <div class="flex items-center gap-2 min-w-0">
          <button id="gcb-up" class="gc-button-secondary shrink-0 px-2" type="button" title="상위 폴더">⬆</button>
          <div id="gcb-path" class="flex-1 min-w-0 flex flex-wrap items-center gap-x-0.5 font-mono text-display-sm"></div>
          <span id="gcb-git" class="gc-badge shrink-0 hidden">git 저장소</span>
        </div>
        <div class="flex items-center gap-2">
          <input id="gcb-path-input" class="gc-input flex-1 min-w-0 font-mono text-display-sm" type="text"
            placeholder="경로 직접 입력 후 Enter (예: /home/user/project, ~/project)"
            spellcheck="false" autocomplete="off" autocapitalize="off" />
          <button id="gcb-go" class="gc-button-secondary shrink-0" type="button">이동</button>
        </div>
        <div id="gcb-list" class="max-h-80 overflow-y-auto flex flex-col gap-0.5"></div>
      </div>
      <div class="gc-modal__footer">
        <div class="gc-modal__error" role="alert"></div>
        <div class="flex gap-2">
          <button class="gc-button-secondary" id="gcb-cancel" type="button">닫기</button>
          <button class="gc-button-primary" id="gcb-select" type="button" disabled>이 경로 사용</button>
        </div>
      </div>
    </div>
  `;
  document.body.appendChild(dialog);
  dialog.showModal();

  const listEl = dialog.querySelector<HTMLElement>("#gcb-list")!;
  const pathEl = dialog.querySelector<HTMLElement>("#gcb-path")!;
  const gitEl = dialog.querySelector<HTMLElement>("#gcb-git")!;
  const errorEl = dialog.querySelector<HTMLElement>(".gc-modal__error")!;
  const upBtn = dialog.querySelector<HTMLButtonElement>("#gcb-up")!;
  const cancelBtn = dialog.querySelector<HTMLButtonElement>("#gcb-cancel")!;
  const selectBtn = dialog.querySelector<HTMLButtonElement>("#gcb-select")!;
  const inputEl = dialog.querySelector<HTMLInputElement>("#gcb-path-input")!;
  const goBtn = dialog.querySelector<HTMLButtonElement>("#gcb-go")!;

  let currentPath = "";
  let homePath = "";

  const setError = (msg: string): void => {
    errorEl.textContent = msg;
  };

  const parentOf = (p: string): string => {
    if (!p || p === "/") return "/";
    const trimmed = p.replace(/\/+$/, "");
    const idx = trimmed.lastIndexOf("/");
    return idx <= 0 ? "/" : trimmed.slice(0, idx);
  };

  // `cd //tmp` 같은 원격 셸 echo 경로의 중복 슬래시를 정리한다.
  const normPath = (p: string): string => p.replace(/\/{2,}/g, "/");
  const joinPath = (base: string, name: string): string => normPath(`${base}/${name}`);

  const renderBreadcrumb = (path: string): void => {
    pathEl.innerHTML = "";
    if (!path || path === "/") {
      const span = document.createElement("span");
      span.textContent = path || "/";
      pathEl.appendChild(span);
      return;
    }
    const parts = path.split("/").filter(Boolean);
    let acc = "";
    for (let i = 0; i < parts.length; i++) {
      acc += "/" + parts[i];
      const isLast = i === parts.length - 1;
      const el = document.createElement("button");
      el.type = "button";
      el.className = isLast
        ? "text-display-sm"
        : "text-display-sm text-[color:var(--color-primary)] hover:underline";
      el.textContent = (i === 0 ? "/" : "") + parts[i];
      el.title = isLast ? acc : `${acc}로 이동`;
      el.disabled = isLast;
      if (!isLast) el.addEventListener("click", () => loadDir(acc));
      pathEl.appendChild(el);
      if (!isLast) {
        const sep = document.createElement("span");
        sep.className = "text-display-sm text-[color:var(--color-ink-muted)]";
        sep.textContent = "/";
        pathEl.appendChild(sep);
      }
    }
  };

  const renderListing = (listing: SshDirListing): void => {
    listEl.innerHTML = "";
    const base = normPath(listing.path);
    const dirs = listing.entries.filter((e) => e.is_dir);
    const files = listing.entries.filter((e) => !e.is_dir);
    for (const e of dirs.concat(files)) {
      const row = document.createElement("button");
      row.type = "button";
      row.className =
        "flex items-center gap-2 px-2 py-1 rounded text-left hover:bg-[color:var(--color-surface-2)]";
      row.textContent = (e.is_dir ? "📁 " : e.is_symlink ? "🔗 " : "📄 ") + e.name;
      row.title = e.is_dir || e.is_symlink ? `열기: ${joinPath(base, e.name)}` : e.name;
      if (e.is_dir || e.is_symlink) {
        row.addEventListener("click", () => {
          loadDir(joinPath(base, e.name));
        });
      } else {
        row.disabled = true;
        row.classList.add("opacity-60");
      }
      listEl.appendChild(row);
    }
    if (listing.entries.length === 0) {
      const empty = document.createElement("div");
      empty.className = "text-display-sm text-[color:var(--color-ink-muted)] px-2 py-1";
      empty.textContent = "빈 폴더";
      listEl.appendChild(empty);
    }
  };

  const loadDir = async (path: string): Promise<void> => {
    setError("");
    selectBtn.disabled = true;
    listEl.innerHTML = `<div class="text-display-sm text-[color:var(--color-ink-muted)] py-2">불러오는 중…</div>`;
    try {
      const listing = await ipc.browseSshDir(target, path);
      const resolved = normPath(listing.path);
      currentPath = resolved;
      if (path === "") homePath = resolved;
      inputEl.value = resolved;
      selectBtn.disabled = false;
      renderBreadcrumb(resolved);
      gitEl.classList.toggle("hidden", !listing.git_repo);
      renderListing({ ...listing, path: resolved });
    } catch (e) {
      const msg = (e as Error).message ?? String(e);
      currentPath = "";
      listEl.innerHTML = `<div class="text-display-sm text-[color:var(--color-danger)] py-2">목록을 불러오지 못했습니다.</div>`;
      setError(
        `SSH 탐색 실패: ${msg}${
          msg.includes("Permission denied")
            ? " — 서버가 이 로그인 방법(비밀번호/키)을 거부했습니다. 비밀번호·키를 다시 확인하고, root 대신 허용된 계정을 쓰는 것도 시도해 보세요."
            : ""
        }`,
      );
    }
  };

  // 입력한 경로로 바로 이동한다. 빈 입력 = 홈, `~`/`~/…`은 홈 기준으로 확장.
  const goToInputPath = (): void => {
    const raw = inputEl.value.trim();
    if (!raw) {
      loadDir("");
      return;
    }
    let p = normPath(raw);
    if (p === "~") {
      p = homePath || "";
    } else if (p.startsWith("~/")) {
      p = homePath ? joinPath(homePath, p.slice(2)) : p.slice(2);
    }
    loadDir(p);
  };
  inputEl.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      goToInputPath();
    }
  });
  goBtn.addEventListener("click", goToInputPath);

  const close = (): void => {
    dialog.close();
    dialog.remove();
  };

  upBtn.addEventListener("click", () => loadDir(parentOf(currentPath || "/")));
  cancelBtn.addEventListener("click", close);
  selectBtn.addEventListener("click", () => {
    const p = currentPath;
    close();
    if (p) onPick(p);
  });
  dialog.addEventListener("click", (e) => {
    if (e.target === dialog) close();
  });
  dialog.addEventListener("cancel", () => {
    dialog.remove();
  });

  loadDir("");
}

function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}


/**
 * "이 폴더는 git 저장소가 아닙니다" 뒤에 붙는 다음 걸음.
 *
 * 터미널로 나가서 `git init` 을 치라고 하면 처음 git 을 쓰는 사람은 거기서
 * 멈춘다. 무엇이 생기는지 먼저 말해 주고, 누르면 만들어 등록까지 한다.
 */
function offerGitInit(
  m: ReturnType<typeof openModal>,
  projectPath: string,
  onReposChanged: () => void,
  onNav: (p: Page) => void,
): void {
  // 이미 붙여 놓은 안내가 있으면 갈아 끼운다 (경로를 고쳐 다시 시도한 경우).
  m.body.querySelector("#gc-init-offer")?.remove();

  const box = document.createElement("div");
  box.id = "gc-init-offer";
  box.className = "gc-banner gc-banner--info flex-col items-start gap-2";
  const text = document.createElement("div");
  text.className = "gc-banner__body text-display-sm whitespace-pre-line";
  text.textContent =
    "이 폴더를 지금 git 저장소로 만들 수 있습니다.\n" +
    "폴더 안에 .git 폴더가 생기고, 파일 내용은 바뀌지 않습니다.";
  box.appendChild(text);
  const btn = document.createElement("button");
  btn.className = "gc-button-primary";
  btn.textContent = "이 폴더를 git 저장소로 만들기";
  btn.addEventListener("click", async () => {
    setBusy(btn, true, "만드는 중…");
    m.setError(null);
    try {
      const repo = await ipc.initRepository(projectPath);
      toast(`${repo.display_name} 저장소를 만들고 등록했습니다.`, "success");
      onReposChanged();
      m.close();
      onNav({ kind: "repo", repoId: repo.id, tab: "work" });
    } catch (e) {
      m.setError(`만들지 못했습니다: ${(e as Error).message ?? e}`);
    } finally {
      setBusy(btn, false);
    }
  });
  box.appendChild(btn);
  m.body.appendChild(box);
}
