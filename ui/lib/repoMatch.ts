// 팀 알림 이벤트 → 내 등록 저장소 매칭.
//
// 예전에는 폴더 이름(display_name)으로만 찾아서, 같은 이름의 저장소가 둘이면
// 모든 알림 액션이 막다른 길이었고 이름을 다르게 지은 팀원은 아예 매칭이
// 안 됐다. 팀이 실제로 공유하는 열쇠는 origin의 remote URL이므로 그걸 먼저
// 쓰고, URL이 없는 (구버전) 이벤트만 이름으로 폴백한다.
import type { Repo } from "./ipc";

/**
 * 원격 URL을 비교 가능한 열쇠(`host/path`)로 정규화한다.
 * Rust의 `git::normalize_remote_url`과 같은 규칙 — 두 구현이 같은 열쇠를
 * 만들어야 송신(훅)과 수신(앱)이 만난다.
 */
export function normalizeRemoteUrl(url: string): string {
  let s = url.trim();
  const scheme = s.indexOf("://");
  if (scheme >= 0) s = s.slice(scheme + 3);
  const at = s.lastIndexOf("@");
  if (at >= 0) s = s.slice(at + 1); // 자격증명·scp의 user 부분 제거
  const colon = s.indexOf(":");
  if (colon >= 0) {
    const head = s.slice(colon + 1).split("/")[0] ?? "";
    const isPort = head.length > 0 && /^[0-9]+$/.test(head);
    if (!isPort) s = s.slice(0, colon) + "/" + s.slice(colon + 1); // scp 형식
  }
  s = s.replace(/\/+$/, "");
  if (s.endsWith(".git")) s = s.slice(0, -4);
  s = s.replace(/\/+$/, "");
  const slash = s.indexOf("/");
  return slash >= 0 ? s.slice(0, slash).toLowerCase() + s.slice(slash) : s.toLowerCase();
}

/** 이벤트 payload에서 정규화된 remote URL을 꺼낸다 (없으면 null). */
export function remoteUrlOfPayload(payload: string): string | null {
  try {
    const parsed = JSON.parse(payload) as { data?: { url?: string } };
    const u = parsed.data?.url?.trim();
    return u ? normalizeRemoteUrl(u) : null;
  } catch {
    return null;
  }
}

/**
 * 이벤트가 가리키는 내 등록 저장소를 찾는다.
 * ① remote URL이 일치하는 저장소 (유일하면 그것, 여럿이면 이름까지 같은 쪽 우선)
 * ② URL이 없거나 못 찾으면 display_name — 단, 유일할 때만.
 */
export function repoForEvent(
  repos: Repo[],
  ev: { repo_name: string; payload: string },
): Repo | null {
  const key = remoteUrlOfPayload(ev.payload);
  if (key) {
    const byUrl = repos.filter(
      (r) => r.remote_url && normalizeRemoteUrl(r.remote_url) === key,
    );
    if (byUrl.length === 1) return byUrl[0]!;
    if (byUrl.length > 1) {
      // 같은 origin을 여러 번 등록한 드문 경우 — 이름까지 맞는 쪽을 고른다.
      return byUrl.find((r) => r.display_name === ev.repo_name) ?? byUrl[0]!;
    }
  }
  const byName = repos.filter((r) => r.display_name === ev.repo_name);
  return byName.length === 1 ? byName[0]! : null;
}
