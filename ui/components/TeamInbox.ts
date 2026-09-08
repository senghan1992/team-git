import { ipc, ipc_peer, type TeamEventRow } from "../lib/ipc";
import { repoForEvent } from "../lib/repoMatch";
import { formatRelative } from "../lib/format";
import type { Page } from "../components/Sidebar";
import { icon, type IconName } from "./Icon";
import { toast } from "./Toast";
import { setBusy } from "./Busy";

export async function renderInboxList(onNav: (p: Page) => void): Promise<HTMLElement> {
  const wrap = document.createElement("div");
  wrap.className = "flex flex-col gap-3";

  let rows: TeamEventRow[] = [];

  /**
   * 같은 저장소·같은 브랜치에서 온 branch_push 는 한 카드로 합친다. 팀원이
   * 브랜치에 여러 번 push 하면 최신 push 에 이전 push 가 전부 포함되므로
   * 알림도 하나(최신)로 보여야 한다 — 여러 개로 늘어서면 "같은 브랜치가
   * 여러 개"로 읽힌다. 이전 push 알림은 저장 시점에 이미 읽음으로
   * 대체된다(store insert collapse). branch_push 가 아니거나 브랜치 정보가
   * 없으면 이벤트 하나가 카드 하나다 (rows 는 최신순이므로 group[0] 이 최신).
   */
  function groupRows(rows: TeamEventRow[]): TeamEventRow[][] {
    const groups = new Map<string, TeamEventRow[]>();
    const out: TeamEventRow[][] = [];
    for (const r of rows) {
      const key = branchKeyOf(r) ?? `event:${r.id}`;
      let g = groups.get(key);
      if (!g) {
        g = [];
        groups.set(key, g);
        out.push(g);
      }
      g.push(r);
    }
    return out;
  }

  function branchKeyOf(r: TeamEventRow): string | null {
    if (!r.event_kind.endsWith("branch_push")) return null;
    const branch = branchOf(r);
    if (!branch) return null;
    return `${r.project_id}\u0000${r.repo_name}\u0000${branch}`;
  }

  /** 카드의 액션이 성공했을 때만 읽음 처리한다 — 실패한 할 일은 배지에 남아야 한다. */
  async function markReadGroup(group: TeamEventRow[]) {
    const unread = group.filter((r) => !r.read);
    if (unread.length === 0) return;
    try {
      for (const r of unread) {
        await ipc_peer.markTeamRead(r.id);
        r.read = true;
      }
      renderMeta();
      // 사이드바 배지가 즉시 따라오도록 알린다.
      window.dispatchEvent(new CustomEvent("gc-team-read-changed"));
    } catch {
      // 읽음 표시는 부가 기능 — 실패해도 흐름을 막지 않는다.
    }
  }

  function renderMeta() {
    const unread = rows.filter((r) => !r.read).length;
    meta.textContent = `총 ${rows.length}건 · 읽지 않음 ${unread}건`;
    markAllBtn.style.display = unread > 0 ? "" : "none";
  }

  async function refresh() {
    try {
      rows = await ipc_peer.listTeamEvents(100, false);
    } catch {
      rows = [];
    }
    renderMeta();
    list.innerHTML = "";
    if (rows.length === 0) {
      const empty = document.createElement("div");
      empty.className = "gc-empty";
      const iconWrap = document.createElement("span");
      iconWrap.className = "gc-empty__icon";
      iconWrap.appendChild(icon("inbox", 32));
      empty.appendChild(iconWrap);
      const t = document.createElement("div");
      t.className = "gc-empty__title";
      t.textContent = "아직 팀 알림이 없습니다";
      empty.appendChild(t);
      const d = document.createElement("div");
      d.className = "gc-empty__desc";
      d.textContent =
        "병합 관리자로 맡은 브랜치에 푸시가 오거나, 병합 관리자가 병합을 반영하면 알림이 도착합니다.";
      empty.appendChild(d);
      list.appendChild(empty);
      return;
    }
    for (const group of groupRows(rows)) {
      list.appendChild(card(group));
    }
  }

  const meta = document.createElement("div");
  meta.className = "text-display-sm text-[color:var(--color-ink-muted)]";

  // 자리를 비웠다 돌아온 사람은 수신함을 훑고 한 번에 정리한다 —
  // 카드 하나하나 눌러야만 배지가 줄어드는 구조는 배지를 영영 못 지운다.
  const markAllBtn = document.createElement("button");
  markAllBtn.className = "gc-button-secondary text-display-sm";
  markAllBtn.textContent = "모두 읽음";
  markAllBtn.addEventListener("click", async () => {
    setBusy(markAllBtn, true, "처리 중…");
    try {
      await ipc_peer.markAllTeamRead();
      window.dispatchEvent(new CustomEvent("gc-team-read-changed"));
      await refresh();
    } catch (e) {
      toast(`읽음 처리 실패: ${(e as Error).message ?? e}`, "error");
    } finally {
      setBusy(markAllBtn, false);
    }
  });

  const header = document.createElement("div");
  header.className = "flex items-center justify-end gap-3";
  header.appendChild(meta);
  header.appendChild(markAllBtn);
  wrap.appendChild(header);

  const list = document.createElement("div");
  list.className = "flex flex-col gap-3";
  wrap.appendChild(list);

  /** 이벤트가 가리키는 내 저장소 — remote URL 우선, 이름은 유일할 때만. */
  async function resolveRepo(r: TeamEventRow) {
    const repos = await ipc.listRepositories();
    return repoForEvent(repos, r);
  }

  function card(group: TeamEventRow[]): HTMLElement {
    // 같은 브랜치 push 가 모였으면 최신(맨 앞) 이 카드를 대표한다.
    const r = group[0];
    const el = document.createElement("div");
    el.className = "gc-card cursor-pointer";
    el.innerHTML = `
      <div class="flex items-center gap-3">
        <span class="gc-badge gc-badge--info" data-new-chip>새 알림</span>
        <span class="gc-badge gc-badge--neutral">${eventKindLabel(r.event_kind)}</span>
        ${group.length > 1 ? `<span class="gc-badge gc-badge--muted" data-dup-chip title="같은 브랜치의 이전 push 는 최신 push 에 포함되어 읽음 처리됩니다.">push ${group.length}회 — 최신 push 가 병합 대상</span>` : ""}
        <span class="text-display-sm text-[color:var(--color-ink-muted)]">${escape(r.sender_device_name)}</span>
        <span class="text-display-sm text-[color:var(--color-ink-muted)] ml-auto">${formatRelative(r.received_at)}</span>
      </div>
      <div class="text-display-md mt-1" data-title>${escape(r.repo_name)}${summaryOf(r)}</div>
      <pre class="hidden mt-2 text-display-sm bg-[color:var(--color-surface)] p-3 rounded-md overflow-x-auto">${escape(r.payload)}</pre>
      <div class="flex flex-wrap gap-2 mt-2">
        <button class="gc-button-secondary" data-view-repo>리포 보기</button>
        <button class="gc-button-secondary inline-flex items-center gap-1" data-kind-action></button>
        <button class="gc-button-secondary inline-flex items-center gap-1 ml-auto" data-mark-read title="처리하지 않고 읽음으로만 표시합니다"></button>
      </div>
    `;
    // 읽음/안 읽음이 한눈에 갈리게 — 카드 톤과 "새 알림" 칩, "읽음 표시" 버튼을
    // 상태에 맞춰 함께 바꾼다. 예전에는 흐린 톤 하나로만 구분해 읽음 처리가
    // 됐는지 알아보기 어려웠다.
    const applyReadStyle = () => {
      el.classList.toggle("opacity-60", r.read);
      el.classList.toggle("font-medium", !r.read);
      const chip = el.querySelector<HTMLElement>("[data-new-chip]");
      if (chip) chip.style.display = r.read ? "none" : "";
      const mark = el.querySelector<HTMLElement>("[data-mark-read]");
      if (mark) mark.style.display = r.read ? "none" : "";
    };
    applyReadStyle();
    const markBtn = el.querySelector<HTMLButtonElement>("[data-mark-read]");
    if (markBtn) {
      markBtn.appendChild(icon("check", 14));
      const lbl = document.createElement("span");
      lbl.textContent = "읽음 표시";
      markBtn.appendChild(lbl);
      markBtn.addEventListener("click", async (ev) => {
        ev.stopPropagation();
        await markReadGroup(group);
        applyReadStyle();
      });
    }
    const pre = el.querySelector("pre") as HTMLPreElement;
    el.addEventListener("click", (e) => {
      if ((e.target as HTMLElement).closest("button")) return;
      pre.classList.toggle("hidden");
      // 내용을 펼쳐 봤다면 읽은 것이다.
      void markReadGroup(group).then(applyReadStyle);
    });
    const viewBtn = el.querySelector<HTMLButtonElement>("[data-view-repo]");
    viewBtn?.addEventListener("click", async (ev) => {
      ev.stopPropagation();
      const repo = await resolveRepo(r);
      if (repo) {
        await markReadGroup(group);
        onNav({ kind: "repo", repoId: repo.id });
      } else {
        toast(
          `'${r.repo_name}' 저장소를 찾을 수 없습니다. 이 컴퓨터에 등록되지 않았거나 원격 주소가 다릅니다.`,
          "error",
        );
      }
    });

    // 종류별 다음 단계 버튼 — 알림이 곧바로 "다음 해야 할 일"로 이어진다.
    const kindBtn = el.querySelector<HTMLButtonElement>("[data-kind-action]");
    const action = kindAction(group);
    if (kindBtn && action) {
      kindBtn.prepend(icon(action.icon, 14));
      const label = document.createElement("span");
      label.textContent = action.label;
      kindBtn.appendChild(label);
      kindBtn.addEventListener("click", async (ev) => {
        ev.stopPropagation();
        // SSH 저장소의 동기화는 수 초가 걸린다 — 진행 표시가 없으면 두 번
        // 눌러 병합이 겹친다.
        setBusy(kindBtn, true, "진행 중…");
        try {
          await action.run();
        } finally {
          setBusy(kindBtn, false);
        }
      });
    } else if (kindBtn) {
      kindBtn.style.display = "none";
    }
    return el;
  }

  // 알림 종류에 맞는 다음 단계 액션. release 등은 리포 보기만 제공한다.
  // event_kind는 과거 버전에서 "team_" 접두사가 붙은 값도 저장됐으므로
  // 접미사 매칭으로 판별한다. 같은 브랜치가 합쳐진 group 이면 최신이 대표.
  function kindAction(group: TeamEventRow[]): { label: string; icon: IconName; run: () => Promise<void> } | null {
    const r = group[0];
    if (r.event_kind.endsWith("merge_request")) {
      return {
        label: "병합 요청 검토",
        icon: "merge",
        run: async () => {
          const repo = await resolveRepo(r);
          if (!repo) {
            toast(
              `'${r.repo_name}' 저장소를 찾을 수 없습니다. 이 컴퓨터에 등록되지 않았거나 원격 주소가 다릅니다.`,
              "error",
            );
            return;
          }
          await markReadGroup(group);
          onNav({ kind: "repo", repoId: repo.id, tab: "merge" });
        },
      };
    }
    if (r.event_kind.endsWith("branch_push")) {
      return {
        label: "병합 센터로",
        icon: "merge",
        run: async () => {
          const repo = await resolveRepo(r);
          if (!repo) {
            toast(
              `'${r.repo_name}' 저장소를 찾을 수 없습니다. 이 컴퓨터에 등록되지 않았거나 원격 주소가 다릅니다.`,
              "error",
            );
            return;
          }
          await markReadGroup(group);
          onNav({ kind: "repo", repoId: repo.id, tab: "merge" });
        },
      };
    }
    if (r.event_kind.endsWith("main_push")) {
      return {
        label: "내 브랜치에 병합",
        icon: "arrow-right",
        run: async () => {
          const repo = await resolveRepo(r);
          if (!repo) {
            toast(
              `'${r.repo_name}' 저장소를 찾을 수 없습니다. 이 컴퓨터에 등록되지 않았거나 원격 주소가 다릅니다.`,
              "error",
            );
            return;
          }
          try {
            // 병합이 반영된 브랜치가 payload에 있다 — release/1.0 같은
            // 두 번째 병합 대상도 그 브랜치로 동기화한다.
            const base = branchOf(r) ?? (repo.default_branch || "main");
            const res = await ipc.syncBranch(repo.id, base);
            if (res.conflicted) {
              await markReadGroup(group);
              toast(`충돌 ${res.files.length}개 발생 — 병합 센터에서 해결하세요.`, "info");
              onNav({ kind: "repo", repoId: repo.id, tab: "merge" });
            } else {
              await markReadGroup(group);
              toast("동기화 완료 — 최신 변경을 병합했습니다.", "success");
              onNav({ kind: "repo", repoId: repo.id });
            }
          } catch (e) {
            const msg = (e as Error).message ?? String(e);
            if (msg.includes("병합이 있습니다")) {
              toast("이미 진행 중인 병합이 있어 병합 센터로 이동합니다.", "info");
              onNav({ kind: "repo", repoId: repo.id, tab: "merge" });
            } else {
              // 실패한 동기화는 읽음 처리하지 않는다 — 아직 해야 할 일이다.
              toast(`동기화 실패: ${msg}`, "error");
            }
          }
        },
      };
    }
    return null;
  }

  await refresh();
  return wrap;
}

