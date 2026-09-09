// 마이페이지 — 로그인한 상태에서 사이드바의 내 이름을 누르면 열린다.
//
// 두 개의 탭으로 짜여 있다:
//   • 이용 가이드 (기본 탭) — 팀의 한 바퀴(등록 → 작업 → 병합 요청 → 승인 →
//     동기화)를 역할별로 보여 준다. README를 안 읽은 사람도 이 모달 하나로
//     "지금 무엇을 어디서 눌러야 하는지"를 알 수 있게.
//   • 내 정보 — 프로필 헤더(아바타·이름·아이디·이메일) → 내 정보 수정 →
//     비밀번호 변경 → 로그아웃 → (맨 아래, 위험 구역) 회원 탈퇴
//
// 예래에는 로그인 폼과 "내 계정" 카드와 "계정 전환/삭제" 목록이 한 모달에
// 섞여 있었다. 로그인한 뒤에도 아이디/비밀번호 입력칸이 그대로 보였고, 이
// 컴퓨터에서 한 번이라도 로그인한 사람들이 삭제 버튼과 함께 나열됐다.
// 어느 앱에서도 마이페이지가 그렇게 생기지 않았으니 낯설 수밖에 없다.
//
// 계정 전환은 "로그아웃 후 다시 로그인"이다. 계정 목록은 서버가 소유하므로
// 이 컴퓨터가 기억할 이유가 없다.
import { ipc, type Account, type ProjectConfigResult, type Repo } from "../lib/ipc";
import { openModal, confirmDialog } from "./Modal";
import { toast } from "./Toast";
import { setBusy } from "./Busy";
import { icon } from "./Icon";
import { refreshSession, setSession, getSession } from "../lib/session";
import { openAccountModal } from "./AccountModal";
import { mergeManagerEmails } from "./nextAction";

export type MyPageTab = "guide" | "account";

/** yyyy년 M월 d일 — 가입일처럼 한 번 읽고 마는 값에는 이 형태가 읽기 쉽다. */
function formatJoined(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "-";
  return `${d.getFullYear()}년 ${d.getMonth() + 1}월 ${d.getDate()}일`;
}

