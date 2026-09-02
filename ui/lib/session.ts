// 로그인 세션 상태 — 사이드바 칩, 프로젝트 설정, 푸시 권한 등이 공유한다.
import { ipc, type Account } from "./ipc";

export const ACCOUNT_EVENT = "gc-account-changed";

let current: Account | null | undefined; // undefined = 아직 모름

export function getSession(): Account | null | undefined {
  return current;
}

export function isLoggedIn(): boolean {
  return !!current;
}

/** 세션을 백엔드에서 다시 읽고 구독자에게 알린다. */
export async function refreshSession(): Promise<Account | null> {
  current = await ipc.accountCurrent().catch(() => null);
  window.dispatchEvent(new CustomEvent(ACCOUNT_EVENT, { detail: current }));
  return current;
}

/** 로그인/로그아웃/계정 변경 후 호출 — 즉시 세션 갱신. */
export async function setSession(account: Account | null): Promise<void> {
  current = account;
  window.dispatchEvent(new CustomEvent(ACCOUNT_EVENT, { detail: account }));
}