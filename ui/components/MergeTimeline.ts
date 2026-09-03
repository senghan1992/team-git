import type { MergeTimeline, TimelineCommit, TimelineMerge, TimelineOpenBranch } from "../lib/ipc";
import { formatDate, formatRelative } from "../lib/format";
import { spinner } from "./Icon";

/**
 * 병합 탭 상단의 "최근 N일 병합 흐름" — 가로축은 시간, 맨 아래 굵은 줄이
 * base 브랜치다. 병합된 브랜치는 색 레인이 base 로 합류하는 곡선으로,
 * 아직 병합되지 않은 브랜치는 오른쪽 끝(지금)까지 이어지는 점선으로 그린다.
 * 레인을 눌러 커밋·파일 목록을 펼친다.
 */

const LANE_COLORS = [
  "var(--lane-1)",
  "var(--lane-2)",
  "var(--lane-3)",
  "var(--lane-4)",
  "var(--lane-5)",
  "var(--lane-6)",
];

/** SVG 의 논리 좌표계 폭 — CSS 로 100% 로 늘린다. */
const W = 960;

export interface TimelineItem {
  kind: "merge" | "open";
  /** 상세 패널 토글 식별자 — 병합은 sha, 열린 브랜치는 이름. */
  key: string;
  label: string;
  lane: number;
  x0: number;
  x1: number;
  merge?: TimelineMerge;
  open?: TimelineOpenBranch;
  /** 레이블을 구간 끝에 오른쪽 정렬로 붙인다 (오른쪽 가장자리에 몰린 항목). */
  anchorEnd: boolean;
  /** 레인 배정에 쓰는 실제 점유 구간 — 선분과 레이블 글자까지 포함한다. */
  occ0: number;
  occ1: number;
}

/** 레이블 글자 폭 추정(px, 11px 글꼴) — 한글·한자는 넓고 라틴/숫자는 좁다. */
export function estimateTextWidth(text: string): number {
  let w = 0;
  for (const ch of text) {
    const code = ch.codePointAt(0) ?? 0;
    w += code > 0x2e7f ? 11 : ch === " " || ch === "·" ? 3.5 : 6.4;
  }
  return w;
}

/**
 * 병합·열린 브랜치를 픽셀 구간으로 바꾸고, 겹치지 않는 구간끼리 레인을
 * 공유하도록 탐욕 배정한다. 순수 함수 — 렌더링 없이 테스트한다.
 *
 * - 창 밖 날짜는 [0, width]로 잘린다 (병합보다 오래된 첫 커밋 등).
 * - 길이 0 구간(병합 커밋만 있는 병합)은 최소 6px 를 보장한다.
 * - 열린 브랜치는 항상 오른쪽 끝(지금)까지 이어진다.
 * - 레인 점유는 선분뿐 아니라 **레이블 글자 폭**까지 포함한다 — 예전에는
 *   선분만 보고 레인을 나눠서, 오늘 시작한 짧은 브랜치 여럿이 오른쪽 끝에
 *   몰리면 글자가 서로 겹쳤다.
 */
export function layoutTimeline(
  data: MergeTimeline,
  opts: { since: string; until: string; width: number },
): { items: TimelineItem[]; laneCount: number } {
  const t0 = Date.parse(opts.since);
  const t1 = Date.parse(opts.until);
  const span = Math.max(t1 - t0, 1);
  const x = (iso: string): number => {
    const t = Date.parse(iso);
    const c = Math.min(Math.max(Number.isFinite(t) ? t : t0, t0), t1);
    return ((c - t0) / span) * opts.width;
  };

  const items: TimelineItem[] = [];
  const place = (partial: Omit<TimelineItem, "anchorEnd" | "occ0" | "occ1" | "lane">) => {
    const labelW = estimateTextWidth(partial.label);
    // 오른쪽 가장자리에 몰린 항목은 레이블을 끝 기준으로 왼쪽으로 뻗친다.
    const anchorEnd = partial.x0 + labelW > opts.width - 4;
    const occ0 = anchorEnd ? Math.max(0, Math.min(partial.x0, partial.x1 - labelW)) : partial.x0;
    const occ1 = anchorEnd ? partial.x1 : Math.max(partial.x1, partial.x0 + labelW);
    items.push({ ...partial, lane: 0, anchorEnd, occ0, occ1 });
  };
  for (const m of data.merges) {
    const x1 = x(m.date);
    let x0 = x(m.first_commit_date ?? m.date);
    if (x1 - x0 < 6) x0 = Math.max(0, x1 - 6);
    place({
      kind: "merge",
      key: m.sha,
      label: m.branch
        ? `${m.branch} · 커밋 ${m.commits.length} · 파일 ${m.files.length}`
        : m.subject,
      x0,
      x1,
      merge: m,
    });
  }
  for (const b of data.open) {
    let x0 = x(b.first_date);
    const x1 = opts.width;
    if (x1 - x0 < 6) x0 = Math.max(0, x1 - 6);
    place({
      kind: "open",
      key: b.name,
      label: `${b.name} · 커밋 ${b.commits.length} · 병합 대기`,
      x0,
      x1,
      open: b,
    });
  }

  // 시작이 이른 것부터 — 빈 레인이 있으면 재사용한다. 16px 여유를 두어
  // 이웃 레이블이 딱 붙지 않게 한다. 점유 구간(occ)은 레이블 글자까지 포함.
  items.sort((a, b) => a.occ0 - b.occ0 || a.occ1 - b.occ1);
  const laneEnds: number[] = [];
  for (const it of items) {
    let lane = laneEnds.findIndex((end) => end + 16 <= it.occ0);
    if (lane < 0) {
      lane = laneEnds.length;
      laneEnds.push(0);
    }
    it.lane = lane;
    laneEnds[lane] = it.occ1;
  }
  return { items, laneCount: laneEnds.length };
}