export function openMyPageModal(initialTab: MyPageTab = "guide"): void {
  void (async () => {
    let me = await ipc.accountCurrent().catch(() => null);
    if (!me) {
      // 세션이 없으면 마이페이지가 아니라 로그인 화면이 맞다.
      openAccountModal();
      return;
    }

    const m = openModal({ title: "이용 가이드", hideFooter: true });
    m.el.addEventListener("close", () => {
      void refreshSession();
    });

    // 가이드가 붙어 모달이 길어졌다 — 화면을 넘치지 않게 본문만 굴린다.
    m.body.style.maxHeight = "min(72vh, 680px)";
    m.body.style.overflowY = "auto";

    const titleEl = m.el.querySelector<HTMLElement>(".gc-modal__title")!;
    const descEl = m.el.querySelector<HTMLElement>(".gc-modal__description");

    // ── 탭 바 — 저장소 화면과 같은 segmented control ────────────────────────
    let tab: MyPageTab = initialTab;
    const tabs = document.createElement("div");
    tabs.className = "gc-tabs mb-1";
    const guideBtn = document.createElement("button");
    guideBtn.className = "gc-tab";
    guideBtn.appendChild(icon("info", 14));
    const guideLabel = document.createElement("span");
    guideLabel.textContent = "이용 가이드";
    guideBtn.appendChild(guideLabel);
    const accountBtn = document.createElement("button");
    accountBtn.className = "gc-tab";
    accountBtn.appendChild(icon("user", 14));
    const accountLabel = document.createElement("span");
    accountLabel.textContent = "내 정보";
    accountBtn.appendChild(accountLabel);
    tabs.appendChild(guideBtn);
    tabs.appendChild(accountBtn);
    m.body.appendChild(tabs);

    const host = document.createElement("div");
    host.className = "flex flex-col gap-5";
    m.body.appendChild(host);

    function paint() {
      const guide = tab === "guide";
      guideBtn.classList.toggle("is-active", guide);
      accountBtn.classList.toggle("is-active", !guide);
      titleEl.textContent = guide ? "이용 가이드" : "내 정보";
      if (descEl) {
        descEl.textContent = guide
          ? "팀의 하루는 이 한 바퀴입니다 — 처음이면 위에서 아래로 한 번만 읽으세요."
          : "";
      }
      host.innerHTML = "";
      if (guide) {
        void renderGuideTab(host, me!);
      } else {
        host.appendChild(profileHeader(me!));
        host.appendChild(
          profileForm(me!, (updated) => {
            me = updated;
            if (tab !== "account") return;
            // 전체를 다시 그리지 않고 저장된 값만 반영한다 (탭 유지).
            host.replaceChild(profileHeader(me), host.firstChild!);
          }),
        );
        host.appendChild(passwordForm());
        host.appendChild(sessionRow(m.close));
        host.appendChild(dangerZone(me!, m.close));
      }
    }
    guideBtn.addEventListener("click", () => {
      if (tab === "guide") return;
      tab = "guide";
      paint();
    });
    accountBtn.addEventListener("click", () => {
      if (tab === "account") return;
      tab = "account";
      paint();
    });

    // 서버에서 최신 정보를 한 번 더 읽는다 (다른 기기에서 바꿨을 수 있다).
    // 오프라인이면 캐시가 그대로 오므로 화면이 비지 않는다.
    //
    // 여기서 `setSession` 을 부르면 안 된다: 세션 이벤트는 앱 전체를 다시
    // 그리고, 그 과정에서 열려 있는 dialog 를 닫아 버린다(고아 dialog 방지
    // 코드). 사이드바 갱신은 이 모달이 닫힐 때 `refreshSession` 이 한다.
    void ipc
      .accountRefresh()
      .then((fresh) => {
        if (!fresh) return;
        me = fresh;
        paint();
      })
      .catch(() => undefined);

    paint();
  })();
}

// ─── 이용 가이드 탭 ──────────────────────────────────────────────────────────
//
// README "팀 6명이 시작하는 법"의 축약판이다. 앱 안에서 바로 보이는 게
// 목적이므로 문서보다 짧게, 화면 위치(어디서)를 함께 알려 준다.

/** 한 바퀴의 단계 — 순서와 화면 위치, 누가 하는지. */
interface GuideStep {
  title: string;
  desc: string;
  where: string;
  who: "팀원" | "관리자" | "전체";
  icon: Parameters<typeof icon>[0];
}

const GUIDE_STEPS: GuideStep[] = [
  {
    title: "저장소 등록",
    desc: "각자 clone한 폴더를 + 저장소 추가로 등록합니다. 처음 한 사람은 설정 탭에서 구성원·병합 관리자를 지정하고, 규칙(.gpconfig)은 저장소에 커밋돼 팀원 모두에게 같게 보입니다.",
    where: "홈 · 설정 탭",
    who: "전체",
    icon: "folder",
  },
  {
    title: "내 브랜치에서 작업",
    desc: "새 브랜치를 만들고 평소처럼 코딩한 뒤 커밋·푸시합니다. main에서 직접 작업하지 마세요 — main은 병합 관리자만 만집니다.",
    where: "작업 탭",
    who: "팀원",
    icon: "edit",
  },
  {
    title: "병합 요청 보내기",
    desc: "push만으로는 관리자 승인 대기열에 오르지 않습니다. 작업을 마쳤으면 병합 요청 보내기를 누르세요 — 요청 시점의 커밋이 그대로 검토·병합됩니다.",
    where: "작업 탭",
    who: "팀원",
    icon: "pull",
  },
  {
    title: "검토하고 승인",
    desc: "요청이 오면 알림이 가고 홈 카드에 “N건 병합 승인”이 쌓입니다. 병합 탭의 승인 대기열에서 변경 파일·커밋을 확인하고 병합하기를 누르면 병합→push→팀원 알림까지 한 번에 진행됩니다.",
    where: "병합 탭",
    who: "관리자",
    icon: "merge",
  },
  {
    title: "최신 코드 동기화",
    desc: "병합이 push되면 팀원 전원에게 알림이 옵니다. 내 브랜치에 동기화 버튼 한 번으로 최신 main을 내 브랜치에 반영하고 작업을 계속합니다.",
    where: "알림",
    who: "전체",
    icon: "refresh",
  },
];

