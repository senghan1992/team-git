import type { Repo } from "../lib/ipc";
import { icon } from "./Icon";
import { openAccountModal } from "./AccountModal";
import { openMyPageModal } from "./MyPageModal";
import { getSession } from "../lib/session";

export type RepoTab = "work" | "merge" | "config";
export type Page =
  | { kind: "home" }
  | { kind: "repo"; repoId: string; tab?: RepoTab }
  | { kind: "team" }
  | { kind: "settings" };

export function renderSidebar(
  current: Page,
  repos: Repo[],
  onNav: (p: Page) => void,
): HTMLElement {
  const aside = document.createElement("aside");
  aside.className = "w-60 bg-[color:var(--color-clay)] h-full border-r border-[color:var(--color-hairline)] flex flex-col";
  aside.innerHTML = `
    <div class="px-4 pt-4 pb-3 flex items-center gap-2.5 gc-hairline">
      <span class="inline-flex items-center justify-center w-7 h-7 rounded-[8px] bg-[color:var(--color-primary)] text-white shadow-[inset_0_1px_0_rgba(255,255,255,0.2)]"></span>
      <div class="text-display-base font-bold tracking-[-0.01em]">Git Companion</div>
    </div>
    <nav class="flex flex-col gap-1 px-2 py-3">
      <button data-nav="home" class="gc-nav-btn"><span class="gc-nav-ico"></span><span>저장소</span></button>
      <button data-nav="team" class="gc-nav-btn flex items-center justify-between">
        <span class="inline-flex items-center gap-2"><span class="gc-nav-ico"></span><span>알림</span></span>
        <span id="team-badge" class="text-xs font-semibold rounded-full px-2 py-0.5 bg-[color:var(--color-primary)] text-white" style="display:none"></span>
      </button>
      <button data-nav="settings" class="gc-nav-btn"><span class="gc-nav-ico"></span><span>설정</span></button>
    </nav>
    <div class="gc-hairline px-5 pt-4 pb-2.5 text-[11px] font-semibold text-[color:var(--color-ink-muted)]">등록된 저장소</div>
    <div class="flex-1 overflow-y-auto px-2 pb-4" id="repo-list"></div>
    <button id="account-chip" class="border-t border-[color:var(--color-hairline)] px-5 py-3 flex items-center gap-2.5 w-full text-left hover:bg-[color:var(--color-clay-strong)] transition-colors">
      <span id="account-ico" class="text-[color:var(--color-ink-muted)]"></span>
      <span id="account-label" class="flex-1 min-w-0 text-display-sm truncate">…</span>
    </button>
  `;
  // Brand tile — the commit mark as a cobalt seal.
  const brandTile = aside.querySelector<HTMLElement>("aside span");
  if (brandTile) brandTile.appendChild(icon("commit", 14));
  // Inject icons into nav buttons (folder for repos, users for team, settings for settings).
  const navIconMap: Record<string, SVGSVGElement> = {
    home: icon("folder", 16),
    // 이 화면은 사람 관리가 아니라 "팀원 소식 받기"다 — 종 아이콘이 맞다.
    team: icon("bell", 16),
    settings: icon("settings", 16),
  };
  const navBtns0 = aside.querySelectorAll<HTMLButtonElement>("[data-nav]");
  navBtns0.forEach((b) => {
    const name = b.dataset.nav!;
    const slot = b.querySelector<HTMLElement>(".gc-nav-ico");
    const ic = navIconMap[name];
    if (slot && ic) slot.appendChild(ic);
  });
  const list = aside.querySelector<HTMLDivElement>("#repo-list")!;
  for (const r of repos) {
    const b = document.createElement("button");
    b.className =
      "gc-nav-btn text-left truncate w-full inline-flex items-center gap-2 " +
      (current.kind === "repo" && current.repoId === r.id ? "is-active" : "");
    b.appendChild(icon("folder", 14));
    const label = document.createElement("span");
    label.textContent = r.display_name;
    b.appendChild(label);
    b.addEventListener("click", () => onNav({ kind: "repo", repoId: r.id, tab: "work" }));
    list.appendChild(b);
  }
  const navBtns = aside.querySelectorAll<HTMLButtonElement>("[data-nav]");
  navBtns.forEach((b) => {
    const nav = b.dataset.nav as string;
    if (
      (nav === "home" && current.kind === "home") ||
      (nav === "team" && current.kind === "team") ||
      (nav === "settings" && current.kind === "settings")
    ) {
      b.classList.add("is-active");
    }
    b.addEventListener("click", () => onNav({ kind: nav as "home" | "team" | "settings" }));
  });
  const chip = aside.querySelector<HTMLButtonElement>("#account-chip")!;
  const chipIco = chip.querySelector<HTMLElement>("#account-ico")!;
  chipIco.appendChild(icon("user", 16));
  const chipLabel = chip.querySelector<HTMLElement>("#account-label")!;
  function renderChip() {
    const acc = getSession();
    chipLabel.classList.remove("text-[color:var(--color-ink-muted)]");
    if (acc) {
      chipLabel.textContent = `${acc.name} (${acc.email})`;
      chip.title = "내 정보";
    } else if (acc === null) {
      // 로그아웃 상태로도 앱을 쓸 수 있으므로, 여기서 왜 눌러야 하는지까지
      // 알려 준다. "로그인" 한 단어만 있으면 눌러야 하는지 알 수 없다.
      chipLabel.textContent = "로그인 안 됨 — 로그인하기";
      chipLabel.classList.add("text-[color:var(--color-ink-muted)]");
      chip.title = "로그인하면 팀원 push 알림을 받고 구성원을 검색할 수 있습니다.";
    } else {
      chipLabel.textContent = "…";
      chip.title = "";
    }
  }
  renderChip();
  // 로그인 상태면 마이페이지, 아니면 로그인 — 로그인한 뒤에도 로그인 폼이
  // 뜨는 것이 예전 UX 의 가장 큰 위화감이었다.
  chip.addEventListener("click", () => {
    if (getSession()) openMyPageModal();
    else openAccountModal();
  });
  window.addEventListener("gc-account-changed", renderChip);

  return aside;
}
