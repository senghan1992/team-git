// 푸시 실행 공통 유틸 — 저장된 자격증명 자동 사용, 없으면 아이디/비밀번호 모달.
// 성공 시(저장 체크) 설정에 자격증명을 저장해 다음 푸시부터 자동 입력된다.
import { ipc, type PushOutcome, type Repo } from "../lib/ipc";
import { openModal } from "./Modal";
import { toast } from "./Toast";
import { setBusy } from "./Busy";

export type PushFlowResult = "ok" | "cancelled" | PushOutcome;

/**
 * 현재 브랜치를 origin에 푸시한다.
 * - 저장된 자격증명이 있으면 그대로 사용 (modal 없이).
 * - HTTPS 원격 + 자격증명 없음 → 아이디/비밀번호 모달 (저장 체크 시 config에 보관).
 * - SSH 원격 → 기존 SSH 인증(키/sshpass) 사용.
 * @returns "ok" | "cancelled" | PushOutcome(실패)
 */
export async function openPushCredentialFlow(
  repo: Repo,
  branch: string | null,
  prefill?: { username: string; password: string } | null,
): Promise<PushFlowResult> {
  const attempt = (credentials?: { username: string; password: string } | null, saveCredential = false) =>
    ipc.pushWithCredentials(repo.id, branch, credentials ?? null, saveCredential);

  // 1) 저장된 자격증명이 있으면 자동 사용. 성공/실패를 판정해서 돌려주고,
  //    인증 거부(만료·회수)면 저장값을 미리 채운 로그인 모달로 이어간다.
  const saved = await ipc.pushCredentialsList().catch(() => ({} as Record<string, { username: string; password: string }>));
  const savedCred = saved[repo.id] ?? null;
  let savedCredExpired = false;
  if (savedCred && !prefill) {
    const res = await attempt(savedCred, false);
    if (res.ok) return "ok";
    if (!res.auth_required) return res;
    savedCredExpired = true;
    prefill = savedCred;
  }

  // 2) 먼저 자격증명 없이 시도 (SSH 원격은 이대로 성공, HTTPS는 auth_required).
  //    (저장된 자격증명이 방금 거부된 경우는 건너뛰고 곧장 모달로 간다.)
  if (!savedCredExpired) {
    const outcome = await attempt(prefill, false);
    if (outcome.ok) return "ok";
    if (!outcome.auth_required) return outcome;
  }

  // 3) HTTPS + 인증 필요 → 아이디/비밀번호 모달.
  return await new Promise<PushFlowResult>((resolve) => {
    const m = openModal({
      title: "Git 호스트 로그인",
      description: savedCredExpired
        ? `${repo.display_name} — 저장된 자격증명이 더 이상 유효하지 않습니다. 다시 입력하세요.`
        : `${repo.display_name} — origin에 푸시하려면 Git 호스트 아이디/비밀번호가 필요합니다.`,
      submitLabel: "푸시",
      onSubmit: async (close) => {
        const username = (m.body.querySelector<HTMLInputElement>("#push-user")!).value.trim();
        const password = (m.body.querySelector<HTMLInputElement>("#push-pass")!).value;
        const save = (m.body.querySelector<HTMLInputElement>("#push-save")!)?.checked ?? false;
        if (!username || !password) {
          m.setError("아이디와 비밀번호를 입력하세요.");
          return;
        }
        m.setSubmitting(true);
        m.setError(null);
        try {
          const res = await attempt({ username, password }, save);
          if (res.ok) {
            if (save) {
              await ipc.pushCredentialSet(repo.id, { username, password }).catch(() => undefined);
              toast("자격증명을 설정에 저장했습니다. 다음 푸시부터 자동 입력됩니다.", "info");
            }
            close();
            resolve("ok");
          } else {
            // 모달을 열어 둔 채 메시지만 보여 준다 — 사용자가 고쳐서 재시도하거나
            // 닫으면 "cancelled"로 끝난다.
            m.setError(res.message || "푸시 실패");
            m.setSubmitting(false);
          }
        } catch (e) {
          m.setError((e as Error).message ?? String(e));
          m.setSubmitting(false);
        }
      },
    });

    m.body.innerHTML = `
      <div class="flex flex-col gap-3">
        <label class="flex flex-col gap-1">
          <span class="text-display-sm text-[color:var(--color-ink-muted)]">아이디</span>
          <input id="push-user" class="gc-input" type="text" autocomplete="username" value="${(prefill?.username ?? "").replace(/"/g, "&quot;")}" />
        </label>
        <label class="flex flex-col gap-1">
          <span class="text-display-sm text-[color:var(--color-ink-muted)]">비밀번호 / 토큰</span>
          <input id="push-pass" class="gc-input" type="password" autocomplete="current-password" value="${(prefill?.password ?? "").replace(/"/g, "&quot;")}" />
        </label>
        <label class="flex items-center gap-2 text-display-sm cursor-pointer">
          <input id="push-save" type="checkbox" />
          <span>이 자격증명을 설정에 저장 (다음부터 자동 입력)</span>
        </label>
        <div class="text-display-xs text-[color:var(--color-ink-muted)]">
          저장한 자격증명은 이 기기의 설정(푸시 자격증명)에서 언제든 삭제할 수 있습니다.
        </div>
      </div>
    `;

    m.el.addEventListener("close", () => {
      // 제출로 닫힌 경우는 이미 resolve됨 — 그 외(취소/백드롭)는 cancelled.
      setTimeout(() => resolve("cancelled"), 0);
      // resolve 중복 방지: close 이후에는 resolve 자체가 no-op이 아니면 문제 없음
      // (Promise는 첫 resolve만 반영된다).
    });
  });
}

/** 버튼에 붙이는 표준 푸시 핸들러 — RepoView 등에서 재사용. */
export function bindPushButton(
  btn: HTMLButtonElement,
  repo: Repo,
  branch: () => string | null,
): void {
  btn.addEventListener("click", async () => {
    setBusy(btn, true, "푸시 중…");
    try {
      const outcome = await openPushCredentialFlow(repo, branch());
      if (outcome === "ok") toast("푸시 완료", "success");
      else if (outcome === "cancelled") toast("푸시를 취소했습니다.", "info");
      else toast(`푸시 실패: ${outcome.message || "알 수 없는 오류"}`, "error");
    } finally {
      setBusy(btn, false);
    }
  });
}