/** 로그인한 사람의 저장소 내 역할 — 관리자로 지정된 규칙이 있는지 훑는다.
 *  관리자 미지정 상태(초기)로는 판정하지 않는다(null) — 아무나 병합할 수
 *  있는 팀에서 "당신이 관리자입니다"라고 말하면 오히려 혼란스럽다. */
async function detectRole(
  email: string,
): Promise<{ role: "manager" | "member" | null; managerOf: string[] }> {
  const repos: Repo[] = await ipc.listRepositories().catch(() => []);
  const managerOf: string[] = [];
  let memberSomewhere = false;
  for (const repo of repos) {
    const cfg: ProjectConfigResult | null = await ipc
      .projectConfigGet(repo.id)
      .catch(() => null);
    if (!cfg?.config) continue;
    const targets = cfg.config.merge_targets?.length
      ? cfg.config.merge_targets
      : [cfg.config.default_base_branch || repo.default_branch || "main"];
    for (const base of targets) {
      const managers = mergeManagerEmails(cfg, base);
      if (managers.length === 0) continue; // 미지정 — 판정 보류
      if (managers.includes(email.toLowerCase())) {
        if (!managerOf.includes(repo.display_name)) managerOf.push(repo.display_name);
        continue;
      }
      const member = (cfg.config.members ?? []).find(
        (x) => x.email.toLowerCase() === email.toLowerCase(),
      );
      if (member?.role === "admin") {
        if (!managerOf.includes(repo.display_name)) managerOf.push(repo.display_name);
        continue;
      }
      memberSomewhere = true;
    }
  }
  if (managerOf.length > 0) return { role: "manager", managerOf };
  if (memberSomewhere) return { role: "member", managerOf };
  return { role: null, managerOf };
}

/** 공통 작은 카드 — 제목 + 아이콘 + children. */
function guideCard(title: string, ico: Parameters<typeof icon>[0]): {
  root: HTMLElement;
  body: HTMLElement;
} {
  const root = document.createElement("div");
  root.className = "gc-card flex flex-col gap-3";
  const head = document.createElement("div");
  head.className = "flex items-center gap-2";
  const ic = document.createElement("span");
  ic.className = "text-[color:var(--color-ink-muted)]";
  ic.appendChild(icon(ico, 15));
  head.appendChild(ic);
  const t = document.createElement("div");
  t.className = "font-medium";
  t.textContent = title;
  head.appendChild(t);
  root.appendChild(head);
  const body = document.createElement("div");
  body.className = "flex flex-col gap-2";
  root.appendChild(body);
  return { root, body };
}

/** 한 줄 항목 — 작은 점 + 본문(굵은 앞머리 지원). */
function guideRow(strong: string | null, rest: string): HTMLElement {
  const row = document.createElement("div");
  row.className = "flex items-start gap-2 text-display-sm";
  const dot = document.createElement("span");
  dot.className =
    "mt-[7px] w-1 h-1 rounded-full bg-[color:var(--color-ink-muted)] shrink-0";
  row.appendChild(dot);
  const text = document.createElement("span");
  text.className = "text-[color:var(--color-ink-muted)]";
  if (strong) {
    const b = document.createElement("span");
    b.className = "text-[color:var(--color-ink)] font-medium";
    b.textContent = strong;
    text.appendChild(b);
    text.appendChild(document.createTextNode(rest));
  } else {
    text.textContent = rest;
  }
  row.appendChild(text);
  return row;
}