const SVG_NS = "http://www.w3.org/2000/svg";

function svgEl<K extends keyof SVGElementTagNameMap>(
  tag: K,
  attrs: Record<string, string> = {},
): SVGElementTagNameMap[K] {
  const el = document.createElementNS(SVG_NS, tag);
  for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, v);
  return el;
}

function svgText(x: number, y: number, text: string, cls: string, anchor = "start"): SVGTextElement {
  const t = svgEl("text", { x: String(x), y: String(y), "text-anchor": anchor, class: cls });
  t.textContent = text;
  return t;
}

/** `9/3 (수)` — 축 눈금 라벨. */
function dayLabel(d: Date): string {
  const wd = d.toLocaleDateString("ko-KR", { weekday: "short" });
  return `${d.getMonth() + 1}/${d.getDate()} (${wd})`;
}

export function renderMergeTimeline(opts: {
  load: (days: number) => Promise<MergeTimeline>;
  base: string;
}): { el: HTMLElement; refresh: () => Promise<void> } {
  const el = document.createElement("div");
  el.className = "gc-card gc-tl";

  const head = document.createElement("div");
  head.className = "gc-tl__head";
  const title = document.createElement("div");
  title.className = "gc-tl__title";
  title.textContent = "최근 7일 병합 흐름";
  head.appendChild(title);
  const seg = document.createElement("div");
  seg.className = "gc-tl-seg";
  seg.setAttribute("role", "group");
  seg.setAttribute("aria-label", "기간 선택");
  head.appendChild(seg);
  el.appendChild(head);

  const body = document.createElement("div");
  body.className = "gc-tl__body";
  el.appendChild(body);

  const detail = document.createElement("div");
  detail.className = "gc-tl__detail";
  detail.hidden = true;
  el.appendChild(detail);

  let days = 7;
  let seq = 0;
  let loadedOnce = false;
  /** 상세가 열려 있는 항목 — 자동 새로고침이 접어 버리지 않게 기억한다. */
  let openKey: string | null = null;
  /** 데이터가 그대로면 다시 그리지 않는다 (20초 자동 새로고침 깜빡임 방지). */
  let lastSignature = "";

  const segButtons: HTMLButtonElement[] = [];
  for (const d of [7, 14, 30]) {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "gc-tl-seg__btn" + (d === days ? " gc-tl-seg__btn--on" : "");
    b.textContent = `${d}일`;
    b.addEventListener("click", () => {
      if (days === d) return;
      days = d;
      title.textContent = `최근 ${d}일 병합 흐름`;
      for (const x of segButtons) x.classList.toggle("gc-tl-seg__btn--on", x === b);
      lastSignature = "";
      void refresh();
    });
    segButtons.push(b);
    seg.appendChild(b);
  }

  function showLoading(): void {
    const row = document.createElement("div");
    row.className = "gc-tl__loading";
    row.appendChild(spinner(16));
    const s = document.createElement("span");
    s.textContent = "병합 흐름을 불러오는 중…";
    row.appendChild(s);
    body.replaceChildren(row);
  }

  function commitRow(c: TimelineCommit): HTMLElement {
    const row = document.createElement("div");
    row.className = "gc-tl__commit";
    const subj = document.createElement("span");
    subj.className = "gc-tl__commit-subject";
    subj.textContent = c.subject;
    row.appendChild(subj);
    const meta = document.createElement("span");
    meta.className = "gc-tl__commit-meta";
    meta.textContent = `${c.author} · ${formatRelative(c.date)}`;
    row.appendChild(meta);
    return row;
  }

  function fileChips(files: string[]): HTMLElement {
    const wrap = document.createElement("div");
    wrap.className = "gc-tl__files";
    const max = 12;
    for (const f of files.slice(0, max)) {
      const chip = document.createElement("span");
      chip.className = "gc-tl__file";
      chip.textContent = f;
      wrap.appendChild(chip);
    }
    if (files.length > max) {
      const more = document.createElement("span");
      more.className = "gc-tl__file gc-tl__file--more";
      more.textContent = `+${files.length - max}개`;
      wrap.appendChild(more);
    }
    return wrap;
  }

  function renderDetail(it: TimelineItem, color: string): void {
    detail.replaceChildren();
    detail.hidden = false;

    const head = document.createElement("div");
    head.className = "gc-tl__detail-head";
    const dot = document.createElement("span");
    dot.className = "gc-tl__dot";
    dot.style.background = color;
    head.appendChild(dot);
    const name = document.createElement("span");
    name.className = "gc-tl__detail-name";
    head.appendChild(name);
    const meta = document.createElement("span");
    meta.className = "gc-tl__detail-meta";
    head.appendChild(meta);
    detail.appendChild(head);

    let commits: TimelineCommit[] = [];
    let files: string[] = [];
    if (it.kind === "merge" && it.merge) {
      name.textContent = it.merge.branch ?? it.merge.subject;
      meta.textContent = `${formatDate(it.merge.date)} · ${it.merge.author} 병합`;
      commits = it.merge.commits;
      files = it.merge.files;
      if (commits.length === 0) {
        const note = document.createElement("div");
        note.className = "gc-tl__detail-meta";
        note.textContent = "이 병합으로 들어온 새 커밋이 없습니다 (이미 base에 있던 커밋).";
        detail.appendChild(note);
      }
    } else if (it.kind === "open" && it.open) {
      name.textContent = it.open.name;
      const badge = document.createElement("span");
      badge.className = "gc-badge gc-badge--warning";
      badge.textContent = "병합 대기";
      head.appendChild(badge);
      meta.textContent = `최근 활동 ${formatRelative(it.open.last_date)}`;
      commits = it.open.commits;
      files = it.open.files;
    }

    const list = document.createElement("div");
    list.className = "gc-tl__commits";
    const max = 8;
    for (const c of commits.slice(0, max)) list.appendChild(commitRow(c));
    if (commits.length > max) {
      const more = document.createElement("div");
      more.className = "gc-tl__commit-meta";
      more.textContent = `…외 커밋 ${commits.length - max}개`;
      list.appendChild(more);
    }
    detail.appendChild(list);
    if (files.length > 0) detail.appendChild(fileChips(files));
  }

  function renderChart(data: MergeTimeline): void {
    const { items, laneCount } = layoutTimeline(data, {
      since: data.since,
      until: data.until,
      width: W,
    });

    if (items.length === 0 && data.direct.length === 0) {
      const empty = document.createElement("div");
      empty.className = "gc-tl__empty";
      empty.textContent = `지난 ${days}일 동안 ${data.base}에 병합된 브랜치가 없습니다.`;
      body.replaceChildren(empty);
      detail.hidden = true;
      openKey = null;
      return;
    }

    const padTop = 6;
    const laneH = 26;
    const baseY = padTop + Math.max(laneCount, 0) * laneH + 14;
    const H = baseY + 26;
    const laneY = (l: number) => baseY - 24 - l * laneH;

    const svg = svgEl("svg", {
      viewBox: `0 0 ${W} ${H}`,
      class: "gc-tl__svg",
      role: "img",
    });
    svg.setAttribute("aria-label", `${data.base} 병합 타임라인`);

    // ── 날짜 눈금 ──────────────────────────────────────────────────────────
    const t0 = Date.parse(data.since);
    const t1 = Date.parse(data.until);
    const span = Math.max(t1 - t0, 1);
    const ticks: Date[] = [];
    const first = new Date(t0);
    first.setHours(24, 0, 0, 0); // 창 시작 이후의 첫 자정
    for (let t = first.getTime(); t <= t1; t += 86400_000) ticks.push(new Date(t));
    const labelEvery = Math.max(1, Math.ceil(ticks.length / 10));
    ticks.forEach((d, i) => {
      const tx = ((d.getTime() - t0) / span) * W;
      svg.appendChild(
        svgEl("line", {
          x1: String(tx),
          y1: String(padTop),
          x2: String(tx),
          y2: String(baseY),
          class: "gc-tl__grid",
        }),
      );
      if (i % labelEvery === 0) {
        svg.appendChild(svgText(tx, baseY + 16, dayLabel(d), "gc-tl__tick", "middle"));
      }
    });

    // ── base 줄 ────────────────────────────────────────────────────────────
    svg.appendChild(
      svgEl("line", { x1: "0", y1: String(baseY), x2: String(W), y2: String(baseY), class: "gc-tl__base" }),
    );
    const chipW = data.base.length * 7 + 14;
    svg.appendChild(
      svgEl("rect", {
        x: "4",
        y: String(baseY - 9),
        width: String(chipW),
        height: "18",
        rx: "5",
        class: "gc-tl__basechip",
      }),
    );
    svg.appendChild(svgText(4 + chipW / 2, baseY + 4, data.base, "gc-tl__basechip-text", "middle"));

    // ── base 직접 커밋 ─────────────────────────────────────────────────────
    for (const c of data.direct) {
      const t = Date.parse(c.date);
      if (!Number.isFinite(t) || t < t0) continue;
      const cx = ((Math.min(t, t1) - t0) / span) * W;
      const dot = svgEl("circle", { cx: String(cx), cy: String(baseY), r: "3", class: "gc-tl__directdot" });
      const tt = svgEl("title");
      tt.textContent = `${c.subject} — ${c.author} · ${formatRelative(c.date)}`;
      dot.appendChild(tt);
      svg.appendChild(dot);
    }

    // ── 브랜치 레인 ────────────────────────────────────────────────────────
    items.forEach((it, idx) => {
      const color = LANE_COLORS[idx % LANE_COLORS.length];
      const y = laneY(it.lane);
      const g = svgEl("g", { class: "gc-tl__item", tabindex: "0", role: "button" });

      if (it.kind === "merge") {
        const bendX = Math.max(it.x0, it.x1 - 14);
        const path = svgEl("path", {
          d: `M ${it.x0} ${y} L ${bendX} ${y} Q ${it.x1} ${y} ${it.x1} ${baseY - 4}`,
          class: "gc-tl__lane",
        });
        path.style.stroke = color;
        g.appendChild(path);
        const dot = svgEl("circle", { cx: String(it.x1), cy: String(baseY), r: "4.5", class: "gc-tl__mergedot" });
        dot.style.fill = color;
        g.appendChild(dot);
      } else {
        const endX = W - 8;
        const path = svgEl("path", {
          d: `M ${it.x0} ${y} L ${endX} ${y}`,
          class: "gc-tl__lane gc-tl__lane--open",
        });
        path.style.stroke = color;
        g.appendChild(path);
        const dot = svgEl("circle", { cx: String(endX), cy: String(y), r: "4", class: "gc-tl__opendot" });
        dot.style.stroke = color;
        g.appendChild(dot);
      }

      // 레이블 — 오른쪽 끝에 몰리면 넘치지 않게 끝 기준으로 붙인다
      // (레인 배정도 같은 판정을 썼으므로 이웃과 겹치지 않는다).
      g.appendChild(
        svgText(
          it.anchorEnd ? Math.min(it.x1, W - 4) : it.x0,
          y - 6,
          it.label,
          "gc-tl__label" + (it.kind === "open" ? " gc-tl__label--open" : ""),
          it.anchorEnd ? "end" : "start",
        ),
      );

      const tt = svgEl("title");
      tt.textContent =
        it.kind === "merge" && it.merge
          ? `${it.merge.subject}\n${it.merge.author} · ${formatDate(it.merge.date)}`
          : `${it.label} — 아직 ${data.base}에 병합되지 않았습니다`;
      g.appendChild(tt);

      const toggle = () => {
        if (openKey === it.key) {
          openKey = null;
          detail.hidden = true;
        } else {
          openKey = it.key;
          renderDetail(it, color);
        }
      };
      g.addEventListener("click", toggle);
      g.addEventListener("keydown", (e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          toggle();
        }
      });
      svg.appendChild(g);
    });

    body.replaceChildren(svg);

    // 열려 있던 상세를 데이터 새로고침 후에도 유지한다.
    if (openKey !== null) {
      const idx = items.findIndex((x) => x.key === openKey);
      if (idx >= 0) renderDetail(items[idx], LANE_COLORS[idx % LANE_COLORS.length]);
      else {
        openKey = null;
        detail.hidden = true;
      }
    }
  }

  async function refresh(): Promise<void> {
    const my = ++seq;
    if (!loadedOnce) showLoading();
    let data: MergeTimeline;
    try {
      data = await opts.load(days);
    } catch (e) {
      if (my !== seq) return;
      if (!loadedOnce) {
        const err = document.createElement("div");
        err.className = "gc-tl__empty";
        err.textContent = `병합 흐름을 불러오지 못했습니다: ${(e as Error).message ?? e}`;
        body.replaceChildren(err);
      }
      // 이미 그려진 차트가 있으면 그대로 둔다 — 다음 새로고침이 다시 시도한다.
      return;
    }
    if (my !== seq) return;
    loadedOnce = true;
    // since/until 은 호출 시각이라 매번 다르다 — 내용이 같으면 그리지 않는다.
    const sig = JSON.stringify([data.base, data.merges, data.direct, data.open, days]);
    if (sig === lastSignature) return;
    lastSignature = sig;
    renderChart(data);
  }

  return { el, refresh };
}
