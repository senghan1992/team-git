// "지금 무엇을 해야 하는가" — 저장소 상태 + 내 역할에서 다음 한 가지 행동을 뽑는다.
//
// 이 앱의 사용자는 두 역할뿐이다: 자기 브랜치에서 작업하는 팀원과, 병합 브랜치로
// 모아 주는 병합 관리자. 화면에 git 버튼을 늘어놓으면 둘 다 무엇을 눌러야 할지
// 모른다. 그래서 상태를 우선순위대로 훑어 **행동 하나**로 줄인다.
//
// 우선순위는 "막힌 것 → 남에게 영향 주는 것 → 내 것" 순서다.

import type { ProjectConfigResult, WorkingTreeStatus } from "../lib/ipc";
import type { RepoTab } from "./Sidebar";

export type NextActionKind =
  | "resolve" // 충돌이 남아 있다 — 이걸 풀기 전엔 아무것도 못 한다
  | "merge" // 내가 병합 관리자이고 병합 대기 브랜치가 있다
  | "commit" // 작업 트리에 커밋 안 한 변경이 있다
  | "push" // 커밋했지만 원격에 안 올렸다 — 관리자가 볼 수 없다
  | "sync" // 병합 브랜치가 앞서 있다 — 내 브랜치에 당겨와야 한다
  | "clean"; // 할 일 없음

export interface NextAction {
  kind: NextActionKind;
  /** 버튼 문구 — 바로 누를 수 있는 동사. */
  label: string;
  /** 왜 이걸 해야 하는지 한 줄. */
  reason: string;
  /** 열어야 할 탭 (sync/clean은 없음). */
  tab: RepoTab | null;
  /** 눈에 띄게 할지 — 남을 기다리게 하는 일이면 true. */
  urgent: boolean;
}

export interface NextActionInput {
  status: WorkingTreeStatus | null;
  /** 병합 대기 브랜치 수 (모르면 null — 그 규칙은 건너뛴다). */
  pendingCount: number | null;
  /** 로그인한 사람이 이 저장소 병합 대상 브랜치의 관리자인가. */
  isMergeManager: boolean;
  /** 병합 대상(기본) 브랜치 이름 — 문구에 쓴다. */
  baseBranch: string;
}

export function computeNextAction(input: NextActionInput): NextAction {
  const { status, pendingCount, isMergeManager, baseBranch } = input;

  const conflicted = status?.files.filter((f) => f.kind === "conflicted").length ?? 0;
  if (conflicted > 0) {
    return {
      kind: "resolve",
      label: `충돌 ${conflicted}개 해결`,
      reason: "병합이 충돌로 멈춰 있습니다. 해결해야 다음 작업을 할 수 있습니다.",
      tab: "merge",
      urgent: true,
    };
  }

  // 관리자가 병합을 안 해 주면 팀 전체가 막힌다 — 내 커밋보다 우선.
  if (isMergeManager && pendingCount !== null && pendingCount > 0) {
    return {
      kind: "merge",
      label: `${pendingCount}건 병합하기`,
      reason: `팀원이 푸시한 브랜치 ${pendingCount}개가 ${baseBranch} 병합을 기다리고 있습니다.`,
      tab: "merge",
      urgent: true,
    };
  }

  const dirty =
    status?.files.filter((f) => f.kind !== "conflicted").length ?? 0;
  if (dirty > 0) {
    return {
      kind: "commit",
      label: `변경 ${dirty}개 커밋`,
      reason: "커밋하지 않은 변경이 있습니다. 커밋해야 원격에 올릴 수 있습니다.",
      tab: "work",
      urgent: false,
    };
  }

  const ahead = status?.ahead ?? 0;
  if (ahead > 0) {
    return {
      kind: "push",
      label: `커밋 ${ahead}개 푸시`,
      reason: "푸시하면 병합 관리자에게 알림이 가고 병합을 받을 수 있습니다.",
      tab: "work",
      urgent: true,
    };
  }

  const behind = status?.behind ?? 0;
  if (behind > 0) {
    return {
      kind: "sync",
      label: `최신 ${behind}개 가져오기`,
      reason: `${baseBranch}에 팀원들의 작업이 반영되어 있습니다. 내 브랜치에 동기화하세요.`,
      tab: "work",
      urgent: false,
    };
  }

  return {
    kind: "clean",
    // "열기" 는 저장소 화면 헤더에서 외부 도구(에디터)를 뜻한다. 같은 단어를
    // 다른 뜻으로 쓰면 어느 쪽이 무엇인지 매번 확인해야 한다.
    label: "저장소 보기",
    reason: "최신 상태입니다. 할 일이 없습니다.",
    tab: "work",
    urgent: false,
  };
}

/**
 * 로그인한 사람이 이 프로젝트의 병합 대상 브랜치 관리자인지 판정한다.
 * 관리자가 아무도 지정되지 않았으면 (아직 `.gpconfig`가 없는 초기 상태)
 * 누구나 병합할 수 있으므로 true로 본다.
 */
export function isMergeManagerFor(
  cfg: ProjectConfigResult | null,
  myEmail: string | null,
  baseBranch: string,
): boolean {
  const managers = cfg?.config?.merge_managers ?? {};
  const assigned = managers[baseBranch];
  if (!assigned) return true;
  if (!myEmail) return false;
  if (assigned.toLowerCase() === myEmail.toLowerCase()) return true;
  // admin 은 모든 브랜치를 병합할 수 있다 — 병합 센터와 같은 규칙.
  return (cfg?.config?.members ?? []).some(
    (m) => m.email.toLowerCase() === myEmail.toLowerCase() && m.role === "admin",
  );
}