async function renderGuideTab(root: HTMLElement, me: Account): Promise<void> {
  root.innerHTML = "";

  // ── 1) 한 바퀴 — 5단계 흐름 ─────────────────────────────────────────────
  const flow = document.createElement("div");
  flow.className = "gc-card flex flex-col gap-3";
  const flowHead = document.createElement("div");
  const flowTitle = document.createElement("div");
  flowTitle.className = "font-medium";
  flowTitle.textContent = "팀의 하루 — 이 한 바퀴가 전부입니다";
  flowHead.appendChild(flowTitle);
  const flowSub = document.createElement("div");
  flowSub.className = "text-display-sm text-[color:var(--color-ink-muted)]";
  flowSub.textContent = "브랜치로 작업하고, 요청하고, 승인하고, 모두가 최신 코드를 받습니다.";
  flowHead.appendChild(flowSub);
  flow.appendChild(flowHead);

  const whoBadge: Record<GuideStep["who"], string> = {
    팀원: "gc-badge gc-badge--info",
    관리자: "gc-badge gc-badge--warning",
    전체: "gc-badge gc-badge--muted",
  };
  GUIDE_STEPS.forEach((s, i) => {
    const row = document.createElement("div");
    row.className = "flex items-start gap-3";
    const num = document.createElement("span");
    num.className =
      "inline-flex items-center justify-center w-6 h-6 rounded-full shrink-0 text-display-xs font-semibold text-white";
    num.style.background = "var(--color-primary)";
    num.textContent = String(i + 1);
    row.appendChild(num);
    const mid = document.createElement("div");
    mid.className = "flex-1 min-w-0 flex flex-col gap-0.5";
    const titleRow = document.createElement("div");
    titleRow.className = "flex items-center gap-2 flex-wrap";
    const t = document.createElement("span");
    t.className = "font-medium";
    t.textContent = s.title;
    titleRow.appendChild(t);
    const where = document.createElement("span");
    where.className = "gc-badge gc-badge--neutral font-mono";
    where.textContent = s.where;
    titleRow.appendChild(where);
    const who = document.createElement("span");
    who.className = whoBadge[s.who];
    who.textContent = s.who;
    titleRow.appendChild(who);
    mid.appendChild(titleRow);
    const d = document.createElement("div");
    d.className = "text-display-sm text-[color:var(--color-ink-muted)]";
    d.textContent = s.desc;
    mid.appendChild(d);
    row.appendChild(mid);
    flow.appendChild(row);
    // 마지막 단계가 아니면 연결선 — 흐름이 위→아래로 읽히게.
    if (i < GUIDE_STEPS.length - 1) {
      const link = document.createElement("div");
      link.className = "flex";
      const rail = document.createElement("div");
      rail.className = "w-6 flex justify-center";
      const line = document.createElement("div");
      line.style.cssText =
        "width:2px; flex:1; min-height:10px; background:var(--color-hairline); margin:2px 0;";
      rail.appendChild(line);
      link.appendChild(rail);
      flow.appendChild(link);
    }
  });
  root.appendChild(flow);

  // ── 2) 내 역할 + 역할별 할 일 (비동기 판정 — 도착하면 채운다) ────────────
  const rolesHost = document.createElement("div");
  root.appendChild(rolesHost);
  const myEmail = getSession()?.email ?? me.email;

  function paintRoles(
    myRole: "manager" | "member" | null,
    managerOf: string[],
  ): void {
    rolesHost.innerHTML = "";
    const wrap = document.createElement("div");
    wrap.className = "grid grid-cols-2 gap-3 items-stretch";

    const mk = (
      kind: "manager" | "member",
      title: string,
      sub: string,
      rows: [string, string][],
    ): HTMLElement => {
      const card = document.createElement("div");
      const mine = myRole === kind;
      card.className =
        "gc-card flex flex-col gap-2 h-full " +
        (mine ? "border-[color:var(--color-primary)]" : "");
      if (mine) card.style.boxShadow = "inset 0 0 0 1px var(--color-primary)";
      const head = document.createElement("div");
      head.className = "flex items-center gap-2 flex-wrap";
      const ic = document.createElement("span");
      ic.className = "text-[color:var(--color-ink-muted)]";
      ic.appendChild(icon(kind === "manager" ? "merge" : "edit", 15));
      head.appendChild(ic);
      const t = document.createElement("span");
      t.className = "font-medium";
      t.textContent = title;
      head.appendChild(t);
      if (mine) {
        const mineBadge = document.createElement("span");
        mineBadge.className = "gc-badge gc-badge--success";
        mineBadge.textContent = "내 역할";
        head.appendChild(mineBadge);
      }
      card.appendChild(head);
      const s = document.createElement("div");
      s.className = "text-display-xs text-[color:var(--color-ink-muted)]";
      s.textContent = sub;
      card.appendChild(s);
      if (kind === "manager" && mine && managerOf.length > 0) {
        const of = document.createElement("div");
        of.className = "text-display-xs text-[color:var(--color-ink)] font-medium truncate";
        of.textContent = `내가 관리자: ${managerOf.join(", ")}`;
        of.title = of.textContent;
        card.appendChild(of);
      }
      for (const [strong, rest] of rows) card.appendChild(guideRow(strong, rest));
      return card;
    };

    wrap.appendChild(
      mk("manager", "병합 관리자", "팀원의 브랜치를 main으로 모은다", [
        ["승인 대기열 — ", "병합 탭에서 요청을 파일·커밋 단위로 검토"],
        ["변경 지도 — ", "겹친 파일이 위로, 충돌이 덜 나는 병합 순서 제안"],
        ["병합 후 push — ", "팀원 전원에게 동기화 알림이 간다"],
      ]),
    );
    wrap.appendChild(
      mk("member", "일반 팀원", "내 브랜치에서 작업하고 승인을 받는다", [
        ["작업 탭 — ", "브랜치 → 커밋 → 푸시, 홈 카드가 다음 할 일을 알려 준다"],
        ["병합 요청 — ", "작업이 끝나면 요청 보내기로 승인을 받는다"],
        ["동기화 — ", "병합 완료 알림의 버튼 한 번으로 최신 코드 반영"],
      ]),
    );
    rolesHost.appendChild(wrap);

    if (myRole === null) {
      const hint = document.createElement("div");
      hint.className = "text-display-xs text-[color:var(--color-ink-muted)]";
      hint.textContent =
        "등록한 저장소의 설정 탭에서 구성원·병합 관리자를 지정하면, 여기에 내 역할이 표시됩니다.";
      rolesHost.appendChild(hint);
    }
  }

  paintRoles(null, []);
  void detectRole(myEmail).then(({ role, managerOf }) => {
    if (!root.isConnected) return;
    paintRoles(role, managerOf);
  });

  // ── 3) 알림 규칙 — 누가 무엇을 받는지 ────────────────────────────────────
  const notif = guideCard("알림 규칙", "bell");
  notif.body.appendChild(
    guideRow("팀원의 push·병합 요청 → ", "그 브랜치의 병합 관리자에게만 갑니다. 남의 일은 내 수신함에 쌓이지 않습니다."),
  );
  notif.body.appendChild(
    guideRow("병합 완료 → ", "팀원 전원에게 “내 브랜치에 동기화” 알림이 갑니다."),
  );
  notif.body.appendChild(
    guideRow("도착 위치 — ", "우측 하단 알림(바로 실행 가능)과 사이드바 알림 배지. 알림 탭에서 읽음·모두 읽음으로 정리합니다."),
  );
  root.appendChild(notif.root);

  // ── 4) 충돌과 안전장치 ────────────────────────────────────────────────────
  const safety = guideCard("충돌이 나면 · 안전장치", "check");
  safety.body.appendChild(
    guideRow("블록 단위 해결 — ", "충돌 파일 전체가 아니라 겹친 블록마다 내 것 / 가져온 것 / 직접 편집을 고릅니다. 미결정 블록은 저장 전에 확인됩니다."),
  );
  safety.body.appendChild(
    guideRow("AI 자동 병합 — ", "설정 탭에서 켜 두면 저장된 지침대로 충돌을 고치고 병합 커밋까지 만듭니다. 원본은 항상 백업되고, 결과는 관리자가 확인한 뒤에 push됩니다."),
  );
  safety.body.appendChild(
    guideRow("작업은 사라지지 않습니다 — ", "커밋하지 않은 변경이 있으면 동기화·병합·브랜치 전환이 먼저 거부되고 무엇을 해야 하는지 알려 줍니다."),
  );
  root.appendChild(safety.root);

  // ── 5) 더 읽을 거리 ──────────────────────────────────────────────────────
  const more = document.createElement("div");
  more.className = "text-display-xs text-[color:var(--color-ink-muted)]";
  more.textContent =
    "더 자세한 사용법과 화면별 설명은 저장소의 README.md, docs/WORKFLOW.md에 있습니다.";
  root.appendChild(more);
}

