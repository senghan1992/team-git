import { ipc, ExternalTool, type Repo } from "../lib/ipc";
import { normalizePort, renderSshTestReport, runSshTest } from "../lib/sshTest";
import { openModal, confirmDialog } from "../components/Modal";
import { toast } from "../components/Toast";
import { icon } from "../components/Icon";
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
  sub.textContent =
    "AI 자동 병합, SSH 연결, 푸시 자격증명, 외부 도구를 관리합니다. 팀 규칙(병합 대상 브랜치·병합 관리자·구성원)은 저장소 → 설정 탭에서 정합니다.";
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
    <div class="text-display-sm text-[color:var(--color-ink-muted)]">
      저장소가 <strong>다른 서버에 있을 때만</strong> 필요합니다. 저장소를 등록할 때
      호스트·사용자·키를 매번 입력하지 않도록 기본값을 여기에 둡니다.
      내 컴퓨터의 폴더만 쓴다면 비워 둬도 됩니다 — 아래 “—”는 오류가 아닙니다.
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
  //
  // 여기는 **목록 관리**만 한다. 실행은 저장소 화면 우측 상단의 "열기" 버튼이
  // 맡는다 — 사람은 저장소를 보고 있을 때 "이걸 에디터로 열자"고 생각하므로,
  // 설정에 들어와 저장소를 골라 실행하게 만들면 순서가 거꾸로다.
  const toolsSection = document.createElement("div");
  toolsSection.className = "flex flex-col gap-4";
  toolsSection.innerHTML = `
    <div class="flex flex-col gap-1">
      <div class="text-display-md font-medium">외부 도구</div>
      <div class="text-display-sm text-[color:var(--color-ink-muted)]">
        저장소 폴더를 다른 프로그램으로 여는 명령입니다. 명령의
        <code>{path}</code>가 저장소 경로로 바뀝니다 — 예: <code>code {path}</code>는
        그 저장소를 VS Code로 엽니다.
        <strong>실행은 저장소 화면 오른쪽 위의 “열기” 버튼</strong>에서 합니다.
      </div>
      <div class="text-display-xs text-[color:var(--color-ink-muted)]">
        명령은 이 앱이 실행 중인 컴퓨터에서 돌아갑니다. 그래서 SSH로 등록한
        저장소(작업 트리가 원격 서버에 있음)에서는 쓸 수 없고, 그 저장소에는
        “열기” 버튼이 나타나지 않습니다.
      </div>
    </div>
  `;
  main.appendChild(toolsSection);

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
          <button class="gc-button-secondary text-display-sm" data-edit>편집</button>
          <button class="gc-button-secondary text-display-sm text-[color:var(--color-danger)]" data-delete>삭제</button>
        </div>
      `;

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

  // ── AI 자동 병합 (시나리오 5) ─────────────────────────────────────────────
  // 병합 중 충돌이 났을 때 AI가 알아서 고치게 하려면 **미리** 두 가지를 정해
  // 둬야 한다: (1) 충돌 시 자동으로 돌릴지, (2) 어떤 지침(프롬프트)으로 고칠지.
  // 그래서 이 카드는 접속 정보와 함께 그 두 개를 같은 자리에서 저장한다.
  const aiSection = document.createElement("section");
  aiSection.className = "gc-card flex flex-col gap-3";
  aiSection.innerHTML = `
    <div class="text-display-lg font-medium inline-flex items-center gap-2"><span id="ai-title-icon"></span><span>AI 자동 병합</span></div>
    <div class="text-display-sm text-[color:var(--color-ink-muted)]">
      병합하다 충돌이 나면 AI가 양쪽 수정을 모두 살리는 코드를 만들어 병합을 마무리합니다.
      원본은 항상 백업되고, 충돌 표시가 남은 파일은 절대 커밋되지 않습니다.
      기본값은 <strong>비활성</strong>입니다.
    </div>

    <label class="gc-check">
      <input id="ai-enabled" type="checkbox" />
      <span class="text-display-md">AI 사용함</span>
    </label>

    <div id="ai-conn" class="flex flex-col gap-3 pl-1">
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

      <label class="gc-check">
        <input id="ai-auto" type="checkbox" />
        <span class="flex flex-col">
          <span class="text-display-md">충돌이 나면 곧바로 자동 해결</span>
          <span class="text-display-xs text-[color:var(--color-ink-muted)]">
            병합 중 충돌이 감지되면 버튼을 누르지 않아도 아래 지침대로 바로 고칩니다.
            끄면 병합 화면에서 “AI 자동 병합” 버튼으로 직접 실행합니다.
          </span>
        </span>
      </label>

      <label class="gc-check">
        <input id="ai-auto-push" type="checkbox" />
        <span class="flex flex-col">
          <span class="text-display-md">자동 해결 후 곧바로 push</span>
          <span class="text-display-xs text-[color:var(--color-ink-muted)]">
            AI가 고친 병합도 확인 단계 없이 병합 브랜치에 push하고 팀원에게 동기화 알림을 보냅니다 —
            커밋 메시지에 무슨 충돌을 어떻게 풀었는지 기록됩니다.
            끄면 결과를 확인한 뒤 “확인했어요 — push”를 눌러야 팀에 반영됩니다.
          </span>
        </span>
      </label>

      <label class="flex flex-col gap-1">
        <span class="text-display-sm text-[color:var(--color-ink-muted)]">바이너리·대용량 파일은 어느 쪽을 쓸까요</span>
        <select id="ai-binary" class="gc-input">
          <option value="theirs">상대 것(가져온 브랜치) — 기본</option>
          <option value="ours">나의 것(병합 대상 브랜치)</option>
        </select>
      </label>

      <label class="flex flex-col gap-1">
        <span class="text-display-sm text-[color:var(--color-ink-muted)] flex items-center justify-between gap-2">
          <span>해결 지침(프롬프트) — 미리 저장해 두면 충돌마다 이 지침으로 고칩니다</span>
          <button id="ai-prompt-reset" type="button" class="gc-button-secondary text-display-xs">기본값으로</button>
        </span>
        <textarea id="ai-prompt" class="gc-input font-mono text-display-sm" rows="6"
          spellcheck="false"></textarea>
        <span class="text-display-xs text-[color:var(--color-ink-muted)]">
          비워 두면 기본 지침을 씁니다. 팀 규칙(예: “API 시그니처는 상대 쪽을 따른다”,
          “마이그레이션 파일은 절대 합치지 말고 양쪽을 모두 남긴다”)을 여기에 적어 두세요.
        </span>
      </label>
    </div>

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

  const aiEnabledBox = aiSection.querySelector<HTMLInputElement>("#ai-enabled")!;
  const aiConn = aiSection.querySelector<HTMLElement>("#ai-conn")!;
  const aiPrompt = aiSection.querySelector<HTMLTextAreaElement>("#ai-prompt")!;
  let aiDefaultPrompt = "";

  // AI를 안 쓰는 사람에게는 세부 항목을 숨겨 화면을 비워 둔다.
  function syncAiDisclosure() {
    aiConn.style.display = aiEnabledBox.checked ? "" : "none";
  }
  aiEnabledBox.addEventListener("change", syncAiDisclosure);

  aiSection.querySelector<HTMLButtonElement>("#ai-prompt-reset")!
    .addEventListener("click", () => {
      aiPrompt.value = aiDefaultPrompt;
      aiPrompt.focus();
    });

  try {
    const [aiCfg, defaultPrompt] = await Promise.all([
      ipc.getAiConfig(),
      ipc.aiDefaultPrompt().catch(() => ""),
    ]);
    aiDefaultPrompt = defaultPrompt;
    aiEnabledBox.checked = !!aiCfg.enabled;
    (aiSection.querySelector<HTMLInputElement>("#ai-base-url")!).value = aiCfg.base_url ?? "";
    (aiSection.querySelector<HTMLInputElement>("#ai-model")!).value = aiCfg.model ?? "";
    (aiSection.querySelector<HTMLInputElement>("#ai-api-key")!).value = aiCfg.api_key ?? "";
    (aiSection.querySelector<HTMLInputElement>("#ai-auto")!).checked = !!aiCfg.auto_resolve;
    (aiSection.querySelector<HTMLInputElement>("#ai-auto-push")!).checked = !!aiCfg.auto_push;
    (aiSection.querySelector<HTMLSelectElement>("#ai-binary")!).value =
      aiCfg.binary_strategy === "ours" ? "ours" : "theirs";
    // 저장된 지침이 없으면 기본 지침을 채워 보여 준다 — 무엇을 편집하는지
    // 눈으로 보이는 편이 빈 칸보다 훨씬 안전하다.
    aiPrompt.value = (aiCfg.system_prompt ?? "").trim() || defaultPrompt;
    aiPrompt.placeholder = defaultPrompt;
  } catch (e) {
    toast(`AI 설정 불러오기 실패: ${(e as Error).message ?? e}`, "error");
  }
  syncAiDisclosure();

  aiSection.querySelector<HTMLButtonElement>("#ai-save")!.addEventListener("click", async () => {
    const typedPrompt = aiPrompt.value.trim();
    const cfg = {
      enabled: aiEnabledBox.checked,
      base_url: aiSection.querySelector<HTMLInputElement>("#ai-base-url")!.value.trim(),
      api_key: aiSection.querySelector<HTMLInputElement>("#ai-api-key")!.value,
      model: aiSection.querySelector<HTMLInputElement>("#ai-model")!.value.trim(),
      // 기본 지침과 같으면 빈 값으로 저장해, 기본값이 바뀌면 자동으로 따라간다.
      system_prompt: typedPrompt === aiDefaultPrompt.trim() ? "" : typedPrompt,
      auto_resolve: aiSection.querySelector<HTMLInputElement>("#ai-auto")!.checked,
      auto_push: aiSection.querySelector<HTMLInputElement>("#ai-auto-push")!.checked,
      binary_strategy: aiSection.querySelector<HTMLSelectElement>("#ai-binary")!.value === "ours"
        ? "ours"
        : "theirs",
    };
    if (cfg.enabled && (!cfg.base_url || !cfg.model)) {
      toast("AI를 사용하려면 Base URL과 모델명을 입력하세요.", "error");
      return;
    }
    try {
      await ipc.setAiConfig(cfg);
      toast(
        cfg.enabled && cfg.auto_resolve
          ? cfg.auto_push
            ? "저장했습니다. 이제 충돌이 나면 AI가 해결·커밋·push까지 자동으로 진행합니다."
            : "저장했습니다. 이제 충돌이 나면 AI가 곧바로 해결을 시도합니다."
          : "AI 설정을 저장했습니다.",
        "success",
      );
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

  async function openCredModal(
    repoId: string | null,
    existing?: { username: string; password: string },
  ) {
    // 저장소 목록은 이 모달만 쓴다 — 열 때 읽어서 최신 목록을 보여 준다.
    const repos = await ipc.listRepositories().catch(() => [] as Repo[]);
    // 자격증명은 저장소마다 저장된다. 저장소가 하나도 없으면 선택 상자가 빈
    // 채로 열리고, 저장을 누르면 "저장소를 선택하세요"만 뜬다 — 고를 것이
    // 없다는 사실을 먼저 말해 준다.
    if (repos.length === 0) {
      toast("먼저 저장소를 등록하세요. 자격증명은 저장소별로 저장됩니다.", "info");
      return;
    }
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
