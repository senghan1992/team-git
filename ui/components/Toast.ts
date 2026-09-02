import { icon } from "./Icon";

type Toast = {
  id: number;
  text: string;
  kind: "info" | "success" | "error";
  /** 보조 설명 — 본문 아래에 작게 표시된다. */
  detail?: string;
  /** 액션 버튼 — 알림을 행동으로 이어준다 (예: 내 브랜치에 동기화). */
  action?: { label: string; run: () => void };
};

const toasts: Toast[] = [];
const listeners: Array<(t: Toast[]) => void> = [];

let nextId = 1;
function emit() {
  for (const l of listeners) l([...toasts]);
}

/** 자동으로 사라지는 일반 토스트. */
export function toast(text: string, kind: Toast["kind"] = "info") {
  const id = nextId++;
  pushToast({ id, text, kind });
  setTimeout(() => dismiss(id), 4200);
}

/**
 * 행동을 가진 알림 — 우측 하단에 떠서 액션 버튼을 누르거나 닫을 때까지
 * 유지된다. 팀원의 main 푸시가 와서 동기화를 제안할 때 쓴다.
 */
export function notify(
  text: string,
  action: { label: string; run: () => void },
  detail?: string,
) {
  const id = nextId++;
  pushToast({ id, text, kind: "info", detail, action });
  // 12초 안에 액션을 누르지 않으면 조용히 사라진다.
  setTimeout(() => dismiss(id), 12000);
  return () => dismiss(id);
}

function pushToast(t: Toast) {
  toasts.push(t);
  emit();
}

export function dismiss(id: number) {
  const i = toasts.findIndex((x) => x.id === id);
  if (i >= 0) {
    toasts.splice(i, 1);
    emit();
  }
}

export function subscribeToasts(cb: (t: Toast[]) => void) {
  listeners.push(cb);
  cb([...toasts]);
  return () => {
    const i = listeners.indexOf(cb);
    if (i >= 0) listeners.splice(i, 1);
  };
}

function iconForKind(kind: Toast["kind"]): "check" | "x" | "info" {
  if (kind === "success") return "check";
  if (kind === "error") return "x";
  return "info";
}

export function renderToasts(): HTMLElement {
  // 우측 하단 알림 스택 — 팀 이벤트(동기화 제안)가 작업 흐름을 가리지 않는다.
  const wrap = document.createElement("div");
  wrap.className = "gc-toast-stack";
  subscribeToasts((ts) => {
    wrap.innerHTML = "";
    for (const t of ts) {
      const el = document.createElement("div");
      el.className = "gc-toast gc-toast--" + t.kind + (t.action ? " gc-toast--actionable" : "");
      const iconWrap = document.createElement("span");
      iconWrap.className = "gc-toast__icon";
      iconWrap.appendChild(icon(iconForKind(t.kind), 13));
      el.appendChild(iconWrap);
      const body = document.createElement("span");
      body.className = "gc-toast__body";
      const text = document.createElement("span");
      text.textContent = t.text;
      body.appendChild(text);
      if (t.detail) {
        const d = document.createElement("span");
        d.className = "gc-toast__detail";
        d.textContent = t.detail;
        body.appendChild(d);
      }
      el.appendChild(body);
      if (t.action) {
        const btn = document.createElement("button");
        btn.className = "gc-toast__action";
        btn.textContent = t.action.label;
        btn.addEventListener("click", () => {
          dismiss(t.id);
          t.action!.run();
        });
        el.appendChild(btn);
      }
      const close = document.createElement("button");
      close.className = "gc-toast__close";
      close.setAttribute("aria-label", "알림 닫기");
      close.appendChild(icon("x", 12));
      close.addEventListener("click", () => dismiss(t.id));
      el.appendChild(close);
      wrap.appendChild(el);
    }
  });
  return wrap;
}