// ─── 프로필 헤더 ─────────────────────────────────────────────────────────────

function profileHeader(me: Account): HTMLElement {
  const box = document.createElement("div");
  box.className = "flex items-center gap-3";

  const avatar = document.createElement("span");
  avatar.className =
    "inline-flex items-center justify-center w-12 h-12 rounded-full bg-[color:var(--color-primary)] text-white text-display-lg font-semibold shrink-0";
  avatar.textContent = (me.name || me.username || "?").trim().charAt(0).toUpperCase();
  box.appendChild(avatar);

  const text = document.createElement("div");
  text.className = "min-w-0 flex flex-col";
  const name = document.createElement("div");
  name.className = "text-display-lg font-medium truncate";
  name.textContent = me.name;
  text.appendChild(name);
  const handle = document.createElement("div");
  handle.className = "text-display-sm text-[color:var(--color-ink-muted)] truncate";
  handle.textContent = `@${me.username} · ${me.email}`;
  text.appendChild(handle);
  const joined = document.createElement("div");
  joined.className = "text-display-xs text-[color:var(--color-ink-muted)]";
  joined.textContent = `${formatJoined(me.created_at)} 가입`;
  text.appendChild(joined);
  box.appendChild(text);

  return box;
}

// ─── 내 정보 수정 ────────────────────────────────────────────────────────────

