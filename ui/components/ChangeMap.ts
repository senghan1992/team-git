// 변경 지도 — "지금 어디가, 누구 손에 고쳐지고 있는가"를 한 화면에 보여 준다.
//
// 시나리오 4: AI 에이전트를 쓰기 시작하면 각자 담당 영역만 고치는 게 아니라
// 엮인 영역까지 함께 수정되기 때문에, 브랜치별 목록만으로는 위험이 안 보인다.
// 그래서 브랜치 → 파일이 아니라 **파일 → 브랜치**로 뒤집어서, 두 곳 이상이
// 손대고 있는 파일을 맨 위에 세운다. 병합 관리자는 이걸 보고 병합 순서를
// 정하고, 팀원은 자기 파일이 남의 브랜치에도 올라와 있는지 확인한다.

import type { PendingBranch } from "../lib/ipc";
import { icon } from "./Icon";

export interface FileTouch {
  path: string;
  /** 이 파일을 건드리는 브랜치들 — 겹침 판정의 근거. */
  touches: { branch: string; author: string; kind: string }[];
}

/** 파일 기준으로 뒤집는다. 겹치는 파일 먼저, 그 다음 경로 순. */
export function buildChangeMap(branches: PendingBranch[]): FileTouch[] {
  const byPath = new Map<string, FileTouch>();
  for (const b of branches) {
    for (const cf of b.changed_files) {
      let entry = byPath.get(cf.path);
      if (!entry) {
        entry = { path: cf.path, touches: [] };
        byPath.set(cf.path, entry);
      }
      entry.touches.push({ branch: b.short_name, author: b.author, kind: cf.kind });
    }
  }
  return [...byPath.values()].sort((a, b) => {
    if (a.touches.length !== b.touches.length) return b.touches.length - a.touches.length;
    return a.path.localeCompare(b.path);
  });
}

/** 변경 종류(A/M/D/R…)를 사람 말로. 배지 색과 함께 쓴다. */
function kindLabel(kind: string): string {
  if (kind === "A") return "추가";
  if (kind === "D") return "삭제";
  if (kind === "M") return "수정";
  if (kind.startsWith("R")) return "이름변경";
  if (kind.startsWith("C")) return "복사";
  return kind || "변경";
}

function kindColor(kind: string): string {
  if (kind === "A") return "#276b4e";
  if (kind === "D") return "#ad392c";
  if (kind === "M") return "#2c4b8f";
  if (kind.startsWith("R") || kind.startsWith("C")) return "#8a5a10";
  return "var(--color-ink-muted)";
}

/**
 * 변경 지도 카드. `branches`가 비어 있으면 `null`을 돌려주므로 호출부는
 * 빈 카드를 붙이지 않아도 된다.
 */
