// Typed wrapper around Tauri's invoke API.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Uuid = string;

// ─── Repo ───────────────────────────────────────────────────────────────────

export interface Repo {
  id: Uuid;
  path: string;
  display_name: string;
  default_branch: string;
  working_branch: string;
  ssh_host: string;
  ssh_user: string;
  ssh_key_path: string;
  ssh_password: string;
  remote_url: string;
  ed25519_fingerprint: string;
  created_at: string;
}

export interface Branch {
  name: string;
  is_remote: boolean;
  upstream: string | null;
}

export interface Commit {
  sha: string;
  message: string;
  author: string;
  date: string;
  parents: string[];
}

export interface FileChange {
  kind:
    | "added"
    | "modified"
    | "deleted"
    | "renamed"
    | "copied"
    | "untracked"
    | "conflicted";
  path: string;
  staged: boolean;
  unstaged: boolean;
}

export interface WorkingTreeStatus {
  branch: string | null;
  upstream: string | null;
  ahead: number;
  behind: number;
  /**
   * origin/<병합 브랜치>가 현재 브랜치보다 앞선 커밋 수 — "동기화"가 실제로
   * 가져올 양. behind(내 원격 브랜치 기준)와 다르다.
   */
  behind_base?: number;
  files: FileChange[];
}

export interface CommitResult {
  ok: boolean;
  sha: string | null;
  message: string;
}

export interface PushOutcome {
  ok: boolean;
  pushed_sha: string | null;
  message: string;
  /** HTTPS 원격 + 자격증명 부재/실패 → UI가 아이디/비밀번호 모달을 띄워야 한다. */
  auth_required?: boolean;
}

export interface PullOutcome {
  ok: boolean;
  message: string;
  conflicted_files: string[];
}

export interface StashEntry {
  index: string;
  subject: string;
}

// ─── Merge center ────────────────────────────────────────────────────────────

export interface ChangedPath {
  path: string;
  kind: string;
}

export interface PendingBranch {
  name: string;
  short_name: string;
  sha: string;
  author: string;
  unix_time: number;
  subject: string;
  ahead: number;
  behind: number;
  changed_files: ChangedPath[];
  /** True when the branch only exists locally (never pushed). */
  local?: boolean;
  /** 로컬 base에는 이미 병합됐지만 base가 아직 push되지 않은 브랜치. */
  merged_locally?: boolean;
}

/** 병합이 끝나 base에 완전히 포함된 원격 브랜치 — 정리(삭제) 후보. */
export interface MergedRemoteBranch {
  name: string;
  short_name: string;
  author: string;
  unix_time: number;
}

/** 타임라인에 실리는 커밋 한 줄 — `date` 는 RFC3339 작성일. */
export interface TimelineCommit {
  sha: string;
  subject: string;
  author: string;
  date: string;
}

/** base 로 들어온 병합 하나 — `date` 는 병합 커밋의 커밋일(합류 지점). */
export interface TimelineMerge {
  sha: string;
  date: string;
  author: string;
  subject: string;
  /** 병합 커밋 제목에서 복원한 브랜치 이름 (컨벤션을 못 읽으면 null). */
  branch: string | null;
  commits: TimelineCommit[];
  files: string[];
  /** 들어온 커밋 중 가장 이른 작성일 — 레인의 시작점. */
  first_commit_date: string | null;
}

/** 아직 base 에 병합되지 않은 원격 브랜치. */
export interface TimelineOpenBranch {
  name: string;
  commits: TimelineCommit[];
  files: string[];
  first_date: string;
  last_date: string;
}

/** 병합 탭 상단의 "최근 N일 병합 흐름" 데이터. */
export interface MergeTimeline {
  base: string;
  since: string;
  until: string;
  merges: TimelineMerge[];
  direct: TimelineCommit[];
  open: TimelineOpenBranch[];
}

export interface MergeOutcome {
  ok: boolean;
  conflicted: boolean;
  conflicted_files: string[];
  message: string;
}

export interface MergeState {
  in_progress: boolean;
  conflicted_files: string[];
}

export interface ConflictDetail {
  path: string;
  is_binary: boolean;
  too_large: boolean;
  base: string | null;
  ours: string;
  theirs: string;
  working: string;
}

export type Resolution =
  | { type: "ours" }
  | { type: "theirs" }
  | { type: "manual"; content: string };

export type AutoResolveMethod = "ai" | "ours" | "theirs" | "skipped";