function profileForm(me: Account, onSaved: (a: Account) => void): HTMLElement {
  const section = document.createElement("form");
  section.className = "gc-card flex flex-col gap-3";
  section.innerHTML = `
    <div class="text-display-md font-medium">프로필</div>
    <label class="flex flex-col gap-1">
      <span class="text-display-sm text-[color:var(--color-ink-muted)]">이름</span>
      <input id="mp-name" class="gc-input" type="text" autocomplete="name" />
    </label>
    <label class="flex flex-col gap-1">
      <span class="text-display-sm text-[color:var(--color-ink-muted)]">이메일</span>
      <input id="mp-email" class="gc-input" type="email" autocomplete="email" />
      <span class="text-display-xs text-[color:var(--color-ink-muted)]">
        팀 구성원·병합 관리자는 이메일로 매칭됩니다. 바꾸면 저장소의
        <code>.gpconfig</code>에 적힌 이메일도 같이 고쳐야 합니다.
      </span>
    </label>
    <label class="flex flex-col gap-1">
      <span class="text-display-sm text-[color:var(--color-ink-muted)]">아이디</span>
      <input class="gc-input" type="text" value="${me.username}" disabled />
      <span class="text-display-xs text-[color:var(--color-ink-muted)]">아이디는 변경할 수 없습니다.</span>
    </label>
    <div id="mp-profile-msg" class="text-display-xs" hidden></div>
    <div class="flex justify-end">
      <button id="mp-profile-save" type="submit" class="gc-button-primary">저장</button>
    </div>
  `;
  const nameInput = section.querySelector<HTMLInputElement>("#mp-name")!;
  const emailInput = section.querySelector<HTMLInputElement>("#mp-email")!;
  nameInput.value = me.name;
  emailInput.value = me.email;
  const msg = section.querySelector<HTMLDivElement>("#mp-profile-msg")!;
  const save = section.querySelector<HTMLButtonElement>("#mp-profile-save")!;

  function show(text: string, kind: "error" | "ok") {
    msg.textContent = text;
    msg.className =
      "text-display-xs " +
      (kind === "error"
        ? "text-[color:var(--color-danger)]"
        : "text-[color:var(--color-success)]");
    msg.hidden = false;
  }

  section.addEventListener("submit", async (ev) => {
    ev.preventDefault();
    const name = nameInput.value.trim();
    const email = emailInput.value.trim();
    msg.hidden = true;
    if (!name || !email) {
      show("이름과 이메일을 입력하세요.", "error");
      return;
    }
    // 바뀐 것이 없으면 서버를 부르지 않는다.
    if (name === me.name && email.toLowerCase() === me.email) {
      show("변경된 내용이 없습니다.", "ok");
      return;
    }
    setBusy(save, true, "저장 중…");
    try {
      const updated = await ipc.accountUpdateProfile(name, email);
      toast("내 정보를 저장했습니다.", "success");
      onSaved(updated);
    } catch (e) {
      show((e as Error).message ?? String(e), "error");
    } finally {
      setBusy(save, false);
    }
  });

  return section;
}

