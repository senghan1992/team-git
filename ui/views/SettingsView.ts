import { ipc, ExternalTool, type Repo } from "../lib/ipc";
import { normalizePort, renderSshTestReport, runSshTest } from "../lib/sshTest";
import { openModal, confirmDialog } from "../components/Modal";
import { toast } from "../components/Toast";
import { icon } from "../components/Icon";
import { setBusy } from "../components/Busy";
interface ToolEntry {
  id: string;
  label: string;
  command_template: string;
  args_template: string;
}

const DEFAULT_TOOLS: ToolEntry[] = [
  { id: "code", label: "VS Code", command_template: "code", args_template: "{path}" },
  { id: "cursor", label: "Cursor", command_template: "cursor", args_template: "{path}" },
  { id: "sublime", label: "Sublime Text", command_template: "subl", args_template: "{path}" },
  { id: "gnome-terminal", label: "GNOME Terminal", command_template: "gnome-terminal", args_template: "--working-directory={path}" },
  { id: "xterm", label: "XTerm", command_template: "xterm", args_template: '-e "cd {path} && bash"' },
  { id: "tmux", label: "Tmux", command_template: "tmux", args_template: 'new-session -c {path}' },
];

export async function renderSettingsView(): Promise<HTMLElement> {
  const main = document.createElement("main");
  main.className = "flex-1 overflow-y-auto p-8 flex flex-col gap-6";

  const head = document.createElement("div");
  head.className = "gc-page-head";
  const title = document.createElement("div");
  title.className = "gc-page-head__title";
  title.textContent = "설정";
  head.appendChild(title);
  const sub = document.createElement("div");
  sub.className = "gc-page-head__sub";
  sub.textContent = "연결, SSH 프로필, 외부 도구를 관리합니다.";
  head.appendChild(sub);
  main.appendChild(head);

  // ── SSH Profile display card ─────────────────────────────────────────────
  const sshDisplayCard = document.createElement("div");
  sshDisplayCard.className = "gc-card flex flex-col gap-3";
  sshDisplayCard.innerHTML = `
    <div class="flex items-center justify-between">
      <div class="text-display-md font-medium inline-flex items-center gap-2" id="ssh-title-row"><span id="ssh-title-icon"></span><span>SSH 프로필</span></div>
      <div class="flex gap-2">
        <button class="gc-button-secondary text-display-sm" id="btn-test-ssh">연결 테스트</button>
        <button class="gc-button-secondary text-display-sm" id="btn-edit-ssh">편집</button>
      </div>
    </div>
    <div class="flex flex-col gap-2 text-display-sm" id="ssh-fields">
      <div class="flex gap-2"><span class="text-[color:var(--color-ink-muted)] w-32 shrink-0">사용자:</span><span id="ssh-d-user">—</span></div>
      <div class="flex gap-2"><span class="text-[color:var(--color-ink-muted)] w-32 shrink-0">호스트:</span><span id="ssh-d-host">—</span></div>
      <div class="flex gap-2"><span class="text-[color:var(--color-ink-muted)] w-32 shrink-0">키 경로:</span><span id="ssh-d-key">—</span></div>
      <div class="flex gap-2"><span class="text-[color:var(--color-ink-muted)] w-32 shrink-0">비밀번호:</span><span id="ssh-d-pw">—</span></div>
      <div class="flex gap-2"><span class="text-[color:var(--color-ink-muted)] w-32 shrink-0">타임아웃:</span><span id="ssh-d-timeout">—</span></div>
      <div class="flex gap-2"><span class="text-[color:var(--color-ink-muted)] w-32 shrink-0">포트:</span><span id="ssh-d-port">—</span></div>
    </div>
  `;
  sshDisplayCard.querySelector<HTMLElement>("#ssh-title-icon")!.appendChild(icon("settings", 16));
  main.appendChild(sshDisplayCard);

  let currentSshProfile = { default_user: "", default_key_path: "", default_password: "", default_host: "", connect_timeout: "5", default_port: 22 };

  async function refreshSshCard() {
    try {
      const profile = await ipc.getSshProfile();
      currentSshProfile = profile;
      (sshDisplayCard.querySelector("#ssh-d-user")!).textContent = profile.default_user || "—";
      (sshDisplayCard.querySelector("#ssh-d-host")!).textContent = profile.default_host || "—";
      (sshDisplayCard.querySelector("#ssh-d-key")!).textContent = profile.default_key_path || "—";
      (sshDisplayCard.querySelector("#ssh-d-pw")!).textContent = profile.default_password ? "•••• 설정됨" : "—";
      (sshDisplayCard.querySelector("#ssh-d-timeout")!).textContent = String(profile.connect_timeout ?? "5") + "초";
      (sshDisplayCard.querySelector("#ssh-d-port")!).textContent = String(profile.default_port ?? 22);
    } catch {
      currentSshProfile = { default_user: "", default_key_path: "", default_password: "", default_host: "", connect_timeout: "5", default_port: 22 };
    }
  }

  await refreshSshCard();

  sshDisplayCard.querySelector<HTMLButtonElement>("#btn-test-ssh")!.addEventListener("click", () => {
    const tm = openModal({
      title: "SSH 연결 테스트",
      submitLabel: "테스트",
      onSubmit: async () => {
        const host = (tm.body.querySelector<HTMLInputElement>("#test-host")!).value.trim();
        if (!host) {
          tm.setError("호스트를 입력하세요.");
          return;
        }
        tm.setError(null);
        try {
          const report = await runSshTest({
            host,
            user: (tm.body.querySelector<HTMLInputElement>("#test-user")!).value.trim(),
            port: normalizePort((tm.body.querySelector<HTMLInputElement>("#test-port")!).value),
            key_path: (tm.body.querySelector<HTMLInputElement>("#test-key")!).value.trim(),
            password: (tm.body.querySelector<HTMLInputElement>("#test-password")!).value,
          });
          renderSshTestReport(tm.body.querySelector<HTMLElement>("#test-result")!, report);
        } catch (e) {
          tm.setError(String(e));
        }
      },
    });

    tm.body.innerHTML = `
      <div class="flex flex-col gap-1">
        <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="test-host">SSH 호스트</label>
        <input id="test-host" class="gc-input" type="text" placeholder="예: dev.example.com" value="${escape(currentSshProfile.default_host)}" />
      </div>
      <div class="flex flex-col gap-1">
        <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="test-port">포트</label>
        <input id="test-port" class="gc-input" type="number" min="1" max="65535" placeholder="예: 22" value="${escape(String(currentSshProfile.default_port ?? 22))}" />
      </div>
      <div class="flex flex-col gap-1">
        <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="test-user">SSH 사용자</label>
        <input id="test-user" class="gc-input" type="text" placeholder="예: ubuntu" value="${escape(currentSshProfile.default_user)}" />
      </div>
      <div class="flex flex-col gap-1">
        <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="test-key">SSH 키 경로</label>
        <input id="test-key" class="gc-input" type="text" placeholder="예: ~/.ssh/id_ed25519" value="${escape(currentSshProfile.default_key_path)}" />
      </div>
      <div class="flex flex-col gap-1">
        <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="test-password">SSH 비밀번호 (선택, 키 대신 사용 시)</label>
        <input id="test-password" class="gc-input" type="password" autocomplete="off" value="${escape(currentSshProfile.default_password ?? "")}" />
      </div>
      <div id="test-result"></div>
    `;
  });

  sshDisplayCard.querySelector<HTMLButtonElement>("#btn-edit-ssh")!.addEventListener("click", () => {
    const m = openModal({
      title: "SSH 프로필 편집",
      onSubmit: async (close) => {
        const default_user = (m.body.querySelector<HTMLInputElement>("#ssh-default-user")!).value.trim();
        const default_key_path = (m.body.querySelector<HTMLInputElement>("#ssh-default-key")!).value.trim();
        const default_password = (m.body.querySelector<HTMLInputElement>("#ssh-default-password")!).value;
        const connect_timeout = (m.body.querySelector<HTMLInputElement>("#ssh-timeout")!).value.trim() || "5";
        const default_host = (m.body.querySelector<HTMLInputElement>("#ssh-default-host")!).value.trim();
        const default_port = normalizePort(m.body.querySelector<HTMLInputElement>("#ssh-default-port")!.value);
        try {
          await ipc.setSshProfile({ default_user, default_key_path, default_password, default_host, connect_timeout, default_port });
          await refreshSshCard();
          close();
        } catch (e) {
          m.setError(`저장 실패: ${(e as Error).message ?? e}`);
        }
      },
    });

    m.body.innerHTML = `
      <div class="flex flex-col gap-1">
        <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="ssh-default-user">기본 SSH 사용자</label>
        <input id="ssh-default-user" class="gc-input" type="text" placeholder="예: ubuntu" value="${escape(currentSshProfile.default_user)}" />
      </div>
      <div class="flex flex-col gap-1">
        <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="ssh-default-key">기본 키 경로</label>
        <input id="ssh-default-key" class="gc-input" type="text" placeholder="예: ~/.ssh/id_ed25519" value="${escape(currentSshProfile.default_key_path)}" />
      </div>
      <div class="flex flex-col gap-1">
        <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="ssh-default-password">기본 비밀번호 (선택)</label>
        <input id="ssh-default-password" class="gc-input" type="password" autocomplete="off" value="${escape(currentSshProfile.default_password ?? "")}" />
      </div>
      <div class="flex flex-col gap-1">
        <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="ssh-default-host">기본 SSH 호스트</label>
        <input id="ssh-default-host" class="gc-input" type="text" placeholder="예: github.com" value="${escape(currentSshProfile.default_host)}" />
      </div>
      <div class="flex flex-col gap-1">
        <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="ssh-timeout">연결 타임아웃 (초)</label>
        <input id="ssh-timeout" class="gc-input" type="number" min="1" max="60" value="${escape(currentSshProfile.connect_timeout)}" />
      </div>
      <div class="flex flex-col gap-1">
        <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="ssh-default-port">기본 포트</label>
        <input id="ssh-default-port" class="gc-input" type="number" min="1" max="65535" placeholder="예: 22" value="${escape(String(currentSshProfile.default_port ?? 22))}" />
      </div>
    `;
  });

  // ── External Tools ───────────────────────────────────────────────────────
  const toolsSection = document.createElement("div");
  toolsSection.className = "flex flex-col gap-4";
  toolsSection.innerHTML = `
    <div class="flex items-center justify-between">
      <div class="text-display-md font-medium">외부 도구</div>
    </div>
  `;
  main.appendChild(toolsSection);

  // ── Repo picker row ──────────────────────────────────────────────────────
  const repoPicker = document.createElement("div");
  repoPicker.className = "flex items-center gap-3";
  repoPicker.innerHTML = `
    <label class="text-display-sm text-[color:var(--color-ink-muted)] shrink-0" for="tool-repo-select">저장소 선택:</label>
    <select id="tool-repo-select" class="gc-input flex-1"></select>
    <span id="no-repo-hint" class="text-display-sm text-[color:var(--color-ink-muted)] hidden">저장소를 먼저 등록하세요</span>
  `;
  toolsSection.appendChild(repoPicker);

  const repos = await ipc.listRepositories();
  const repoSel = repoPicker.querySelector<HTMLSelectElement>("#tool-repo-select")!;
  const noRepoHint = repoPicker.querySelector("#no-repo-hint")!;
  if (repos.length === 0) {
    repoSel.style.display = "none";
    noRepoHint.classList.remove("hidden");
  } else {
    for (const r of repos) {
      const opt = document.createElement("option");
      opt.value = r.id;
      opt.textContent = r.display_name;
      repoSel.appendChild(opt);
    }
  }

  // ── Tool card grid ──────────────────────────────────────────────────────
  const grid = document.createElement("div");
  grid.className = "grid grid-cols-1 md:grid-cols-2 gap-4";
  toolsSection.appendChild(grid);

  let tools: ToolEntry[] = [...DEFAULT_TOOLS];
  try {
    const saved = await ipc.listExternalTools();
    if (saved.length > 0) {
      tools = saved.map((t: ExternalTool) => ({
        id: t.id,
        label: t.label,
        command_template: t.command_template,
        args_template: t.args_template,
      }));
    }
  } catch { /* use defaults */ }

  function openToolModal(tool?: ToolEntry) {
    const isEdit = !!tool;
    const editingId = tool?.id ?? crypto.randomUUID();

    const m = openModal({
      title: isEdit ? "외부 도구 편집" : "외부 도구 추가",
      onSubmit: async (close) => {
        const label = (m.body.querySelector<HTMLInputElement>("#tool-label")!).value.trim();
        const command_template = (m.body.querySelector<HTMLInputElement>("#tool-cmd")!).value.trim();
        const args_template = (m.body.querySelector<HTMLInputElement>("#tool-args")!).value.trim();
        if (!label || !command_template) {
          m.setError("라벨과 명령을 입력하세요.");
          return;
        }
        try {
          await ipc.setExternalTool({ id: editingId, label, command_template, args_template, enabled: true });
          await refreshTools();
          close();
        } catch (e) {
          m.setError(`저장 실패: ${(e as Error).message ?? e}`);
        }
      },
    });

    m.body.innerHTML = `
      <div class="flex flex-col gap-1">
        <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="tool-label">라벨</label>
        <input id="tool-label" class="gc-input" type="text" placeholder="예: VS Code" value="${escape(tool?.label ?? "")}" />
      </div>
      <div class="flex flex-col gap-1">
        <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="tool-cmd">명령</label>
        <input id="tool-cmd" class="gc-input" type="text" placeholder="예: code" value="${escape(tool?.command_template ?? "")}" />
      </div>
      <div class="flex flex-col gap-1">
        <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="tool-args">인수 템플릿</label>
        <input id="tool-args" class="gc-input" type="text" placeholder="예: {path}" value="${escape(tool?.args_template ?? "")}" />
      </div>
    `;
  }

  async function refreshTools() {
    try {
      const saved = await ipc.listExternalTools();
      if (saved.length > 0) {
        tools = saved.map((t: ExternalTool) => ({
          id: t.id,
          label: t.label,
          command_template: t.command_template,
          args_template: t.args_template,
        }));
      }
    } catch { /* keep current */ }

    grid.innerHTML = "";

    for (const tool of tools) {
      const card = document.createElement("div");
      card.className = "gc-card flex flex-col gap-2";
      card.innerHTML = `
        <div class="text-display-md font-medium">${escape(tool.label)}</div>
        <div class="text-display-sm text-[color:var(--color-ink-muted)] flex flex-col gap-1">
          <div><span class="font-mono">${escape(tool.command_template)}</span> ${escape(tool.args_template)}</div>
        </div>
        <div class="flex gap-2 mt-1">
          <button class="gc-button-secondary text-display-sm" data-run>실행</button>
          <button class="gc-button-secondary text-display-sm" data-edit>편집</button>
          <button class="gc-button-secondary text-display-sm text-[color:var(--color-danger)]" data-delete>삭제</button>
        </div>
      `;

      const runBtn = card.querySelector<HTMLButtonElement>("[data-run]")!;
      runBtn.addEventListener("click", async () => {
        const repoId = repoSel.value;
        if (!repoId) { toast("저장소를 선택하세요", "error"); return; }
        setBusy(runBtn, true, "실행 중…");
        try {
          await ipc.openExternalTool(repoId, tool.id);
          toast(`${tool.label} 실행 완료`, "success");
        } catch (e) {
          toast(`실행 실패: ${(e as Error).message ?? e}`, "error");
        } finally {
          setBusy(runBtn, false);
        }
      });
      card.querySelector<HTMLButtonElement>("[data-edit]")!.addEventListener("click", () => {
        openToolModal(tool);
      });

      card.querySelector<HTMLButtonElement>("[data-delete]")!.addEventListener("click", async () => {
        const confirmed = await confirmDialog({
          title: "외부 도구 삭제",
          message: `"${tool.label}"을(를) 삭제하시겠습니까?`,
          confirmLabel: "삭제",
          destructive: true,
        });
        if (!confirmed) return;
        try {
          await ipc.removeExternalTool(tool.id);
          await refreshTools();
          toast("삭제 완료", "success");
        } catch (e) {
          toast(`삭제 실패: ${(e as Error).message ?? e}`, "error");
        }
      });

      grid.appendChild(card);
    }

    // "Add tool" card
    const addCard = document.createElement("div");
    addCard.className = "gc-card flex flex-col items-center justify-center gap-2 py-10 text-[color:var(--color-ink-muted)] hover:bg-[color:var(--color-surface-2)] hover:border-[color:var(--color-border-strong)] transition-colors";
    addCard.style.cursor = "pointer";
    const addTile = document.createElement("span");
    addTile.className = "inline-flex items-center justify-center w-9 h-9 rounded-[10px] bg-[color:var(--color-clay)] border border-[color:var(--color-hairline)]";
    addTile.appendChild(icon("plus", 16));
    addCard.appendChild(addTile);
    const addLabel = document.createElement("span");
    addLabel.className = "text-display-sm font-medium";
    addLabel.textContent = "외부 도구 추가";
    addCard.appendChild(addLabel);
    addCard.addEventListener("click", () => openToolModal());
    grid.appendChild(addCard);
  }

  await refreshTools();

  // ── AI conflict resolution (optional) ────────────────────────────────────
  const aiSection = document.createElement("section");
  aiSection.className = "gc-card flex flex-col gap-3";
  aiSection.innerHTML = `
    <div class="text-display-lg font-medium inline-flex items-center gap-2"><span id="ai-title-icon"></span><span>AI 충돌 해결(선택)</span></div>
    <div class="text-display-sm text-[color:var(--color-ink-muted)]">
      OpenAI 호환 /chat/completions 엔드포인트에 ours/theirs 본문을 보내고
      병합 제안을 받습니다. 기본값은 <strong>비활성</strong>입니다.
    </div>
    <div class="flex items-center gap-2">
      <input id="ai-enabled" type="checkbox" />
      <label for="ai-enabled" class="text-display-md">사용함</label>
    </div>
    <label class="flex flex-col gap-1">
      <span class="text-display-sm text-[color:var(--color-ink-muted)]">Base URL</span>
      <input id="ai-base-url" class="gc-input" placeholder="https://api.openai.com/v1" />
    </label>
    <label class="flex flex-col gap-1">
      <span class="text-display-sm text-[color:var(--color-ink-muted)]">모델</span>
      <input id="ai-model" class="gc-input" placeholder="gpt-4o-mini" />
    </label>
    <label class="flex flex-col gap-1">
      <span class="text-display-sm text-[color:var(--color-ink-muted)]">API 키</span>
      <input id="ai-api-key" type="password" class="gc-input" autocomplete="off" />
    </label>
    <div class="text-display-xs text-[color:var(--color-ink-muted)]">
      키는 <code>~/.config/com.gitcompanion.app/config.json</code>에 평문 저장됩니다.
      사용 후 비워 두려면 비활성화하세요.
    </div>
    <div class="flex justify-end">
      <button id="ai-save" class="gc-button-primary">저장</button>
    </div>
  `;
  aiSection.querySelector<HTMLElement>("#ai-title-icon")!.appendChild(icon("sparkles", 18));
  main.appendChild(aiSection);

  try {
    const aiCfg = await ipc.getAiConfig();
    (aiSection.querySelector<HTMLInputElement>("#ai-enabled")!).checked = aiCfg.enabled;
    (aiSection.querySelector<HTMLInputElement>("#ai-base-url")!).value = aiCfg.base_url;
    (aiSection.querySelector<HTMLInputElement>("#ai-model")!).value = aiCfg.model;
    (aiSection.querySelector<HTMLInputElement>("#ai-api-key")!).value = aiCfg.api_key;
  } catch (e) {
    toast(`AI 설정 불러오기 실패: ${(e as Error).message ?? e}`, "error");
  }

  aiSection.querySelector<HTMLButtonElement>("#ai-save")!.addEventListener("click", async () => {
    const cfg = {
      enabled: aiSection.querySelector<HTMLInputElement>("#ai-enabled")!.checked,
      base_url: aiSection.querySelector<HTMLInputElement>("#ai-base-url")!.value.trim(),
      api_key: aiSection.querySelector<HTMLInputElement>("#ai-api-key")!.value,
      model: aiSection.querySelector<HTMLInputElement>("#ai-model")!.value.trim(),
    };
    try {
      await ipc.setAiConfig(cfg);
      toast("AI 설정을 저장했습니다.", "success");
    } catch (e) {
      toast(`저장 실패: ${(e as Error).message ?? e}`, "error");
    }
  });

  // ── 푸시 자격증명 (저장된 Git 호스트 아이디/비밀번호) ───────────────────────
  const credSection = document.createElement("section");
  credSection.className = "gc-card flex flex-col gap-3";
  credSection.innerHTML = `
    <div class="flex items-center justify-between">
      <div class="text-display-lg font-medium">푸시 자격증명</div>
      <button class="gc-button-primary text-display-sm" id="btn-add-cred">+ 추가</button>
    </div>
    <div class="text-display-sm text-[color:var(--color-ink-muted)]">
      HTTPS 원격 저장소에 푸시할 때 쓰는 Git 호스트 아이디/비밀번호입니다. 저장하면
      푸시 시 모달 없이 자동 입력되며, <code>~/.config/com.gitcompanion.app/config.json</code>에
      저장됩니다. SSH(키) 방식 저장소는 이 항목이 필요 없습니다.
    </div>
    <div id="cred-list" class="flex flex-col gap-2"></div>
  `;
  main.appendChild(credSection);

  async function renderCredList() {
    const list = credSection.querySelector<HTMLElement>("#cred-list")!;
    const [saved, allRepos] = await Promise.all([
      ipc.pushCredentialsList().catch(() => ({}) as Record<string, { username: string; password: string }>),
      ipc.listRepositories().catch(() => [] as Repo[]),
    ]);
    list.innerHTML = "";
    const entries = Object.entries(saved);
    if (entries.length === 0) {
      list.innerHTML = `<div class="text-display-sm text-[color:var(--color-ink-muted)]">저장된 자격증명이 없습니다.</div>`;
    }
    for (const [repoId, cred] of entries) {
      const repo = allRepos.find((r) => r.id === repoId);
      const row = document.createElement("div");
      row.className = "flex items-center gap-2 border border-[color:var(--color-hairline)] rounded-md px-3 py-2";
      const label = document.createElement("span");
      label.className = "flex-1 min-w-0 truncate text-display-sm";
      label.textContent = `${repo?.display_name ?? repoId} — ${cred.username} / ••••`;
      row.appendChild(label);
      const edit = document.createElement("button");
      edit.className = "gc-button-secondary text-display-sm";
      edit.textContent = "편집";
      edit.addEventListener("click", () => openCredModal(repoId, cred));
      row.appendChild(edit);
      const del = document.createElement("button");
      del.className = "gc-button-secondary text-display-sm text-[color:var(--color-danger)]";
      del.textContent = "삭제";
      del.addEventListener("click", async () => {
        await ipc.pushCredentialDelete(repoId);
        await renderCredList();
        toast("자격증명을 삭제했습니다.", "success");
      });
      row.appendChild(del);
      list.appendChild(row);
    }
  }

  function openCredModal(repoId: string | null, existing?: { username: string; password: string }) {
    const m = openModal({
      title: existing ? "자격증명 편집" : "자격증명 추가",
      submitLabel: "저장",
      onSubmit: async (close) => {
        const rid = (m.body.querySelector<HTMLSelectElement>("#cred-repo")!).value;
        const username = (m.body.querySelector<HTMLInputElement>("#cred-user")!).value.trim();
        const password = (m.body.querySelector<HTMLInputElement>("#cred-pass")!).value;
        if (!rid) { m.setError("저장소를 선택하세요."); return; }
        if (!username || !password) { m.setError("아이디와 비밀번호를 입력하세요."); return; }
        m.setSubmitting(true);
        m.setError(null);
        try {
          await ipc.pushCredentialSet(rid, { username, password });
          await renderCredList();
          toast("자격증명을 저장했습니다.", "success");
          close();
        } catch (e) {
          m.setError(`저장 실패: ${(e as Error).message ?? e}`);
          m.setSubmitting(false);
        }
      },
    });

    const repoOpts = repos
      .map((r) => `<option value="${escape(r.id)}" ${r.id === repoId ? "selected" : ""}>${escape(r.display_name)}</option>`)
      .join("");
    m.body.innerHTML = `
      <div class="flex flex-col gap-1">
        <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="cred-repo">저장소</label>
        <select id="cred-repo" class="gc-input">${repoOpts}</select>
      </div>
      <div class="flex flex-col gap-1">
        <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="cred-user">아이디</label>
        <input id="cred-user" class="gc-input" type="text" autocomplete="username" value="${escape(existing?.username ?? "")}" />
      </div>
      <div class="flex flex-col gap-1">
        <label class="text-display-sm text-[color:var(--color-ink-muted)]" for="cred-pass">비밀번호 / 토큰</label>
        <input id="cred-pass" class="gc-input" type="password" autocomplete="new-password" value="${escape(existing?.password ?? "")}" />
      </div>
    `;
  }

  credSection.querySelector<HTMLButtonElement>("#btn-add-cred")!.addEventListener("click", () => openCredModal(null));
  await renderCredList();

  const footer = document.createElement("div");
  footer.className = "mt-auto pt-4 text-display-xs text-[color:var(--color-ink-muted)]";
  footer.textContent = "Git Companion v0.1.0";
  main.appendChild(footer);

  return main;
}

function escape(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}
