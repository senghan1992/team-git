// "이 저장소를 에디터로 열기" 버튼.
//
// 외부 도구(`{path}` 를 저장소 경로로 치환해 명령을 실행)는 원래 설정 화면에만
// 있었고, 거기서 저장소를 드롭다운으로 골라 실행하게 되어 있었다. 순서가
// 거꾸로다 — 사람은 저장소를 보고 있을 때 "이걸 에디터로 열자"고 생각한다.
// 그래서 실행은 저장소 화면으로 오고, 설정에는 도구 목록 관리만 남는다.
import { ipc, type ExternalTool, type Repo } from "../lib/ipc";
import { toast } from "./Toast";
import { icon } from "./Icon";
import { setBusy } from "./Busy";

export interface OpenWithOpts {
  /** 설정 화면으로 보내는 콜백 (도구 목록을 편집하러). */
  onEditTools?: () => void;
}

/**
 * 도구가 하나면 바로 실행되는 버튼, 여러 개면 목록이 열리는 버튼을 만든다.
 * 도구가 없거나 SSH 저장소면 `null` — 붙일 것이 없다.
 */
export async function renderOpenWithButton(
  repo: Repo,
  opts: OpenWithOpts = {},
): Promise<HTMLElement | null> {
  // SSH 저장소는 작업 트리가 원격 서버에 있어 이 컴퓨터의 도구로 열 수 없다.
  // 눌러 보고 실패하게 두지 않고 버튼 자체를 내보내지 않는다.
  if (repo.ssh_host) return null;

  let tools: ExternalTool[] = [];
  try {
    tools = (await ipc.listExternalTools()).filter((t) => t.enabled);
  } catch {
    return null;
  }
  if (tools.length === 0) return null;

  async function run(tool: ExternalTool, btn: HTMLButtonElement) {
    setBusy(btn, true, "실행 중…");
    try {
      await ipc.openExternalTool(repo.id, tool.id);
      toast(`${tool.label}에서 열었습니다.`, "success");
    } catch (e) {
      // 명령이 PATH 에 없을 때가 가장 흔하다 — 무엇을 실행하려 했는지 보여 준다.
      toast(
        `${tool.label} 실행 실패: ${(e as Error).message ?? e} (명령: ${tool.command_template})`,
        "error",
      );
    } finally {
      setBusy(btn, false);
    }
  }

  const wrap = document.createElement("div");
  wrap.className = "relative shrink-0";

  const main = document.createElement("button");
  main.className = "gc-button-secondary inline-flex items-center gap-1.5";
  main.appendChild(icon("edit", 14));
  const label = document.createElement("span");
  label.textContent = tools.length === 1 ? `${tools[0]!.label}로 열기` : "열기";
  main.appendChild(label);
  wrap.appendChild(main);

  if (tools.length === 1) {
    main.addEventListener("click", () => void run(tools[0]!, main));
    return wrap;
  }

  // 여러 개 — 목록을 띄운다.
  const menu = document.createElement("div");
  menu.className = "gc-menu";
  menu.style.display = "none";
  wrap.appendChild(menu);

  for (const t of tools) {
    const item = document.createElement("button");
    item.className = "gc-menu__item";
    const nm = document.createElement("span");
    nm.className = "flex-1 text-left";
    nm.textContent = t.label;
    item.appendChild(nm);
    const cmd = document.createElement("span");
    cmd.className = "font-mono text-display-xs text-[color:var(--color-ink-muted)]";
    cmd.textContent = t.command_template;
    item.appendChild(cmd);
    item.addEventListener("click", () => {
      menu.style.display = "none";
      void run(t, main);
    });
    menu.appendChild(item);
  }

  if (opts.onEditTools) {
    const sep = document.createElement("div");
    sep.className = "gc-menu__sep";
    menu.appendChild(sep);
    const edit = document.createElement("button");
    edit.className = "gc-menu__item text-[color:var(--color-ink-muted)]";
    edit.textContent = "도구 편집…";
    edit.addEventListener("click", () => {
      menu.style.display = "none";
      opts.onEditTools!();
    });
    menu.appendChild(edit);
  }

  main.addEventListener("click", (ev) => {
    ev.stopPropagation();
    menu.style.display = menu.style.display === "none" ? "" : "none";
  });
  // 바깥을 누르면 닫는다. 버튼이 화면에서 사라지면 리스너도 정리한다.
  const onDocClick = (ev: MouseEvent) => {
    if (!wrap.isConnected) {
      document.removeEventListener("click", onDocClick);
      return;
    }
    if (!wrap.contains(ev.target as Node)) menu.style.display = "none";
  };
  document.addEventListener("click", onDocClick);

  return wrap;
}