// ─── 비밀번호 변경 ───────────────────────────────────────────────────────────

function passwordForm(): HTMLElement {
  const section = document.createElement("form");
  section.className = "gc-card flex flex-col gap-3";
  section.innerHTML = `
    <div class="text-display-md font-medium">비밀번호 변경</div>
    <label class="flex flex-col gap-1">
      <span class="text-display-sm text-[color:var(--color-ink-muted)]">현재 비밀번호</span>
      <input id="mp-pw-cur" class="gc-input" type="password" autocomplete="current-password" />
    </label>
    <label class="flex flex-col gap-1">
      <span class="text-display-sm text-[color:var(--color-ink-muted)]">새 비밀번호 (8자 이상)</span>
      <input id="mp-pw-new" class="gc-input" type="password" autocomplete="new-password" />
    </label>
    <label class="flex flex-col gap-1">
      <span class="text-display-sm text-[color:var(--color-ink-muted)]">새 비밀번호 확인</span>
      <input id="mp-pw-new2" class="gc-input" type="password" autocomplete="new-password" />
    </label>
    <div id="mp-pw-msg" class="text-display-xs" hidden></div>
    <div class="flex justify-end">
      <button id="mp-pw-save" type="submit" class="gc-button-secondary">비밀번호 변경</button>
    </div>
  `;
  const cur = section.querySelector<HTMLInputElement>("#mp-pw-cur")!;
  const next = section.querySelector<HTMLInputElement>("#mp-pw-new")!;
  const again = section.querySelector<HTMLInputElement>("#mp-pw-new2")!;
  const msg = section.querySelector<HTMLDivElement>("#mp-pw-msg")!;
  const save = section.querySelector<HTMLButtonElement>("#mp-pw-save")!;

  function show(text: string, kind: "error" | "ok") {
    msg.textContent = text;
    msg.className =
      "text-display-xs " +
      (kind === "error"
        ? "text-[color:var(--color-danger)]"
        : "text-[color:var(--color-success)]");
    msg.hidden = false;
  }

  section.addEventListener("submit", async (ev) => {
    ev.preventDefault();
    msg.hidden = true;
    if (!cur.value || !next.value) {
      show("현재 비밀번호와 새 비밀번호를 입력하세요.", "error");
      return;
    }
    // 확인란 불일치는 서버까지 갈 필요가 없다 — 가장 흔한 실수라 즉시 알린다.
    if (next.value !== again.value) {
      show("새 비밀번호가 서로 다릅니다.", "error");
      return;
    }
    if (next.value.length < 8) {
      show("새 비밀번호는 8자 이상이어야 합니다.", "error");
      return;
    }
    setBusy(save, true, "변경 중…");
    try {
      await ipc.accountChangePassword(cur.value, next.value);
      cur.value = next.value = again.value = "";
      show("비밀번호를 변경했습니다.", "ok");
      toast("비밀번호를 변경했습니다.", "success");
    } catch (e) {
      show((e as Error).message ?? String(e), "error");
    } finally {
      setBusy(save, false);
    }
  });

  return section;
}