export interface AutoFileResolution {
  path: string;
  method: AutoResolveMethod;
  note?: string | null;
}

export interface AutoResolveReport {
  resolved: AutoFileResolution[];
  remaining: string[];
  /**
   * 아직 충돌로 남은 파일마다 "왜 자동으로 안 고쳤는지". 특히 양쪽이 모두 고친
   * 파일은 일부러 손대지 않으므로, 이유가 없으면 사용자는 오류인지 의도인지
   * 구분할 수 없다.
   */
  remainingReasons?: AutoFileResolution[];
  committed: boolean;
  /** Rust 쪽 `AutoResolveReport`는 camelCase로 직렬화한다. */
  backupId?: string | null;
  message: string;
}

export interface BackupEntry {
  id: string;
  created_at: string;
  files: string[];
}

export interface SyncResult {
  conflicted: boolean;
  files: string[];
  message: string;
}

export interface AiConfig {
  enabled: boolean;
  base_url: string;
  api_key: string;
  model: string;
  /** 병합 관리자가 미리 저장해 두는 해결 지침. 비우면 기본 프롬프트를 쓴다. */
  system_prompt: string;
  /** 병합/동기화가 충돌로 끝나면 바로 자동 해결을 실행한다. */
  auto_resolve: boolean;
  /** AI(또는 규칙) 자동 해결로 만든 병합 커밋을 확인 없이 곧바로 push한다 — 완전 자동 루프. */
  auto_push: boolean;
  /** 바이너리·대용량 파일 처리: "theirs" | "ours" */
  binary_strategy: string;
}

// ─── 로그인 계정 / 푸시 자격증명 / 프로젝트 설정 ───────────────────────────────

/**
 * 로그인한 사람. 계정 자체는 팀 서버의 `users` 테이블이 소유하고, 앱은
 * "지금 로그인한 사람"만 캐시한다 — 비밀번호(해시 포함)는 앱에 오지 않는다.
 */
export interface Account {
  id: string;
  name: string;
  /** 소문자. 팀 구성원·병합 관리자는 이메일로 매칭된다 (.gpconfig). */
  email: string;
  /** 로그인 아이디 (소문자). */
  username: string;
  /** 서버가 준 ISO-8601 문자열. */
  created_at: string;
}

export interface PushCredential {
  username: string;
  password: string;
}

export interface GpMember {
  id: string;
  name: string;
  email: string;
  role: string;
}

export interface ProjectConfig {
  gpconfig_version: number;
  default_base_branch: string;
  members: GpMember[];
  /** branch → 구성원 이메일 (그 브랜치의 병합 관리자) */
  merge_managers: Record<string, string>;
  /** 병합 대상 브랜치 목록 — 이 브랜치들로만 병합할 수 있다. 비어 있으면 default_base_branch만 대상. */
  merge_targets: string[];
  notify_recipients: string[];
  notify: { on_branch_ready: boolean; on_merge_complete: boolean };
}

export interface ProjectConfigResult {
  exists: boolean;
  config: ProjectConfig;
}

export interface ProjectConfigSaveResult {
  config: ProjectConfig;
  commit: { ok: boolean; message: string } | null;
}

// ─── SSH Profile ─────────────────────────────────────────────────────────────

export interface SshProfile {
  default_user: string;
  default_key_path: string;
  default_host: string;
  connect_timeout: string;
  default_port: number;
  // optional password auth (empty = key-based)
  default_password: string;
}

export interface TestSshArgs {
  host: string;
  user: string;
  port: number;
  key_path: string;
  password: string;
  timeout_secs: number;
}

export interface SshTestReport {
  ok: boolean;
  latency_ms: number;
  user: string;
  hostname: string;
  system: string;
  fingerprint: string;
  error: string | null;
}

// ─── Register / Patch ────────────────────────────────────────────────────────

/** SSH connection parameters for registration / remote browsing. */
export interface SshTarget {
  ssh_user: string;
  ssh_host: string;
  ssh_key_path: string;
  ssh_password: string;
  ssh_port: number;
}

export interface RegisterProjectArgs extends SshTarget {
  project_path: string;
}

export interface SshDirEntry {
  name: string;
  is_dir: boolean;
  is_symlink: boolean;
}

/** Result of browsing one remote directory over SSH. */
export interface SshDirListing {
  /** Resolved absolute path on the remote (from `pwd` after `cd`). */
  path: string;
  /** True when the path is inside a git work tree. */
  git_repo: boolean;
  entries: SshDirEntry[];
}

