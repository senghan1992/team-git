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

  dialog.addEventListener("click", (e) => {
    if (e.target === dialog) {
      onCancel?.();
      handle.close();
    }
  });

  dialog.addEventListener("cancel", () => {
    onCancel?.();
    setTimeout(() => {
      if (document.contains(dialog)) {
        dialog.remove();
        if (activeModal === dialog) activeModal = null;
      }
    }, 0);
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
      onCancel?.();
      handle.close();
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
