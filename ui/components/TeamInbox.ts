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

  /** 카드의 액션이 성공했을 때만 읽음 처리한다 — 실패한 할 일은 배지에 남아야 한다. */
  async function markRead(r: TeamEventRow) {
    if (r.read) return;
    try {
      await ipc_peer.markTeamRead(r.id);
      r.read = true;
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
      d.textContent = "팀원 모두가 푸시하면 알림이 도착합니다.";
      empty.appendChild(d);
      list.appendChild(empty);
      return;
    }
    for (const r of rows) {
      list.appendChild(card(r));
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

  function card(r: TeamEventRow): HTMLElement {
    const el = document.createElement("div");
    el.className = "gc-card cursor-pointer " + (r.read ? "opacity-60" : "font-medium");
    el.innerHTML = `
      <div class="flex items-center gap-3">
        <span class="gc-badge gc-badge--neutral">${eventKindLabel(r.event_kind)}</span>
        <span class="text-display-sm text-[color:var(--color-ink-muted)]">${escape(r.sender_device_name)}</span>
        <span class="text-display-sm text-[color:var(--color-ink-muted)] ml-auto">${formatRelative(r.received_at)}</span>
      </div>
      <div class="text-display-md mt-1">${escape(r.repo_name)}</div>
      <pre class="hidden mt-2 text-display-sm bg-[color:var(--color-surface)] p-3 rounded-md overflow-x-auto">${escape(r.payload)}</pre>
      <div class="flex gap-2 mt-2">
        <button class="gc-button-secondary" data-view-repo>리포 보기</button>
        <button class="gc-button-secondary inline-flex items-center gap-1" data-kind-action></button>
      </div>
    `;
    const pre = el.querySelector("pre") as HTMLPreElement;
    el.addEventListener("click", (e) => {
      if ((e.target as HTMLElement).tagName === "BUTTON") return;
      pre.classList.toggle("hidden");
      // 내용을 펼쳐 봤다면 읽은 것이다.
      void markRead(r).then(() => el.classList.add("opacity-60"));
    });
    const viewBtn = el.querySelector<HTMLButtonElement>("[data-view-repo]");
    viewBtn?.addEventListener("click", async (ev) => {
      ev.stopPropagation();
      const repo = await resolveRepo(r);
      if (repo) {
        await markRead(r);
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
    const action = kindAction(r);
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
  // 접미사 매칭으로 판별한다.
  function kindAction(r: TeamEventRow): { label: string; icon: IconName; run: () => Promise<void> } | null {
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
          await markRead(r);
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
            const res = await ipc.syncBranch(repo.id, repo.default_branch);
            if (res.conflicted) {
              await markRead(r);
              toast(`충돌 ${res.files.length}개 발생 — 병합 센터에서 해결하세요.`, "info");
              onNav({ kind: "repo", repoId: repo.id, tab: "merge" });
            } else {
              await markRead(r);
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

function eventKindLabel(kind: string): string {
  const k = kind.replace(/^team_/, "");
  if (k === "branch_push") return "브랜치 푸시";
  if (k === "main_push") return "메인 병합";
  if (k === "release") return "릴리스";
  return kind;
}

function escape(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}