export interface RepoPatch {
  display_name?: string | null;
  working_branch?: string | null;
  ssh_user?: string | null;
  ssh_host?: string | null;
  ssh_key_path?: string | null;
  ssh_password?: string | null;
  ssh_port?: number | null;
}

// ─── IPC ────────────────────────────────────────────────────────────────────

export const ipc = {
  // repo
  listRepositories: () => invoke<Repo[]>("list_repositories"),
  registerRepository: (args: RegisterProjectArgs) =>
    invoke<Repo>("register_repository", { args }),
  browseSshDir: (target: SshTarget, path: string) =>
    invoke<SshDirListing>("browse_ssh_dir", { target, path }),
  /** 아직 git 저장소가 아닌 폴더를 저장소로 만들고 바로 등록한다. */
  initRepository: (path: string) => invoke<Repo>("init_repository", { path }),
  removeRepository: (id: Uuid) => invoke<void>("remove_repository", { id }),
  updateRepository: (id: Uuid, patch: RepoPatch) =>
    invoke<Repo>("update_repository", { id, patch }),

  // branches
  listBranches: (repoId: Uuid) =>
    invoke<Branch[]>("list_branches", { repoId }),
  checkoutBranch: (repoId: Uuid, branch: string) =>
    invoke<void>("checkout_branch", { repoId, branch }),
  createBranch: (repoId: Uuid, branch: string) =>
    invoke<void>("create_branch", { repoId, branch }),

  // commits
  listCommits: (repoId: Uuid, branch: string, count: number) =>
    invoke<Commit[]>("list_commits", { repoId, branch, count }),

  // git ops
  status: (repoId: Uuid) =>
    invoke<WorkingTreeStatus>("status", { repoId }),
  addFiles: (repoId: Uuid, paths: string[]) =>
    invoke<WorkingTreeStatus>("add_files", { repoId, paths }),
  commit: (repoId: Uuid, message: string, stageAll: boolean) =>
    invoke<CommitResult>("commit", { repoId, message, stageAll }),
  push: (repoId: Uuid, branch?: string | null) =>
    invoke<PushOutcome>("push", {
      repoId,
      branch: branch ?? null,
      credentials: null,
      saveCredential: false,
    }),
  pushBranch: (repoId: Uuid, branch: string) =>
    invoke<PushOutcome>("push", { repoId, branch }),
  pull: (repoId: Uuid) =>
    invoke<PullOutcome>("pull", { repoId }),
  diff: (repoId: Uuid, pathspec: string | null, staged: boolean, stat: boolean) =>
    invoke<string>("diff", { repoId, pathspec, staged, stat }),
  stash: (repoId: Uuid, action: string) =>
    invoke<void>("stash", { repoId, action }),
  stashList: (repoId: Uuid) =>
    invoke<StashEntry[]>("stash_list", { repoId }),

  // SSH profile
  getSshProfile: () =>
    invoke<SshProfile>("get_ssh_profile"),
  setSshProfile: (profile: SshProfile) =>
    invoke<void>("set_ssh_profile", { patch: profile }),
  testSshConnection: (args: TestSshArgs) =>
    invoke<SshTestReport>("test_ssh_connection", { args }),

  // merge center
  fetchRepo: (repoId: Uuid) =>
    invoke<string>("fetch_repo", { repoId }),
  listPendingBranches: (repoId: Uuid, base: string) =>
    invoke<PendingBranch[]>("list_pending_branches", { repoId, base }),
  startMerge: (repoId: Uuid, branchRef: string, base: string, expectedSha?: string | null) =>
    invoke<MergeOutcome>("start_merge", {
      repoId,
      branchRef,
      base,
      expectedSha: expectedSha ?? null,
    }),
  mergeState: (repoId: Uuid) =>
    invoke<MergeState>("merge_state", { repoId }),
  baseUnpushedCount: (repoId: Uuid, base: string) =>
    invoke<number>("base_unpushed_count", { repoId, base }),
  listMergedRemoteBranches: (repoId: Uuid, base: string) =>
    invoke<MergedRemoteBranch[]>("list_merged_remote_branches", { repoId, base }),
  mergeTimeline: (repoId: Uuid, base: string, days: number) =>
    invoke<MergeTimeline>("merge_timeline", { repoId, base, days }),
  deleteRemoteBranch: (repoId: Uuid, base: string, branch: string) =>
    invoke<void>("delete_remote_branch", { repoId, base, branch }),
  branchFileDiff: (repoId: Uuid, base: string, branchRef: string, path: string) =>
    invoke<string>("branch_file_diff", { repoId, base, branchRef, path }),
  conflictDetail: (repoId: Uuid, path: string) =>
    invoke<ConflictDetail>("conflict_detail", { repoId, path }),
  resolveConflict: (repoId: Uuid, path: string, resolution: Resolution) =>
    invoke<string[]>("resolve_conflict", { repoId, path, resolution }),
  abortMerge: (repoId: Uuid) =>
    invoke<void>("abort_merge", { repoId }),
  completeMerge: (repoId: Uuid, message?: string) =>
    invoke<MergeOutcome>("complete_merge", { repoId, message: message ?? null }),

  // auto merge / sync
  mergeAutoResolve: (
    repoId: Uuid,
    binaryStrategy?: "ours" | "theirs",
  ) =>
    invoke<AutoResolveReport>("merge_auto_resolve", {
      repoId,
      binaryStrategy: binaryStrategy ?? null,
    }),
  mergeBackupList: (repoId: Uuid) =>
    invoke<BackupEntry[]>("merge_backup_list", { repoId }),
  mergeBackupRestore: (repoId: Uuid, backupId: string) =>
    invoke<number>("merge_backup_restore", { repoId, backupId }),
  syncBranch: (repoId: Uuid, base: string) =>
    invoke<SyncResult>("sync_branch", { repoId, base }),

  // AI config
  getAiConfig: () => invoke<AiConfig>("get_ai_config"),
  setAiConfig: (cfg: AiConfig) => invoke<void>("set_ai_config", { cfg }),
  aiDefaultPrompt: () => invoke<string>("ai_default_prompt"),
  aiSuggestResolution: (
    filePath: string,
    base: string | null,
    ours: string,
    theirs: string,
  ) =>
    invoke<string>("ai_suggest_resolution", {
      filePath,
      base,
      ours,
      theirs,
    }),

  // ── accounts (로그인) ────────────────────────────────────────────
  // 계정은 팀 서버가 소유한다. accountCurrent 만 로컬 캐시를 읽으므로
  // 오프라인에서도 즉시 답한다.
  accountRegister: (name: string, email: string, username: string, password: string) =>
    invoke<Account>("account_register", { name, email, username, password }),
  accountLoginByPassword: (username: string, password: string) =>
    invoke<Account>("account_login_by_password", { username, password }),
  /**
   * Google 로그인 — 서버가 Google OAuth 를 지원할 때만 쓸 수 있다.
   * 로그인 창이 닫힐 때까지 기다렸다가 로그인한 계정을 돌려준다.
   */
  googleLoginStart: () => invoke<Account>("google_login_start"),
  accountLogout: () => invoke<void>("account_logout"),
  accountCurrent: () => invoke<Account | null>("account_current"),
  /** 서버에서 내 정보를 다시 읽는다. 오프라인이면 캐시를 그대로 돌려준다. */
  accountRefresh: () => invoke<Account | null>("account_refresh"),
  accountUpdateProfile: (name?: string, email?: string) =>
    invoke<Account>("account_update_profile", { name, email }),
  accountChangePassword: (currentPassword: string, newPassword: string) =>
    invoke<void>("account_change_password", { currentPassword, newPassword }),
  /** 회원 탈퇴 — 서버에서 계정을 지우고 로그아웃한다. */
  accountDeleteSelf: () => invoke<void>("account_delete_self"),
  /** 팀 구성원 검색 (이름/아이디/이메일). 2자 이상부터 결과가 온다. */
  accountSearch: (query: string) => invoke<Account[]>("account_search", { query }),

  // ── push credentials (푸시 자격증명) ─────────────────────────────
  pushCredentialsList: () =>
    invoke<Record<string, PushCredential>>("push_credentials_list"),
  pushCredentialSet: (repoId: Uuid, credential: PushCredential) =>
    invoke<void>("push_credential_set", {
      repoId,
      username: credential.username,
      password: credential.password,
    }),
  pushCredentialDelete: (repoId: Uuid) =>
    invoke<void>("push_credential_delete", { repoId }),

  // ── project config (.gpconfig) ──────────────────────────────────
  projectConfigGet: (repoId: Uuid) =>
    invoke<ProjectConfigResult>("project_config_get", { repoId }),
  projectConfigSet: (repoId: Uuid, config: ProjectConfig, autoCommit: boolean) =>
    invoke<ProjectConfigSaveResult>("project_config_set", {
      repoId,
      config,
      autoCommit,
    }),
  projectConfigCommit: (repoId: Uuid) =>
    invoke<{ ok: boolean; message: string }>("project_config_commit", { repoId }),

  // ── push (자격증명 선택 전달) ────────────────────────────────────
  // `credentials`가 없으면 저장된 자격증명/SSH만 사용하고, HTTPS 원격이라면
  // 결과의 auth_required가 true로 돌아온다 → UI가 아이디/비밀번호 모달을 띄운다.
  pushWithCredentials: (
    repoId: Uuid,
    branch: string | null | undefined,
    credentials?: PushCredential | null,
    saveCredential?: boolean,
  ) =>
    invoke<PushOutcome>("push", {
      repoId,
      branch: branch ?? null,
      credentials: credentials ?? null,
      saveCredential: saveCredential ?? false,
    }),
};


