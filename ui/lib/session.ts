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

/** 세션을 백엔드에서 다시 읽고, **바뀐 경우에만** 구독자에게 알린다.
 *  예전에는 무조건 이벤트를 쏘았다 — 이용 가이드 모달을 열었다 닫기만 해도
 *  ACCOUNT_EVENT 가 발행돼 앱 전체가 다시 그려지고, 그 덕에 병합 탭이
 *  통째로 다시 로딩됐다. 같은 계정이면 조용히 넘어간다 (사이드바 칩은
 *  어차피 같은 값을 보여 준다). */
export async function refreshSession(): Promise<Account | null> {
  const prev = current;
  current = await ipc.accountCurrent().catch(() => null);
  const changed =
    prev === undefined ||
    JSON.stringify(prev ?? null) !== JSON.stringify(current ?? null);
  if (changed) {
    window.dispatchEvent(new CustomEvent(ACCOUNT_EVENT, { detail: current }));
  }
  return current;
}

/** 로그인/로그아웃/계정 변경 후 호출 — 즉시 세션 갱신. */
export async function setSession(account: Account | null): Promise<void> {
  current = account;
  window.dispatchEvent(new CustomEvent(ACCOUNT_EVENT, { detail: account }));
}