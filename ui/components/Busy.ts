import { spinner } from "./Icon";

export function setBusy(el: HTMLElement, busy: boolean, label = "처리 중…"): void {
  if (busy) {
    if (el.dataset.prevHtml !== undefined) { (el as HTMLButtonElement).disabled = true; return; }
    el.setAttribute("aria-busy", "true");
    el.dataset.prevHtml = el.innerHTML;
    el.innerHTML = "";
    el.appendChild(spinner(14));
    const s = document.createElement("span");
    s.textContent = label;
    el.appendChild(s);
    (el as HTMLButtonElement).disabled = true;
  } else {
    el.removeAttribute("aria-busy");
    if (el.dataset.prevHtml !== undefined) {
      el.innerHTML = el.dataset.prevHtml;
      delete el.dataset.prevHtml;
    }
    (el as HTMLButtonElement).disabled = false;
  }
}

export function renderPageLoading(label = "불러오는 중…"): HTMLElement {
  const d = document.createElement("div");
  d.className = "gc-page-loading";
  const ring = document.createElement("div");
  ring.className = "gc-loading-ring";
  ring.appendChild(spinner(24));
  d.appendChild(ring);
  const s = document.createElement("span");
  s.className = "gc-page-loading__label";
  s.textContent = label;
  d.appendChild(s);
  return d;
}

export function renderPageLoadingFill(label?: string): HTMLElement {
  const d = renderPageLoading(label);
  d.classList.add("gc-page-loading--fill");
  return d;
}