export function listenEvent<T>(name: string, cb: (payload: T) => void): Promise<UnlistenFn> {
  return listen<T>(name, (e) => cb(e.payload));
}

// ─── Peer / Team ─────────────────────────────────────────────────────────────

export interface PeerConfig {
  backend_url: string;
  device_token: string;
  device_id: string;
  device_name: string;
  last_poll_port: number | null;
}

export interface PeerDeviceInfo {
  id: string;
  name: string;
  user_id: string;
}

export interface PeerProjectInfo {
  id: string;
  display_name: string;
  join_code: string;
  role: string;
}

export interface TeamEventRow {
  id: string;
  project_id: string;
  sender_device_name: string;
  event_kind: string;
  repo_name: string;
  payload: string;
  received_at: string;
  read: boolean;
}

export interface MemberInfo {
  device_id: string | null;
  email: string | null;
  name: string | null;
  role: string;
  joined_at: string | null;
}

export interface RepoLinkSummary {
  repo_id: Uuid;
  display_name: string;
  path: string;
}

export const ipc_peer = {
  getConfig: () => invoke<PeerConfig>("peer_get_config"),
  setBackendUrl: (url: string) => invoke<void>("peer_set_backend_url", { url }),
  /** 저장한(또는 넘긴) 주소로 실제 연결되는지 확인한다. */
  checkBackend: (url?: string) =>
    invoke<{ ok: boolean; message: string }>("peer_check_backend", { url }),
  registerDevice: (backendUrl: string, name: string) =>
    invoke<{ id: string; name: string; user_id: string }>("peer_register_device", { backendUrl, name }),
  pollNow: () => invoke<void>("peer_poll_now"),
  listProjects: () => invoke<PeerProjectInfo[]>("peer_list_projects"),
  createProject: (name: string, repoId?: string | null) =>
    invoke<PeerProjectInfo>("peer_create_project", { name, repoId }),
  joinProject: (code: string, repoId?: string | null) =>
    invoke<PeerProjectInfo>("peer_join_project", { code, repoId }),
  leaveProject: (projectId: string) =>
    invoke<void>("peer_leave_project", { projectId }),
  linkRepo: (repoId: Uuid, projectId: string) =>
    invoke<void>("peer_link_repo_to_project", { repoId, projectId }),
  unlinkRepo: (repoId: Uuid, projectId: string) =>
    invoke<void>("peer_unlink_repo", { repoId, projectId }),
  reposForProject: (projectId: string) =>
    invoke<RepoLinkSummary[]>("peer_repos_for_project", { projectId }),
  unreadCount: () => invoke<number>("peer_unread_count"),
  listTeamEvents: (limit: number, unreadOnly: boolean) =>
    invoke<TeamEventRow[]>("peer_list_team_events", { limit, unreadOnly }),
  markTeamRead: (id: string) => invoke<void>("peer_mark_team_read", { id }),
  markAllTeamRead: () => invoke<number>("peer_mark_all_team_read"),
  localUrl: () => invoke<string>("peer_local_url"),
  inviteByEmail: (
    projectId: string,
    email: string,
    name?: string | null,
    role?: string | null
  ) =>
    invoke<{ device_id: string | null; email: string; role: string; pending: boolean }>(
      "peer_invite_by_email",
      { projectId, email, name, role }
    ),
  listMembers: (projectId: string) =>
    invoke<MemberInfo[]>("peer_list_members", { projectId }),
  removeEmailInvite: (projectId: string, email: string) =>
    invoke<void>("peer_remove_email_invite", { projectId, email }),
};