/** 제목 옆에 붙는 한 줄 요약 — 브랜치와 커밋 메시지. payload 를 펼치지 않아도 무슨 일인지 보인다. */
function summaryOf(r: TeamEventRow): string {
  try {
    const p = JSON.parse(r.payload) as { data?: { branch?: string; message?: string } };
    const branch = p.data?.branch?.trim();
    const message = p.data?.message?.trim();
    const parts = [branch ? `<code>${escape(branch)}</code>` : "", message ? escape(message) : ""].filter(Boolean);
    return parts.length
      ? ` <span class="text-display-sm text-[color:var(--color-ink-muted)]">· ${parts.join(" — ")}</span>`
      : "";
  } catch {
    return "";
  }
}

function eventKindLabel(kind: string): string {
  const k = kind.replace(/^team_/, "");
  // 라벨은 알림이 *무엇을 하라는 것인지* 말한다 — merge_request 는 병합 관리자의
  // 승인 대기열에 오른 요청이고, branch_push 는 참고용 push 알림이며,
  // main_push 는 구성원에게 동기화를 안내하는 알림이다.
  if (k === "merge_request") return "병합 요청";
  if (k === "branch_push") return "push";
  if (k === "main_push") return "동기화 안내";
  if (k === "release") return "릴리스";
  return kind;
}

/** payload에서 병합이 반영된 브랜치 이름 (없으면 null). */
function branchOf(r: TeamEventRow): string | null {
  try {
    const p = JSON.parse(r.payload) as { data?: { branch?: string } };
    const b = p.data?.branch?.trim();
    return b || null;
  } catch {
    return null;
  }
}

function escape(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}
