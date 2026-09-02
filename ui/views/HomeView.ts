import { ipc, type Repo, type SshDirListing, type SshTarget } from "../lib/ipc";
import { normalizePort, renderSshTestReport, runSshTest } from "../lib/sshTest";
import { renderRepoCard } from "../components/RepositoryCard";
import { openModal } from "../components/Modal";
import { icon } from "../components/Icon";
import type { Page } from "../components/Sidebar";

export function renderHomeView(
  repos: Repo[],
  onNav: (p: Page) => void,
  onReposChanged: () => void,
): HTMLElement {
  const main = document.createElement("main");
  main.className = "flex-1 overflow-y-auto p-8 flex flex-col gap-6";

  // ── CTA: 프로젝트 추가 — 코발트 플라크(유약을 바른 한 장) ─────────────
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
  ctaTitle.textContent = "프로젝트 추가";
  ctaHead.appendChild(ctaTitle);
  ctaCard.appendChild(ctaHead);
  const ctaDesc = document.createElement("p");
  ctaDesc.className = "gc-cta__desc";
  ctaDesc.textContent = "저장소를 등록하면 브랜치 관리, 커밋, 푸시, 풀을 바로 시작할 수 있습니다.";
  ctaCard.appendChild(ctaDesc);
  const ctaBtn = document.createElement("button");
  ctaBtn.id = "btn-add-project";
  ctaBtn.className = "gc-cta__btn";
  ctaBtn.textContent = "저장소 추가";
  ctaCard.appendChild(ctaBtn);
  main.appendChild(ctaCard);

  ctaBtn.addEventListener("click", () => {
    openAddProjectModal(onReposChanged, onNav);
  });
  // ── Repo grid (shown when repos exist) ──────────────────────────────────
  if (repos.length > 0) {
    const head = document.createElement("div");
    head.className = "gc-page-head";
    const title = document.createElement("div");
    title.className = "gc-page-head__title";
    title.textContent = "저장소 목록";
    head.appendChild(title);
    const sub = document.createElement("div");
    sub.className = "gc-page-head__sub";
    sub.textContent = `${repos.length}개의 저장소가 등록되어 있습니다.`;
    head.appendChild(sub);
    main.appendChild(head);
    const grid = document.createElement("div");
    grid.className = "grid grid-cols-1 md:grid-cols-2 gap-4";
    for (const r of repos) {
      grid.appendChild(renderRepoCard(
        r,
        onReposChanged,
        () => onNav({ kind: "repo", repoId: r.id }),
      ));
    }
    main.appendChild(grid);
  }

  return main;
}

function openAddProjectModal(onReposChanged: () => void, onNav: (p: Page) => void): void {
  const m = openModal({
    title: "프로젝트 추가",
    submitLabel: "등록",
    onSubmit: async (close) => {
      const ssh_host = (m.body.querySelector<HTMLInputElement>("#ssh-host")!).value.trim();
      const ssh_user = (m.body.querySelector<HTMLInputElement>("#ssh-user")!).value.trim();
      const ssh_key_path = (m.body.querySelector<HTMLInputElement>("#ssh-key")!).value.trim();
      const ssh_password = (m.body.querySelector<HTMLInputElement>("#ssh-password")!).value;
      const project_path = (m.body.querySelector<HTMLInputElement>("#proj-path")!).value.trim();
      const ssh_port = normalizePort(m.body.querySelector<HTMLInputElement>("#ssh-port")!.value);

      if (!project_path) {
        m.setError("프로젝트 경로를 입력하세요.");
        return;
      }

      m.setSubmitting(true);
      m.setError(null);

      let repo;
      try {
        repo = await ipc.registerRepository({ ssh_user, ssh_host, ssh_key_path, ssh_password, ssh_port, project_path });
      } catch (e) {
        m.setSubmitting(false);
        m.setError(`SSH 연결 실패: ${(e as Error).message ?? e}`);
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
  m.body.innerHTML = `
    <div class="flex flex-col gap-1">
      <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="ssh-host">SSH 호스트 (선택)</label>
      <input id="ssh-host" class="gc-input" type="text" placeholder="예: dev.example.com" />
    </div>
    <div class="flex flex-col gap-1">
      <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="ssh-port">포트</label>
      <input id="ssh-port" class="gc-input" type="number" min="1" max="65535" placeholder="예: 22" value="22" />
    </div>
    <div class="flex flex-col gap-1">
      <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="ssh-user">SSH 사용자 (선택)</label>
      <input id="ssh-user" class="gc-input" type="text" placeholder="예: ubuntu" />
    </div>
    <div class="flex flex-col gap-1">
      <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="ssh-key">SSH 키 경로 (선택)</label>
      <input id="ssh-key" class="gc-input" type="text" placeholder="예: ~/.ssh/id_ed25519" />
    </div>
    <div class="flex flex-col gap-1">
      <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="ssh-password">SSH 비밀번호 (선택, 사용자/비밀번호 인증)</label>
      <input id="ssh-password" class="gc-input" type="password" autocomplete="off" placeholder="키 대신 비밀번호 로그인을 쓸 경우 입력" />
    </div>
    <div class="flex flex-col gap-1">
      <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="proj-path">프로젝트 경로 <span class="text-[color:var(--color-danger)]">*</span></label>
      <div class="flex gap-2">
        <input id="proj-path" class="gc-input flex-1 min-w-0" type="text" placeholder="예: /home/me/projects/foo" />
        <button id="btn-browse" class="gc-button-secondary shrink-0" type="button">SSH로 찾아보기</button>
      </div>
    </div>
    <button id="btn-test-connection" class="gc-button-secondary self-start" type="button">연결 테스트</button>
    <div id="ssh-test-result"></div>
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