// ─── 로그아웃 / 계정 전환 ────────────────────────────────────────────────────

function sessionRow(close: () => void): HTMLElement {
  const row = document.createElement("div");
  row.className = "flex flex-wrap items-center gap-2";

  const logout = document.createElement("button");
  logout.className = "gc-button-secondary";
  logout.textContent = "로그아웃";
  logout.addEventListener("click", async () => {
    setBusy(logout, true, "로그아웃 중…");
    try {
      await ipc.accountLogout();
      close();
      await setSession(null);
      toast("로그아웃했습니다.", "info");
    } catch (e) {
      toast(`로그아웃 실패: ${(e as Error).message ?? e}`, "error");
    } finally {
      setBusy(logout, false);
    }
  });
  row.appendChild(logout);

  // "계정 전환"은 결국 로그아웃 후 다시 로그인이다. 목록을 보여 주는 대신
  // 그 두 단계를 한 번에 해 준다.
  const switchBtn = document.createElement("button");
  switchBtn.className = "gc-button-secondary";
  switchBtn.textContent = "다른 계정으로 로그인";
  switchBtn.addEventListener("click", async () => {
    setBusy(switchBtn, true, "전환 중…");
    try {
      await ipc.accountLogout();
      close();
      await setSession(null);
      openAccountModal();
    } catch (e) {
      toast(`전환 실패: ${(e as Error).message ?? e}`, "error");
    } finally {
      setBusy(switchBtn, false);
    }
  });
  row.appendChild(switchBtn);

  return row;
}

// ─── 위험 구역 ───────────────────────────────────────────────────────────────

function dangerZone(me: Account, close: () => void): HTMLElement {
  // 되돌릴 수 없는 동작은 맨 아래에, 시각적으로 분리해서 둔다.
  const box = document.createElement("details");
  box.className = "gc-danger";
  const summary = document.createElement("summary");
  summary.className = "text-display-sm cursor-pointer";
  summary.textContent = "회원 탈퇴";
  box.appendChild(summary);

  const body = document.createElement("div");
  body.className = "flex flex-col gap-2 pt-2";
  const desc = document.createElement("div");
  desc.className = "text-display-xs text-[color:var(--color-ink-muted)] whitespace-pre-line";
  desc.textContent =
    "계정과 로그인 기록이 서버에서 삭제되고 즉시 로그아웃됩니다. 되돌릴 수 없습니다.\n" +
    "등록한 저장소와 커밋은 지워지지 않습니다 — 이 앱의 계정만 삭제됩니다.";
  body.appendChild(desc);

  const btn = document.createElement("button");
  btn.className = "gc-button-secondary self-start text-[color:var(--color-danger)]";
  btn.textContent = "계정 삭제";
  btn.addEventListener("click", async () => {
    const ok = await confirmDialog({
      title: "회원 탈퇴",
      message: `${me.name} (${me.email}) 계정을 삭제합니다.\n되돌릴 수 없습니다. 계속하시겠습니까?`,
      confirmLabel: "탈퇴",
      destructive: true,
    });
    if (!ok) return;
    setBusy(btn, true, "삭제 중…");
    try {
      await ipc.accountDeleteSelf();
      close();
      await setSession(null);
      toast("계정을 삭제했습니다.", "info");
    } catch (e) {
      toast(`탈퇴 실패: ${(e as Error).message ?? e}`, "error");
    } finally {
      setBusy(btn, false);
    }
  });
  body.appendChild(btn);
  box.appendChild(body);

  return box;
}
