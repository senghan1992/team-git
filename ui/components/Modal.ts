import { spinner } from "./Icon";

export interface ModalProps {
  title: string;
  description?: string;
  submitLabel?: string;
  cancelLabel?: string;
  hideFooter?: boolean;
  initialFocusSelector?: string;
  onSubmit?: (close: () => void) => void | Promise<void>;
  onCancel?: () => void;
}

export interface ModalHandle {
  el: HTMLDialogElement;
  body: HTMLElement;
  setSubmitting(b: boolean): void;
  setError(msg: string | null): void;
  close(): void;
}

let activeModal: HTMLDialogElement | null = null;

export function openModal(props: ModalProps): ModalHandle {
  if (activeModal) {
    activeModal.remove();
    activeModal = null;
  }

  const {
    title,
    description,
    submitLabel = "저장",
    cancelLabel = "취소",
    hideFooter,
    initialFocusSelector,
    onSubmit,
    onCancel,
  } = props;

  const dialog = document.createElement("dialog");
  dialog.className = "gc-modal";
  activeModal = dialog;

  dialog.innerHTML = `
    <div class="gc-modal__panel">
      <div class="gc-modal__header">
        <div class="gc-modal__title">${escape(title)}</div>
        ${description ? `<div class="gc-modal__description">${escape(description)}</div>` : ""}
      </div>
      <div class="gc-modal__body"></div>
      ${!hideFooter ? `
      <div class="gc-modal__footer">
        <div class="gc-modal__error" role="alert"></div>
        <div class="flex gap-2">
          <button class="gc-button-secondary" data-cancel>${escape(cancelLabel)}</button>
          ${onSubmit ? `<button class="gc-button-primary" data-submit>${escape(submitLabel)}</button>` : ""}
        </div>
      </div>
      ` : ""}
    </div>
  `;

  const body = dialog.querySelector(".gc-modal__body") as HTMLElement;
  const errorEl = dialog.querySelector<HTMLElement>(".gc-modal__error") ?? null;
  const submitBtn = dialog.querySelector<HTMLButtonElement>("[data-submit]");
  const cancelBtn = dialog.querySelector<HTMLButtonElement>("[data-cancel]");

  let submitting = false;

  const handle: ModalHandle = {
    el: dialog,
    body,
    setSubmitting(b: boolean) {
      submitting = b;
      if (!submitBtn) return;
      if (b) {
        if (submitBtn.dataset.prevLabel !== undefined) { submitBtn.disabled = true; return; }
        submitBtn.dataset.prevLabel = submitBtn.innerHTML;
        submitBtn.innerHTML = "";
        submitBtn.appendChild(spinner(14));
        const s = document.createElement("span");
        s.textContent = submitBtn.dataset.prevLabel ?? "";
        submitBtn.appendChild(s);
      } else if (submitBtn.dataset.prevLabel !== undefined) {
        submitBtn.innerHTML = submitBtn.dataset.prevLabel;
        delete submitBtn.dataset.prevLabel;
      }
      submitBtn.disabled = b;
    },
    setError(msg: string | null) {
      if (errorEl) errorEl.textContent = msg ?? "";
    },
    close() {
      dialog.close();
      dialog.remove();
      if (activeModal === dialog) activeModal = null;
    },
  };

  // 커밋 메시지·충돌 편집처럼 타이핑한 내용은 백드롭 클릭/ESC/취소 오클릭
  // 한 번에 사라지면 안 된다 — 텍스트를 입력한 적이 있을 때만 닫기 전에
  // 한 번 확인한다. (프로그램이 채운 값은 input 이벤트가 없어 해당 없음.)
  let typedSomething = false;
  body.addEventListener("input", (e) => {
    const t = e.target;
    if (t instanceof HTMLTextAreaElement) {
      typedSomething = true;
    } else if (
      t instanceof HTMLInputElement &&
      ["text", "password", "email", "url", "search"].includes(t.type)
    ) {
      typedSomething = true;
    }
  });
  const confirmDiscard = async (): Promise<boolean> => {
    if (!typedSomething) return true;
    return confirmDialog({
      title: "입력한 내용이 있습니다",
      message: "닫으면 입력한 내용이 사라집니다. 닫을까요?",
      confirmLabel: "닫기",
      destructive: true,
    });
  };

  dialog.addEventListener("click", (e) => {
    if (e.target === dialog) {
      void confirmDiscard().then((ok) => {
        if (!ok) return;
        onCancel?.();
        handle.close();
      });
    }
  });

  dialog.addEventListener("cancel", (e) => {
    // ESC — 바로 닫지 말고 입력 보호 확인을 거친다.
    e.preventDefault();
    void confirmDiscard().then((ok) => {
      if (!ok) return;
      onCancel?.();
      handle.close();
    });
  });

  if (submitBtn) {
    submitBtn.addEventListener("click", async () => {
      if (submitting) return;
      handle.setError(null);
      handle.setSubmitting(true);
      try {
        if (onSubmit) {
          await onSubmit(() => handle.close());
        } else {
          handle.close();
        }
      } catch (e) {
        handle.setError((e as Error).message ?? String(e));
      } finally {
        if (document.contains(dialog)) handle.setSubmitting(false);
      }
    });
  }

  if (cancelBtn) {
    cancelBtn.addEventListener("click", () => {
      void confirmDiscard().then((ok) => {
        if (!ok) return;
        onCancel?.();
        handle.close();
      });
    });
  }

  document.body.appendChild(dialog);
  dialog.showModal();

  if (initialFocusSelector) {
    const target = body.querySelector<HTMLElement>(initialFocusSelector);
    target?.focus();
  } else {
    const defaultTarget = body.querySelector<HTMLElement>("input,select,textarea,button");
    defaultTarget?.focus();
  }

  return handle;
}

export async function confirmDialog(opts: {
  title: string;
  message: string;
  confirmLabel?: string;
  destructive?: boolean;
}): Promise<boolean> {
  return new Promise<boolean>((resolve) => {
    const dialog = document.createElement("dialog");
    dialog.className = "gc-modal";

    const confirmLabel = opts.confirmLabel ?? "확인";
    const btnClass = opts.destructive
      ? "gc-button-primary bg-[color:var(--color-danger)] hover:bg-[color:var(--color-danger-hover)]"
      : "gc-button-primary";

    dialog.innerHTML = `
      <div class="gc-modal__panel">
        <div class="gc-modal__header">
          <div class="gc-modal__title">${escape(opts.title)}</div>
          ${opts.message ? `<div class="gc-modal__description">${escape(opts.message)}</div>` : ""}
        </div>
        <div class="gc-modal__footer">
          <div class="flex gap-2 justify-end">
            <button class="gc-button-secondary" data-cancel>취소</button>
            <button class="${btnClass}" data-confirm>${escape(confirmLabel)}</button>
          </div>
        </div>
      </div>
    `;

    const confirmBtn = dialog.querySelector<HTMLButtonElement>("[data-confirm]")!;
    const cancelBtn = dialog.querySelector<HTMLButtonElement>("[data-cancel]")!;

    const cleanup = () => {
      dialog.close();
      dialog.remove();
    };

    dialog.addEventListener("click", (e) => {
      if (e.target === dialog) { cleanup(); resolve(false); }
    });

    confirmBtn.addEventListener("click", () => { cleanup(); resolve(true); });
    cancelBtn.addEventListener("click", () => { cleanup(); resolve(false); });

    dialog.addEventListener("cancel", () => { cleanup(); resolve(false); });

    document.body.appendChild(dialog);
    dialog.showModal();
    confirmBtn.focus();
  });
}

function escape(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}