export function renderChangeMap(branches: PendingBranch[]): HTMLElement | null {
  const rows = buildChangeMap(branches);
  if (rows.length === 0) return null;

  const shared = rows.filter((r) => r.touches.length > 1);
  // 기본은 위험한 것만 — 파일이 수백 개여도 화면이 무너지지 않는다.
  let showAll = shared.length === 0;

  const card = document.createElement("section");
  card.className = "gc-card flex flex-col gap-3";

  // ── 헤더 ────────────────────────────────────────────────────────────────
  const head = document.createElement("div");
  head.className = "flex items-start gap-2";
  const headIcon = document.createElement("span");
  headIcon.className = "text-[color:var(--color-ink-muted)] mt-0.5";
  headIcon.appendChild(icon("folder", 16));
  head.appendChild(headIcon);
  const headText = document.createElement("div");
  headText.className = "flex-1 min-w-0";
  const headTitle = document.createElement("div");
  headTitle.className = "text-display-lg font-medium";
  headTitle.textContent = "변경 지도";
  headText.appendChild(headTitle);
  const headSub = document.createElement("div");
  headSub.className = "text-display-sm text-[color:var(--color-ink-muted)]";
  headSub.textContent = `대기 중인 브랜치 ${branches.length}개가 파일 ${rows.length}개를 수정하고 있습니다.`;
  headText.appendChild(headSub);
  head.appendChild(headText);
  card.appendChild(head);

  // ── 겹침 경고 ───────────────────────────────────────────────────────────
  if (shared.length > 0) {
    const warn = document.createElement("div");
    warn.className = "gc-banner gc-banner--danger";
    const iw = document.createElement("span");
    iw.className = "gc-banner__icon";
    iw.appendChild(icon("warn", 20));
    warn.appendChild(iw);
    const body = document.createElement("div");
    body.className = "gc-banner__body flex-1 flex flex-col gap-0.5";
    const t = document.createElement("div");
    t.className = "gc-banner__title";
    t.textContent = `같은 파일을 두 곳 이상에서 고치고 있습니다 — ${shared.length}개`;
    body.appendChild(t);
    const d = document.createElement("div");
    d.textContent =
      "먼저 병합한 쪽이 기준이 됩니다. 나중에 병합하는 브랜치에서 충돌이 날 가능성이 높으니, 아래 파일들을 확인하고 순서를 정하세요.";
    body.appendChild(d);
    warn.appendChild(body);
    card.appendChild(warn);
  }

  // ── 파일 목록 ───────────────────────────────────────────────────────────
  const list = document.createElement("div");
  list.className = "gc-list";
  card.appendChild(list);

  const moreBtn = document.createElement("button");
  moreBtn.className = "gc-button-secondary text-display-sm self-start";
  card.appendChild(moreBtn);

  function paint() {
    const visible = showAll ? rows : shared;
    list.innerHTML = "";
    for (const r of visible) {
      const row = document.createElement("div");
      row.className = "gc-list__row items-start gap-3";

      // 파일 경로 — 겹치면 경고 아이콘을 앞에 세운다.
      const left = document.createElement("div");
      left.className = "flex items-start gap-2 min-w-0 flex-1";
      if (r.touches.length > 1) {
        const w = document.createElement("span");
        w.className = "shrink-0 mt-0.5";
        w.style.color = "#ad392c";
        w.appendChild(icon("warn", 14));
        left.appendChild(w);
      }
      const pathEl = document.createElement("span");
      pathEl.className = "font-mono text-display-sm truncate";
      pathEl.title = r.path;
      pathEl.textContent = r.path;
      left.appendChild(pathEl);
      row.appendChild(left);

      // 누가 / 어느 브랜치에서 어떻게 고치는지.
      const who = document.createElement("div");
      who.className = "flex flex-wrap items-center gap-1.5 justify-end shrink-0 max-w-[55%]";
      for (const t of r.touches) {
        const chip = document.createElement("span");
        chip.className = "gc-badge gc-badge--muted inline-flex items-center gap-1";
        const k = document.createElement("span");
        k.style.color = kindColor(t.kind);
        k.textContent = kindLabel(t.kind);
        chip.appendChild(k);
        const sep = document.createElement("span");
        sep.className = "text-[color:var(--color-ink-muted)]";
        sep.textContent = "·";
        chip.appendChild(sep);
        const name = document.createElement("span");
        name.className = "font-mono";
        name.textContent = t.branch;
        chip.appendChild(name);
        const author = document.createElement("span");
        author.className = "text-[color:var(--color-ink-muted)]";
        author.textContent = `(${t.author})`;
        chip.appendChild(author);
        who.appendChild(chip);
      }
      row.appendChild(who);
      list.appendChild(row);
    }

    if (rows.length === shared.length) {
      moreBtn.style.display = "none";
      return;
    }
    moreBtn.style.display = "";
    moreBtn.textContent = showAll
      ? `겹치는 파일만 보기 (${shared.length}개)`
      : `전체 ${rows.length}개 파일 보기`;
  }

  moreBtn.addEventListener("click", () => {
    showAll = !showAll;
    paint();
  });
  paint();

  return card;
}
