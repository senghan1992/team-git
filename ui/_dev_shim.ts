// Stub for running the UI in a plain browser (no Tauri native bridge).
// Loaded before any @tauri-apps/api code — sets window.__TAURI_INTERNALS__
// so that ipc.ts invoke() calls resolve to canned mock data.

const mockData: Record<string, any> = {
  list_repositories: [
    {
      id: "r1",
      path: "/repo/a",
      display_name: "demo-app",
      default_branch: "main",
      base_branch: "main",
      working_branch: "main",
      ssh_host: "",
      ssh_user: "",
      ssh_key_path: "",
      ssh_password: "",
      ssh_port: 22,
      remote_url: "https://example.com/demo-app.git",
      ed25519_fingerprint: "",
      maintainers: [],
      channel_ids: ["c1"],
      hook_installed: true,
      created_at: new Date().toISOString(),
    },
  ],
  list_channels: [
    {
      id: "c1",
      kind: "slack",
      name: "team",
      url: "https://hooks.slack.com/services/T000/B000/XXXX",
      default_for_main_push: true,
      default_for_branch_push: true,
      default_for_release: false,
      created_at: new Date().toISOString(),
    },
  ],
  list_inbox: [
    {
      id: "n1",
      channel_id: "c1",
      channel_kind: "slack",
      event_kind: "branch_push",
      repo_name: "demo-app",
      payload: JSON.stringify({
        event: "branch_push",
        data: {
          author: "alice",
          message: "feat: x",
          sha: "abc1234",
          repo_name: "demo-app",
          branch: "feat",
          url: "",
        },
      }),
      status_code: 200,
      error: null,
      sent_at: new Date(Date.now() - 3600_000).toISOString(),
      read: false,
    },
  ],
  is_first_run: false,
  get_settings: {
    schema_version: 1,
    first_run_completed: true,
    repositories: [],
    channels: [],
    sync: { auto_fetch_on_open: true, conflict_strategy: "manual" },
  },
  list_branches: [
    { name: "main", is_remote: false, upstream: "origin/main" },
    { name: "feat", is_remote: false, upstream: null },
    { name: "origin/main", is_remote: true, upstream: null },
  ],
  list_commits: {
    commits: [
      {
        sha: "aaaaaaa",
        message: "merge feat",
        author: "bob",
        date: new Date().toISOString(),
        parents: ["bbbbbbb", "ccccccc"],
      },
      {
        sha: "bbbbbbb",
        message: "feat: b",
        author: "alice",
        date: new Date(Date.now() - 86400_000).toISOString(),
        parents: ["ddddddd"],
      },
      {
        sha: "ddddddd",
        message: "initial",
        author: "alice",
        date: new Date(Date.now() - 172800_000).toISOString(),
        parents: [],
      },
    ],
    total: 3,
    page: 1,
    per_page: 50,
  },
  status: {
    branch: "main",
    upstream: "origin/main",
    ahead: 0,
    behind: 2,
    files: [
      { kind: "modified", path: "src/lib/session.ts", staged: true, unstaged: false },
      { kind: "added", path: "src/lib/peer.ts", staged: false, unstaged: true },
      { kind: "untracked", path: "docs/flow.md", staged: false, unstaged: false },
      { kind: "deleted", path: "src/lib/old.ts", staged: false, unstaged: true },
    ],
  },
  diff: (args: { pathspec: string }) =>
    `diff --git a/${args.pathspec} b/${args.pathspec}\nindex 0a1b2c3..d4e5f6 100644\n--- a/${args.pathspec}\n+++ b/${args.pathspec}\n@@ -1,6 +1,8 @@\n import { ipc } from "./ipc";\n+import { notify } from "./Toast";\n \n export async function load() {\n-  const ok = await ipc.status();\n+  const ok = await ipc.status();\n+  const fresh = await ipc.refresh();\n   return ok;\n }` as never,
  // ── accounts & project config (.gpconfig) — 설정 탭 미리보기용 ──
  account_list: [
    {
      id: "u-me",
      name: "김민지",
      email: "minji@example.com",
      username: null,
      password_hash: null,
      created_at: new Date().toISOString(),
    },
    {
      id: "u2",
      name: "박준호",
      email: "junho@example.com",
      username: null,
      password_hash: null,
      created_at: new Date().toISOString(),
    },
    {
      id: "u3",
      name: "이서연",
      email: "seoyeon@example.com",
      username: null,
      password_hash: null,
      created_at: new Date().toISOString(),
    },
    {
      id: "u4",
      name: "정도윤",
      email: "doyoon@example.com",
      username: null,
      password_hash: null,
      created_at: new Date().toISOString(),
    },
  ],
  account_current: {
    id: "u-me",
    name: "김민지",
    email: "minji@example.com",
    username: null,
    password_hash: null,
    created_at: new Date().toISOString(),
  },
  account_login: (args: { id: string }) =>
    ({ id: args.id, name: "김민지", email: "minji@example.com", username: null, password_hash: null, created_at: new Date().toISOString() }) as never,
  project_config_get: {
    exists: true,
    config: {
      gpconfig_version: 2,
      default_base_branch: "main",
      members: [
        { id: "u-me", name: "김민지", email: "minji@example.com", role: "admin" },
        { id: "u2", name: "박준호", email: "junho@example.com", role: "member" },
      ],
      merge_managers: { "release/1.0": "junho@example.com" },
      merge_targets: ["main", "release/1.0", "feature/login"],
      notify_recipients: ["junho@example.com"],
      notify: { on_branch_ready: true, on_merge_complete: false },
    },
  },
  project_config_set: (args: { config: unknown }) =>
    ({
      config: args.config,
      commit: { ok: true, message: "mock commit" },
    }) as never,
  peer_list_projects: [{ id: "p1", display_name: "Test Project", join_code: "TEST-0001", role: "admin" }],
  peer_create_project: { id: "p1", display_name: "Test Project", join_code: "TEST-0001", role: "admin" },
  peer_join_project: { id: "p2", display_name: "Joined Project", join_code: "JOIN-0002", role: "member" },
  peer_leave_project: undefined,
  peer_list_members: [{ device_id: null, email: "a@b.com", name: null, role: "member", pending: true }],
  peer_invite_by_email: undefined,
  peer_remove_email_invite: undefined,
  peer_unread_count: 0,
  peer_repos_for_project: [],
  peer_list_team_events: [
    {
      id: "team_evt_1",
      project_id: "p1",
      sender_device_name: "김민지의 MacBook",
      event_kind: "main_push",
      repo_name: "demo-app",
      payload: JSON.stringify({
        kind: "main_push",
        data: {
          author: "김민지",
          message: "feature/login 브렌치 병합",
          sha: "abcdef0",
          repo_name: "demo-app",
          branch: "main",
          url: "",
        },
      }),
      received_at: new Date().toISOString(),
      read: false,
    },
  ],
  sync_branch: { conflicted: false, files: [], message: "mock sync ok" },
  peer_poll_now: undefined,
};
interface TauriInternals {
  invoke: (cmd: string, args: unknown) => Promise<unknown>;
  listen: (name: string) => Promise<() => void>;
}
const globals = globalThis as unknown as { __TAURI_INTERNALS__?: TauriInternals };
if (globals.__TAURI_INTERNALS__) {
  // Production Tauri already injected the real bridge — leave it alone.
} else {
  globals.__TAURI_INTERNALS__ = {
    invoke: async (cmd: string, args: unknown): Promise<unknown> => {
      let body: unknown = null;
      let gotJson = false;
      try {
        const r = await fetch("/__gc/invoke", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ cmd, args }),
        });
        try {
          body = await r.json();
          gotJson = true;
        } catch {
          // non-JSON response — treat as bridge offline
        }
      } catch {
        // bridge not running — fall back to mock data below
      }
      if (gotJson) {
        const env = body as { kind?: unknown; message?: unknown } | null;
        if (env !== null && typeof env === "object" && "kind" in env) {
          // The dev bridge answered with an error envelope — surface it so
          // the UI shows a real message instead of resolving null/[] and
          // crashing with a cryptic TypeError.
          throw new Error(
            typeof env.message === "string"
              ? env.message
              : `[dev bridge] ${String(env.kind)}`,
          );
        }
        return body;
      }
      console.debug("[stub] invoke", cmd, "→", mockData[cmd] ?? []);
      const stub = mockData[cmd];
      return typeof stub === "function" ? (stub as (a: unknown) => unknown)(args) : (stub ?? []);
    },
    listen: async (_name: string) => () => {},
  };
}

export {};
