// Vite plugin that lets the Vite dev server answer the same Tauri IPC calls
// the Rust backend handles in production. Config and registered repositories
// live in the real `~/.config/com.gitcompanion.app/config.json`, so what you
// see in the browser matches what the bundled app would do. This file is
// imported only from `vite.config.ts` — `vite build` will not bundle it into
// the production output.
//
// Covered commands (subset of the Rust IPC surface, focused on the merge
// center and the everyday add/commit/push/pull workflow):
//   list_repositories, register_repository, remove_repository,
//   update_repository, list_branches, list_commits, status, add_files,
//   commit, push, push_branch, pull, diff, stash, stash_list,
//   create_branch, checkout_branch, fetch_repo,
//   list_pending_branches, start_merge, merge_state, conflict_detail,
//   resolve_conflict, abort_merge, complete_merge, get_ai_config,
//   set_ai_config, ai_suggest_resolution, get_ssh_profile, set_ssh_profile,
//   test_ssh_connection, browse_ssh_dir, list_external_tools, set_external_tool, remove_external_tool,
//   open_external_tool,
//   account_register, account_list, account_delete, account_login, account_logout,
//   account_current, push_credentials_list, push_credential_set, push_credential_delete,
//   project_config_get, project_config_set, project_config_commit.
//
// Peer / team calls (`peer_*`) intentionally fall through to the dev shim
// because they require the FastAPI backend to be running.

import type { Plugin, ViteDevServer } from "vite";
import { randomUUID, createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, readFileSync, readdirSync, statSync, unlinkSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join, dirname } from "node:path";
import type { IncomingMessage, ServerResponse } from "node:http";

// ── config.json read/write ──────────────────────────────────────────────────

interface RepoRecord {
  id: string;
  path: string;
  display_name: string;
  default_branch: string;
  working_branch?: string;
  ssh_host?: string;
  ssh_user?: string;
  ssh_key_path?: string;
  ssh_password?: string;
  ed25519_fingerprint?: string;
  ssh_port?: number;
  remote_url?: string;
  created_at: string;
}

interface AiConfigRecord {
  enabled: boolean;
  base_url: string;
  api_key: string;
  model: string;
}

interface AccountRecord {
  id: string;
  name: string;
  email: string;
  username?: string | null;
  password_hash?: string | null;
  created_at: string;
}

interface PushCredentialRecord {
  username: string;
  password: string;
}

interface GpMemberRecord {
  id: string;
  name: string;
  email: string;
  role: string;
}

interface ProjectConfigRecord {
  gpconfig_version?: number;
  default_base_branch?: string;
  members?: GpMemberRecord[];
  merge_managers?: Record<string, string>;
  merge_targets?: string[];
  notify_recipients?: string[];
  notify?: { on_branch_ready?: boolean; on_merge_complete?: boolean };
}

function defaultProjectConfig(): ProjectConfigRecord {
  return {
    gpconfig_version: 2,
    default_base_branch: "",
    members: [],
    merge_managers: {},
    merge_targets: [],
    notify_recipients: [],
    notify: { on_branch_ready: false, on_merge_complete: false },
  };
}

/** Rust gpconfig::normalize와 동일: 이메일 중복 제거, 비구성원 관리자/수신자 제거. */
function normalizeProjectConfig(cfg: ProjectConfigRecord): ProjectConfigRecord {
  const out = defaultProjectConfig();
  out.gpconfig_version = 2;
  out.default_base_branch = cfg.default_base_branch ?? "";
  const seen: string[] = [];
  out.members = (cfg.members ?? []).filter((m) => {
    const email = (m.email ?? "").trim().toLowerCase();
    if (!email || seen.includes(email)) return false;
    seen.push(email);
    return true;
  });
  out.merge_managers = {};
  for (const [br, email] of Object.entries(cfg.merge_managers ?? {})) {
    const e = (email ?? "").trim().toLowerCase();
    if (seen.includes(e)) out.merge_managers[br] = e;
  }
  out.notify_recipients = [...new Set((cfg.notify_recipients ?? [])
    .map((e) => e.trim().toLowerCase())
    .filter((e) => seen.includes(e)))];
  out.notify = {
    on_branch_ready: !!cfg.notify?.on_branch_ready,
    on_merge_complete: !!cfg.notify?.on_merge_complete,
  };
  // 병합 대상 브랜치: 공백 제거 + 순서 유지 중복 제거 (Rust normalize와 동일).
  const seenTargets = new Set<string>();
  out.merge_targets = [];
  for (const b of cfg.merge_targets ?? []) {
    const v = (b ?? "").trim();
    if (!v || seenTargets.has(v)) continue;
    seenTargets.add(v);
    out.merge_targets.push(v);
  }
  return out;
}

interface AppSettings {
  schema_version: number;
  repositories: RepoRecord[];
  projects?: unknown[];
  external_tools?: unknown[];
  ssh_profile?: Record<string, unknown>;
  peer?: Record<string, unknown>;
  ai?: AiConfigRecord;
  accounts?: AccountRecord[];
  active_account_id?: string | null;
  push_credentials?: Record<string, PushCredentialRecord>;
}

const APP_DIR = "com.gitcompanion.app";
const CONFIG_FILE = "config.json";

function configPath(): string {
  const override = process.env.GC_DEV_CONFIG;
  if (override) return override;
  return join(homedir(), ".config", APP_DIR, CONFIG_FILE);
}

function loadSettings(): AppSettings {
  const p = configPath();
  if (!existsSync(p)) {
    const empty: AppSettings = { schema_version: 8, repositories: [] };
    mkdirSync(join(homedir(), ".config", APP_DIR), { recursive: true });
    writeFileSync(p, JSON.stringify(empty, null, 2));
    return empty;
  }
  let s: AppSettings;
  try {
    s = JSON.parse(readFileSync(p, "utf8")) as AppSettings;
  } catch {
    s = { schema_version: 8, repositories: [] };
  }
  if (ensureSeedAccounts(s)) {
    saveSettings(s);
  }
  return s;
}

function saveSettings(s: AppSettings): void {
  writeFileSync(configPath(), JSON.stringify(s, null, 2));
}

// ── 로그인 (Rust config_store와 동일한 해시/시드 계정) ───────────────

function hashPassword(username: string, password: string): string {
  return createHash("sha256").update(`git-companion::${username}:${password}`).digest("hex");
}

const SEED_ACCOUNTS: Array<[string, string, string, string]> = [
  ["test", "test", "테스트 1", "test@example.com"],
  ["test2", "test2", "테스트 2", "test2@example.com"],
];

function ensureSeedAccounts(s: AppSettings): boolean {
  let changed = false;
  for (const [username, password, name, email] of SEED_ACCOUNTS) {
    if (s.accounts?.some((a) => (a.username ?? "").toLowerCase() === username)) continue;
    s.accounts ??= [];
    s.accounts.push({
      id: hashPassword(username, "seed-" + username).slice(0, 8) + "-" + randomUUID(),
      name,
      email,
      username,
      password_hash: hashPassword(username, password),
      created_at: new Date().toISOString(),
    });
    changed = true;
  }
  return changed;
}

function findRepo(s: AppSettings, id: string): RepoRecord | undefined {
  return s.repositories.find((r) => r.id === id);
}

// ── git wrappers ────────────────────────────────────────────────────────────

interface GitResult {
  ok: boolean;
  stdout: string;
  stderr: string;
}

function git(cwd: string, args: string[]): GitResult {
  const res = spawnSync("git", ["-c", "core.quotepath=off", ...args], {
    cwd,
    env: { ...process.env, LC_ALL: "C.UTF-8", LANG: "C.UTF-8" },
    encoding: "utf8",
    maxBuffer: 32 * 1024 * 1024,
  });
  if (res.error) {
    return { ok: false, stdout: "", stderr: res.error.message };
  }
  return {
    ok: (res.status ?? 1) === 0,
    stdout: res.stdout ?? "",
    stderr: res.stderr ?? "",
  };
}

function gitWithEnv(cwd: string, args: string[], extraEnv: Record<string, string>): GitResult {
  const res = spawnSync("git", ["-c", "core.quotepath=off", ...args], {
    cwd,
    env: { ...process.env, LC_ALL: "C.UTF-8", LANG: "C.UTF-8", ...extraEnv },
    encoding: "utf8",
    maxBuffer: 32 * 1024 * 1024,
  });
  if (res.error) return { ok: false, stdout: "", stderr: res.error.message };
  return {
    ok: (res.status ?? 1) === 0,
    stdout: res.stdout ?? "",
    stderr: res.stderr ?? "",
  };
}

/** GIT_ASKPASS 스크립트 본문 — 비밀번호는 argv/env가 아닌 임시 파일 안에만 존재한다. */
function askpassScript(user: string, pass: string): string {
  const esc = (s: string) => s.replace(/'/g, `'\\''`);
  return `#!/bin/sh\ncase \"$1\" in\n  *Username*|*username*) echo '${esc(user)}' ;;\n  *) echo '${esc(pass)}' ;;\nesac\n`;
}

function isAuthFailure(stderr: string): boolean {
  const e = stderr.toLowerCase();
  return [
    "could not read username",
    "could not read password",
    "authentication failed",
    "invalid username or password",
    "http basic: access denied",
    "access denied",
    "requested url returned error: 401",
    "requested url returned error: 403",
  ].some((m) => e.includes(m));
}

/** HTTPS 푸시: 임시 GIT_ASKPASS로 자격증명 주입 (Rust ops::push_with_askpass와 동일). */
function pushWithAskpass(t: GitTarget, branch: string, cred: PushCredentialRecord): GitResult {
  const script = askpassScript(cred.username, cred.password);
  if (t.ssh) {
    const rel = `../.gc-askpass-${randomUUID()}.sh`;
    writeWorkingTree(t, rel, script);
    const remote = `GIT_ASKPASS='${rel.replace(/'/g, `'\\''`)}' GIT_TERMINAL_PROMPT='0' git -C ${shellQuoteArg(t.path)} -c core.quotepath=off push origin 'HEAD:${branch.replace(/'/g, `'\\''`)}'`;
    const res = sshRun(t.ssh, remote);
    sshRun(t.ssh, `rm -f '${rel.replace(/'/g, `'\\''`)}'`);
    return { ok: res.ok, stdout: res.stdout, stderr: res.stderr };
  }
  const path = join(tmpdir(), `gc-askpass-${randomUUID()}.sh`);
  writeFileSync(path, script);
  try {
    chmodSync(path, 0o700);
  } catch { /* windows only */ }
  const res = gitWithEnv(t.path, ["push", "origin", `HEAD:${branch}`], {
    GIT_ASKPASS: path,
    GIT_TERMINAL_PROMPT: "0",
  });
  try {
    unlinkSync(path);
  } catch { /* already gone */ }
  return res;
}

interface SshOptions {
  user: string;
  host: string;
  key_path: string;
  port: number;
  timeout_secs: number;
  /** optional user/password auth; empty = key-based */
  password?: string;
}

interface SshResult {
  ok: boolean;
  stdout: string;
  stderr: string;
}

/** Run a remote command over SSH from the dev server (real connection).
 * Key-based by default; password auth drives `ssh` via `sshpass -e` (the
 * password travels in the SSHPASS env var, never on the command line).
 * When both a key and a password are configured, the password is tried
 * first (the user's preference) and the key is used as a fallback if the
 * server rejects it — some servers (Ubuntu default) block root password
 * logins while still accepting keys. */
function sshRun(opts: SshOptions, remoteCmd: string, input?: string | Buffer): SshResult {
  const attempt = (useKey: boolean): SshResult => {
    const args: string[] = [];
    const prog = useKey ? "ssh" : "sshpass";
    if (useKey) {
      if (opts.key_path) args.push("-i", opts.key_path);
      args.push(
        "-o", "BatchMode=yes",
        "-o", "StrictHostKeyChecking=accept-new",
        "-o", "PreferredAuthentications=publickey",
      );
    } else {
      args.push("-e", "ssh");
      args.push(
        "-o", "StrictHostKeyChecking=accept-new",
        "-o", "PreferredAuthentications=password",
        "-o", "PubkeyAuthentication=no",
        "-o", "NumberOfPasswordPrompts=1",
      );
    }
    if (opts.port && opts.port !== 22) args.push("-p", String(opts.port));
    args.push("-o", `ConnectTimeout=${opts.timeout_secs || 5}`);
    args.push(opts.user ? `${opts.user}@${opts.host}` : opts.host, remoteCmd);
    const res = spawnSync(prog, args, {
      encoding: "utf8",
      maxBuffer: 32 * 1024 * 1024,
      input,
      env: useKey ? process.env : { ...process.env, SSHPASS: opts.password },
    });
    if (res.error) {
      return {
        ok: false,
        stdout: "",
        stderr:
          !useKey && res.error.message.includes("ENOENT")
            ? "비밀번호 인증에는 sshpass가 필요합니다. (sudo apt install sshpass)"
            : res.error.message,
      };
    }
    return {
      ok: (res.status ?? 1) === 0,
      stdout: res.stdout ?? "",
      stderr: res.stderr ?? "",
    };
  };

  if (opts.password && opts.key_path) {
    // 비밀번호 먼저 시도하고, 서버가 거부하면 키로 재시도한다.
    const pw = attempt(false);
    if (pw.ok || !/Permission denied/.test(pw.stderr)) return pw;
    return attempt(true);
  }
  return opts.password ? attempt(false) : attempt(true);
}

/** Shell-quote a path so the remote `cd` cannot be escaped. */
function shellQuotePath(p: string): string {
  return `'${p.replace(/'/g, `'\\''`)}'`;
}

/** A git work tree on this machine (local) or reachable over SSH. */
interface GitTarget {
  path: string;
  ssh: SshOptions | null;
}

function targetOf(r: RepoRecord): GitTarget {
  return {
    path: r.path ?? "",
    ssh: r.ssh_host
      ? {
          user: r.ssh_user ?? "",
          host: r.ssh_host,
          key_path: r.ssh_key_path ?? "",
          password: r.ssh_password ?? "",
          port: r.ssh_port ?? 22,
          timeout_secs: 5,
        }
      : null,
  };
}

/** Run git on a target; ssh repos shell-quote every arg (mirrors the Rust app). */
function tgGit(t: GitTarget, args: string[]): GitResult {
  if (!t.ssh) return git(t.path, args);
  const remote = `git -C ${shellQuoteArg(t.path)} -c core.quotepath=off ${args.map(shellQuoteArg).join(" ")}`;
  const res = sshRun(t.ssh, remote);
  return { ok: res.ok, stdout: res.stdout, stderr: res.stderr };
}

function shellQuoteArg(s: string): string {
  return `'${s.replace(/'/g, `'\\''`)}'`;
}

/** Read a working-tree file (via ssh `cat` for ssh repos). */
function readWorkingTree(t: GitTarget, relPath: string): string {
  if (t.ssh) {
    return sshRun(t.ssh, `cat ${shellQuoteArg(`${t.path}/${relPath}`)}`).stdout;
  }
  const full = join(t.path, relPath);
  const res = spawnSync("cat", [full], { encoding: "utf8" });
  return res.stdout ?? "";
}

/** Write a working-tree file (via ssh `cat >` for ssh repos). */
function writeWorkingTree(t: GitTarget, relPath: string, content: string): void {
  if (t.ssh) {
    const res = sshRun(t.ssh, `cat > ${shellQuoteArg(`${t.path}/${relPath}`)}`, content);
    if (!res.ok) throw new Error(res.stderr.trim() || "ssh write failed");
    return;
  }
  writeFileSync(join(t.path, relPath), content);
}

/** Ed25519 host fingerprint via ssh-keyscan + ssh-keygen (best effort). */
function sshFingerprint(host: string, port: number): string {
  if (!host) return "";
  try {
    const scan = spawnSync(
      "ssh-keyscan",
      ["-p", String(port), "-T", "5", "-t", "ed25519", host],
      { encoding: "utf8", timeout: 8000 },
    );
    if (scan.status !== 0 || !scan.stdout) return "";
    const keyLine = scan.stdout
      .split("\n")
      .find((l) => l.includes("ssh-ed25519") && l.trim().length > 0);
    if (!keyLine) return "";
    const keygen = spawnSync("ssh-keygen", ["-lf", "-"], {
      input: keyLine,
      encoding: "utf8",
    });
    if (keygen.status !== 0 || !keygen.stdout) return "";
    return keygen.stdout.trim();
  } catch {
    return "";
  }
}

function repoById(id: string): RepoRecord | { error: string } {
  const s = loadSettings();
  const r = findRepo(s, id);
  if (!r) return { error: `repo not found: ${id}` };
  return r;
}

function readBody(req: IncomingMessage): Promise<unknown> {
  const { promise, resolve, reject } = Promise.withResolvers<unknown>();
  const chunks: Buffer[] = [];
  req.on("data", (c: Buffer) => chunks.push(c));
  req.on("end", () => {
    const raw = Buffer.concat(chunks).toString("utf8");
    try {
      resolve(raw ? JSON.parse(raw) : {});
    } catch (e) {
      reject(e);
    }
  });
  req.on("error", reject);
  return promise;
}

// ── response helpers ────────────────────────────────────────────────────────

function send(res: ServerResponse, status: number, body: unknown): void {
  res.statusCode = status;
  res.setHeader("content-type", "application/json");
  res.end(JSON.stringify(body));
}


// ── IPC dispatch ────────────────────────────────────────────────────────────

interface InvokeArgs {
  cmd: string;
  args: Record<string, unknown>;
}

function jsonError(kind: string, message: string): unknown {
  return { kind, message };
}

async function dispatch(invoke: InvokeArgs): Promise<unknown> {
  const { cmd, args } = invoke;
  try {
    switch (cmd) {
      // ── repo registry ─────────────────────────────────────────────────
      case "list_repositories": {
        const s = loadSettings();
        return s.repositories;
      }
      case "register_repository": {
        const a = (args.args ?? args) as {
          project_path: string;
          ssh_user?: string;
          ssh_host?: string;
          ssh_key_path?: string;
          ssh_password?: string;
          ssh_port?: number;
        };
        // SSH repos are real over the dev server too — mirror the Rust app.
        const sshCfg: SshOptions | null = a.ssh_host
          ? {
              user: a.ssh_user ?? "",
              host: a.ssh_host,
              key_path: a.ssh_key_path ?? "",
              password: a.ssh_password ?? "",
              port: a.ssh_port ?? 22,
              timeout_secs: 5,
            }
          : null;
        const remoteGit = (cmd: string) => {
          if (!sshCfg) return git(a.project_path, cmd.split(" "));
          return sshRun(sshCfg, `git -C ${shellQuoteArg(a.project_path)} ${cmd}`);
        };
        const inside = remoteGit("rev-parse --is-inside-work-tree");
        if (!inside.ok) {
          return jsonError("git", "선택한 경로가 git 저장소가 아닙니다.");
        }
        const origin = remoteGit("remote get-url origin");
        const head = remoteGit("symbolic-ref --short HEAD");
        const rec: RepoRecord = {
          id: crypto.randomUUID(),
          path: a.project_path,
          display_name: a.project_path.split("/").pop() ?? a.project_path,
          default_branch: head.stdout.trim() || "main",
          working_branch: "",
          ssh_host: a.ssh_host ?? "",
          ssh_user: a.ssh_user ?? "",
          ssh_key_path: a.ssh_key_path ?? "",
          ssh_password: a.ssh_password ?? "",
          ed25519_fingerprint: "",
          ssh_port: a.ssh_port ?? 22,
          remote_url: origin.stdout.trim(),
          created_at: new Date().toISOString(),
        };
        const s = loadSettings();
        s.repositories.push(rec);
        saveSettings(s);
        return rec;
      }
      case "remove_repository": {
        const s = loadSettings();
        s.repositories = s.repositories.filter((r) => r.id !== args.id);
        saveSettings(s);
        return null;
      }
      case "update_repository": {
        const s = loadSettings();
        const r = findRepo(s, args.id as string);
        if (!r) return jsonError("repo_not_found", String(args.id));
        const patch = (args.patch ?? {}) as Record<string, unknown>;
        for (const [k, v] of Object.entries(patch)) {
          if (v === null || v === undefined) continue;
          // @ts-expect-error narrow union — server-side patch, keys are managed
          r[k] = v;
        }
        saveSettings(s);
        return r;
      }

      // ── merge center ──────────────────────────────────────────────────
      case "fetch_repo": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        return tgGit(targetOf(r), ["fetch", "--prune", "origin"]).stderr;
      }
      case "list_pending_branches": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        return pendingBranches(targetOf(r), "origin", args.base as string);
      }
      case "start_merge": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        return startMerge(targetOf(r), args.branchRef as string, args.base as string);
      }
      case "merge_state": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        return mergeState(targetOf(r));
      }
      case "conflict_detail": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        return conflictDetail(targetOf(r), args.path as string);
      }
      case "resolve_conflict": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        return resolveConflict(targetOf(r), args.path as string, args.resolution as Resolution);
      }
      case "abort_merge": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        tgGit(targetOf(r), ["merge", "--abort"]);
        return null;
      }
      case "complete_merge": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        const msg = (args.message as string | null | undefined) ?? null;
        const args2 = msg ? ["commit", "-m", msg] : ["commit", "--no-edit"];
        const out = tgGit(targetOf(r), args2);
        if (!out.ok) return jsonError("git", out.stderr.trim());
        return {
          ok: true,
          conflicted: false,
          conflicted_files: [],
          message: out.stdout.trim(),
        };
      }
      case "merge_auto_resolve": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        // ipc.ts sends camelCase; keep snake_case as a fallback
        const strategy =
          (args.binaryStrategy as string | null | undefined) ??
          (args.binary_strategy as string | null | undefined) ??
          "";
        if (strategy && strategy !== "ours" && strategy !== "theirs") {
          return jsonError("config", `알 수 없는 선택: ${strategy} (ours 또는 theirs)`);
        }
        try {
          return autoResolveMerge(targetOf(r), strategy === "ours" ? "ours" : "theirs");
        } catch (e) {
          return jsonError("git", (e as Error).message ?? String(e));
        }
      }
      case "merge_backup_list": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        return mergeBackupList(targetOf(r));
      }
      case "merge_backup_restore": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        try {
          // ipc.ts sends camelCase; keep snake_case as a fallback
          return mergeBackupRestore(
            targetOf(r),
            ((args.backupId as string | null | undefined) ??
              (args.backup_id as string | null | undefined)) as string,
          );
        } catch (e) {
          return jsonError("git", (e as Error).message ?? String(e));
        }
      }
      case "sync_branch": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        try {
          return syncToBase(targetOf(r), args.base as string);
        } catch (e) {
          return jsonError("git", (e as Error).message ?? String(e));
        }
      }

      // ── peer / team (dev-only stubs so the inbox UI can be exercised) ──
      case "peer_unread_count": {
        return 2;
      }
      case "peer_list_projects": {
        return [
          { id: "p1", display_name: "테스트 프로젝트", join_code: "TEST-0001", role: "admin" },
        ];
      }
      case "peer_list_members": {
        return [
          { device_id: null, email: "alice@example.com", name: "앨리스", role: "member", joined_at: null },
          { device_id: null, email: "bob@example.com", name: "밥", role: "admin", joined_at: null },
        ];
      }
      case "peer_repos_for_project": {
        // dev stub: 등록된 첫 저장소를 연결된 것으로 보여준다.
        const first = loadSettings().repositories[0];
        return first
          ? [{ repo_id: first.id, display_name: first.display_name, path: first.path }]
          : [];
      }
      case "peer_list_team_events": {
        const now = new Date().toISOString();
        return [
          {
            id: "e1",
            project_id: "p1",
            sender_device_name: "alice",
            event_kind: "branch_push",
            repo_name: "e2e-app",
            payload: JSON.stringify({ event: "branch_push", data: { branch: "feature", author: "alice" } }),
            received_at: now,
            read: false,
          },
          {
            id: "e2",
            project_id: "p1",
            sender_device_name: "bob",
            event_kind: "main_push",
            repo_name: loadSettings().repositories[0]?.display_name ?? "aos-git",
            payload: JSON.stringify({ event: "main_push", data: { branch: "main", author: "bob", message: "feature/x 브렌치 병합" } }),
            received_at: now,
            read: false,
          },
        ];
      }

      // ── branch / commit / status ─────────────────────────────────────
      case "list_branches": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        return listBranches(targetOf(r));
      }
      case "create_branch": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        const out = tgGit(targetOf(r), ["checkout", "-b", args.branch as string]);
        if (!out.ok) return jsonError("git", out.stderr.trim());
        return null;
      }
      case "checkout_branch": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        // 브랜치 목록에는 원격 트래킹 이름(origin/…)도 포함되므로 로컬 이름으로 정규화한다.
        // (그대로 쓰면 아래 폴백이 `origin/origin/…`을 만들며 실패한다 — Rust ops::checkout_branch와 동일.)
        const branch = (args.branch as string).replace(/^origin\//, "");
        const out = tgGit(targetOf(r), ["checkout", branch]);
        if (out.ok) return null;
        // 작업 트리가 더럽거나 미추적 파일이 겹치면 전환 자체가 불가능하다 — 한글로 친절하게 알려준다.
        const dirty = /would be overwritten|local changes|stash them|untracked working tree files/i.test(out.stderr);
        if (dirty) {
          return jsonError("git", "작업 트리에 커밋되지 않은 변경사항이 있어 브랜치를 전환할 수 없습니다. 변경사항을 커밋하거나 스태시한 뒤 다시 시도하세요.");
        }
        const track = tgGit(targetOf(r), ["checkout", "-b", branch, `origin/${branch}`]);
        if (track.ok) return null;
        const terr = track.stderr.trim();
        const msg = /would be overwritten|local changes|stash them|untracked working tree files/i.test(terr)
          ? "작업 트리에 커밋되지 않은 변경사항이 있어 브랜치를 전환할 수 없습니다. 변경사항을 커밋하거나 스태시한 뒤 다시 시도하세요."
          : `checkout failed: ${terr}`;
        return jsonError("git", msg);
      }
      case "list_commits": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        return listCommits(targetOf(r), args.branch as string, args.count as number);
      }
      case "status": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        return workingTreeStatus(targetOf(r));
      }
      case "add_files": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        const out = tgGit(targetOf(r), ["add", "--", ...((args.paths as string[]) ?? [])]);
        if (!out.ok) return jsonError("git", out.stderr.trim());
        return workingTreeStatus(targetOf(r));
      }
      case "commit": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        const flag = args.stageAll ? ["-a"] : [];
        const out = tgGit(targetOf(r), ["commit", ...flag, "-m", args.message as string]);
        if (!out.ok) return jsonError("git", out.stderr.trim());
        const sha = tgGit(targetOf(r), ["rev-parse", "HEAD"]).stdout.trim();
        return { ok: true, sha, message: out.stdout.trim() };
      }
      case "push":
      case "push_branch": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        const t = targetOf(r);
        let branch = args.branch as string | undefined;
        if (!branch) {
          branch = tgGit(t, ["rev-parse", "--abbrev-ref", "HEAD"]).stdout.trim();
        }
        if (!branch) return jsonError("git", "cannot determine branch");
        const url = tgGit(t, ["remote", "get-url", "origin"]).stdout.trim();
        const https = /^https?:\/\//.test(url);
        const cred = (args.credentials ?? null) as PushCredentialRecord | null;
        let out: GitResult;
        if (https && !cred) {
          // HTTPS + 자격증명 없음 → git 프롬프트 대신 모달을 띄우도록 auth_required를 알린다 (Rust와 동일).
          return {
            ok: false,
            pushed_sha: null,
            auth_required: true,
            message: "Git 호스트 로그인이 필요합니다. 푸시할 때 아이디/비밀번호를 입력하세요.",
          };
        }
        if (https && cred) {
          out = pushWithAskpass(t, branch, cred);
        } else {
          out = tgGit(t, ["push", "origin", `HEAD:${branch}`]);
        }
        if (out.ok && args.saveCredential && cred) {
          const s = loadSettings();
          s.push_credentials ??= {};
          s.push_credentials[r.id] = { username: cred.username, password: cred.password };
          saveSettings(s);
        }
        if (!out.ok && https && isAuthFailure(out.stderr)) {
          return {
            ok: false,
            pushed_sha: null,
            auth_required: true,
            message: "Git 호스트 로그인이 실패했거나 저장되지 않았습니다. 아이디/비밀번호를 다시 입력하세요.",
          };
        }
        return {
          ok: out.ok,
          pushed_sha: null,
          auth_required: false,
          message: (out.stdout.trim() || out.stderr.trim()) || "",
        };
      }
      case "pull": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        const branch = tgGit(targetOf(r), ["rev-parse", "--abbrev-ref", "HEAD"]).stdout.trim();
        const out = tgGit(targetOf(r), ["pull", "--ff-only", "origin", branch]);
        const conflicted = tgGit(targetOf(r), ["diff", "--name-only", "--diff-filter=U"]).stdout
          .split("\n").filter(Boolean);
        return { ok: out.ok, message: out.stdout + out.stderr, conflicted_files: conflicted };
      }
      case "diff": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        const a: string[] = [];
        if (args.staged) a.push("--staged");
        if (args.stat) a.push("--stat");
        if (args.pathspec) a.push("--", args.pathspec as string);
        return tgGit(targetOf(r), ["diff", ...a]).stdout;
      }
      case "stash": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        const action = args.action as string;
        const a = action === "save"
          ? ["stash", "push"]
          : action.startsWith("save:")
            ? ["stash", "push", "-m", action.slice(5)]
            : ["stash", action];
        tgGit(targetOf(r), a);
        return null;
      }
      case "stash_list": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        const out = tgGit(targetOf(r), ["stash", "list", "--format=%gd|%gs"]);
        if (!out.ok) return jsonError("git", out.stderr.trim());
        return out.stdout
          .split("\n")
          .map((line) => {
            const i = line.indexOf("|");
            return i < 0
              ? null
              : { index: line.slice(0, i).trim(), subject: line.slice(i + 1).trim() };
          })
          .filter((x): x is { index: string; subject: string } => x !== null && x.index !== "");
      }

      // ── AI config ─────────────────────────────────────────────────────
      case "get_ai_config": {
        return loadSettings().ai ?? {
          enabled: false,
          base_url: "",
          api_key: "",
          model: "",
        };
      }
      case "set_ai_config": {
        const s = loadSettings();
        s.ai = args.cfg as AiConfigRecord;
        saveSettings(s);
        return null;
      }
      case "ai_suggest_resolution": {
        // The bridge intentionally does not call any LLM — the dev preview
        // has no API keys. The frontend expects a real string back; we
        // answer with a trivial concat so the user can see the wiring
        // works without a network round-trip.
        const ours = args.ours as string;
        const theirs = args.theirs as string;
        return `${ours}\n${theirs}`;
      }

      // ── 로그인 계정 (로컬 레지스트리) ──────────────────────────────
      case "account_register": {
        const s = loadSettings();
        s.accounts ??= [];
        const name = (args.name as string).trim();
        const email = (args.email as string).trim().toLowerCase();
        const username = (args.username as string | undefined)?.trim().toLowerCase() ?? "";
        const password = (args.password as string | undefined) ?? "";
        if (!name || !email) return jsonError("bad_request", "이름과 이메일을 입력하세요.");
        if (!email.includes("@")) return jsonError("bad_request", "올바른 이메일 주소를 입력하세요.");
        let passwordHash: string | undefined;
        if (username && password) {
          if (!/^[a-z0-9._-]{1,32}$/.test(username)) {
            return jsonError("bad_request", "아이디는 영문/숫자/._- 만 사용하고 1~32자로 입력하세요.");
          }
          if (password.length < 4) return jsonError("bad_request", "비밀번호는 4자 이상 입력하세요.");
          passwordHash = hashPassword(username, password);
        }
        if (s.accounts.some((a) => a.email === email)) {
          return jsonError("bad_request", `${email}은(는) 이미 등록된 이메일입니다.`);
        }
        if (username && s.accounts.some((a) => (a.username ?? "").toLowerCase() === username)) {
          return jsonError("bad_request", `${username}은(는) 이미 사용 중인 아이디입니다.`);
        }
        const acc: AccountRecord = {
          id: randomUUID(),
          name,
          email,
          username: username || null,
          password_hash: passwordHash ?? null,
          created_at: new Date().toISOString(),
        };
        s.accounts.push(acc);
        s.active_account_id = acc.id;
        saveSettings(s);
        return acc;
      }
      case "account_login_by_password": {
        const s = loadSettings();
        const id = ((args.username as string) ?? "").trim().toLowerCase();
        const acc = (s.accounts ?? []).find(
          (a) =>
            (a.username ?? "").toLowerCase() === id ||
            (id.includes("@") && a.email === id),
        );
        const err = () => jsonError("bad_request", "아이디 또는 비밀번호가 올바르지 않습니다.");
        if (!acc) return err();
        const hash = acc.password_hash;
        if (!hash || hash !== hashPassword(acc.username ?? id, (args.password as string) ?? "")) {
          return err();
        }
        s.active_account_id = acc.id;
        saveSettings(s);
        return acc;
      }
      case "account_list": {
        return loadSettings().accounts ?? [];
      }
      case "account_delete": {
        const s = loadSettings();
        s.accounts ??= [];
        const id = args.id as string;
        s.accounts = s.accounts.filter((a) => a.id !== id);
        if (s.active_account_id === id) s.active_account_id = null;
        saveSettings(s);
        return null;
      }
      case "account_login": {
        const s = loadSettings();
        const acc = (s.accounts ?? []).find((a) => a.id === (args.id as string));
        if (!acc) return jsonError("bad_request", "계정을 찾을 수 없습니다.");
        s.active_account_id = acc.id;
        saveSettings(s);
        return acc;
      }
      case "account_logout": {
        const s = loadSettings();
        s.active_account_id = null;
        saveSettings(s);
        return null;
      }
      case "account_current": {
        const s = loadSettings();
        const id = s.active_account_id;
        if (!id) return null;
        return (s.accounts ?? []).find((a) => a.id === id) ?? null;
      }

      // ── 푸시 자격증명 (설정에서 저장 / 자동 입력) ────────────────────
      case "push_credentials_list": {
        return loadSettings().push_credentials ?? {};
      }
      case "push_credential_set": {
        const s = loadSettings();
        s.push_credentials ??= {};
        s.push_credentials[args.repoId as string] = {
          username: args.username as string,
          password: args.password as string,
        };
        saveSettings(s);
        return null;
      }
      case "push_credential_delete": {
        const s = loadSettings();
        s.push_credentials ??= {};
        delete s.push_credentials[args.repoId as string];
        saveSettings(s);
        return null;
      }

      // ── 프로젝트 설정 (.gpconfig) ────────────────────────────────────
      case "project_config_get": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        const t = targetOf(r);
        const raw = readWorkingTree(t, ".gpconfig").trim();
        if (!raw) return { exists: false, config: defaultProjectConfig() };
        try {
          return { exists: true, config: normalizeProjectConfig(JSON.parse(raw)) };
        } catch (e) {
          return jsonError("git", `.gpconfig 파싱 실패: ${(e as Error).message}`);
        }
      }
      case "project_config_set": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        const t = targetOf(r);
        let cfg = normalizeProjectConfig((args.config ?? {}) as ProjectConfigRecord);
        // 저장하는 사람도 구성원으로 자동 포함 (로그인 상태일 때).
        const me = (() => {
          const s = loadSettings();
          const id = s.active_account_id;
          if (!id) return null;
          return (s.accounts ?? []).find((a) => a.id === id) ?? null;
        })();
        if (me && !cfg.members.some((m) => m.email.toLowerCase() === me.email.toLowerCase())) {
          cfg.members.push({ id: me.id, name: me.name, email: me.email, role: "member" });
        }
        if (!cfg.default_base_branch) cfg.default_base_branch = r.default_branch || "main";
        writeWorkingTree(t, ".gpconfig", JSON.stringify(cfg, null, 2));
        let commit: { ok: boolean; message: string } | null = null;
        if (args.autoCommit) {
          const add = tgGit(t, ["add", "--", ".gpconfig"]);
          const co = tgGit(t, ["commit", "-m", "chore: update project config (.gpconfig)"]);
          if (add.ok && co.ok) {
            commit = { ok: true, message: co.stdout.trim() };
          } else if (add.ok && /nothing to commit|no changes added/.test(co.stderr)) {
            commit = { ok: true, message: "변경 사항 없음" };
          } else {
            return jsonError("git", `커밋 실패: ${co.stderr.trim() || add.stderr.trim()}`);
          }
        }
        return { config: cfg, commit };
      }
      case "project_config_commit": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        const t = targetOf(r);
        const add = tgGit(t, ["add", "--", ".gpconfig"]);
        if (!add.ok) return jsonError("git", add.stderr.trim());
        const co = tgGit(t, ["commit", "-m", "chore: update project config (.gpconfig)"]);
        if (!co.ok && !/nothing to commit|no changes added/.test(co.stderr)) {
          return jsonError("git", co.stderr.trim());
        }
        return { ok: true, message: co.stdout.trim() || "변경 사항 없음" };
      }

      // ── SSH / external tools (minimal mocks) ──────────────────────────
      case "get_ssh_profile": {
        return loadSettings().ssh_profile ?? {
          default_user: "",
          default_key_path: "",
          default_host: "",
          connect_timeout: "5",
          default_port: 22,
        };
      }
      case "set_ssh_profile": {
        const s = loadSettings();
        s.ssh_profile = (args.patch ?? {}) as Record<string, unknown>;
        saveSettings(s);
        return s.ssh_profile;
      }
      case "list_external_tools": {
        return loadSettings().external_tools ?? [];
      }
      case "set_external_tool": {
        return jsonError("not_implemented", "dev bridge does not yet mutate external_tools");
      }
      case "remove_external_tool":
      case "open_external_tool":
        return null;

      // Real SSH — the dev server runs on the same machine as the user, so
      // auth (agent / key / known_hosts) works exactly like in the app.
      case "test_ssh_connection": {
        const a = (args.args ?? args) as {
          host: string;
          user: string;
          port: number;
          key_path: string;
          password?: string;
          timeout_secs: number;
        };
        if (!a.host) return jsonError("bad_request", "호스트를 입력하세요.");
        const t0 = Date.now();
        const res = sshRun(
          {
            user: a.user ?? "",
            host: a.host,
            key_path: a.key_path ?? "",
            password: a.password ?? "",
            port: a.port ?? 22,
            timeout_secs: a.timeout_secs ?? 5,
          },
          "echo __GC_OK__; whoami; hostname; uname -sr",
        );
        const latency_ms = Date.now() - t0;
        if (res.ok && res.stdout.includes("__GC_OK__")) {
          const lines = res.stdout.split("\n");
          const i = lines.findIndex((l) => l.trim() === "__GC_OK__");
          const user = lines[i + 1]?.trim() ?? "";
          const hostname = lines[i + 2]?.trim() ?? "";
          const system =
            lines
              .slice(i + 3)
              .map((l) => l.trim())
              .filter(Boolean)
              .join(" ") || "";
          return {
            ok: true,
            latency_ms,
            user,
            hostname,
            system,
            fingerprint: sshFingerprint(a.host, a.port ?? 22),
            error: null,
          };
        }
        const err = res.stderr.trim();
        return {
          ok: false,
          latency_ms,
          user: "",
          hostname: "",
          system: "",
          fingerprint: "",
          error: err || "SSH connect failed",
        };
      }
      case "browse_ssh_dir": {
        const a = (args.args ?? args) as {
          target?: {
            ssh_user?: string;
            ssh_host?: string;
            ssh_key_path?: string;
            ssh_password?: string;
            ssh_port?: number;
          };
          path?: string;
        };
        const t = a.target ?? {};
        if (!t.ssh_host) return jsonError("bad_request", "SSH 호스트를 먼저 입력하세요.");
        const p = (a.path ?? "").trim();
        const quoted = p ? shellQuotePath(p) : "~";
        const res = sshRun(
          {
            user: t.ssh_user ?? "",
            host: t.ssh_host,
            key_path: t.ssh_key_path ?? "",
            password: t.ssh_password ?? "",
            port: t.ssh_port ?? 22,
            timeout_secs: 5,
          },
          `cd ${quoted} && pwd && ls -1FA && (git rev-parse --is-inside-work-tree 2>/dev/null || true)`,
        );
        if (!res.ok) {
          return jsonError(
            "ssh",
            `디렉터리를 열 수 없습니다: ${res.stderr.trim()}`,
          );
        }
        const lines = res.stdout
          .replace(/\r\n/g, "\n")
          .split("\n")
          .filter((l, idx, arr) => !(idx === arr.length - 1 && l === ""));
        if (lines.length === 0) {
          return jsonError("ssh", "원격에서 응답이 없습니다.");
        }
        const cwd = lines[0];
        let git_repo = false;
        if (lines[lines.length - 1].trim() === "true") {
          git_repo = true;
          lines.pop();
        }
        const entries = lines
          .slice(1)
          .filter((l) => l.length > 0)
          .map((l) => {
            const last = l[l.length - 1];
            const is_dir = last === "/";
            const is_symlink = last === "@";
            const marked =
              last === "/" || last === "*" || last === "@" || last === "|" || last === "=";
            return {
              name: marked ? l.slice(0, -1) : l,
              is_dir,
              is_symlink,
            };
          });
        return { path: cwd, git_repo, entries };
      }

      default:
        return jsonError(
          "not_implemented",
          `dev bridge has no handler for ${cmd}`,
        );
    }
  } catch (e) {
    return jsonError("internal", (e as Error).message ?? String(e));
  }
}

// ── merge helpers ──────────────────────────────────────────────────────────

interface ChangedPath { path: string; kind: string }
interface PendingBranch {
  name: string;
  short_name: string;
  sha: string;
  author: string;
  unix_time: number;
  subject: string;
  ahead: number;
  behind: number;
  changed_files: ChangedPath[];
}
interface MergeOutcome {
  ok: boolean;
  conflicted: boolean;
  conflicted_files: string[];
  message: string;
}
interface MergeState { in_progress: boolean; conflicted_files: string[] }
interface ConflictDetail {
  path: string;
  is_binary: boolean;
  too_large: boolean;
  base: string | null;
  ours: string;
  theirs: string;
  working: string;
}
type Resolution =
  | { type: "ours" }
  | { type: "theirs" }
  | { type: "manual"; content: string };

function pendingBranches(t: GitTarget, remote: string, base: string): PendingBranch[] {
  const fmt =
    "%(refname:short)%09%(objectname)%09%(authorname)%09%(committerdate:unix)%09%(subject)";
  const list = tgGit(t, ["for-each-ref", `refs/remotes/${remote}`, "--format", fmt]);
  if (!list.ok) return [];
  const baseRef = `${remote}/${base}`;
  const out: PendingBranch[] = [];
  for (const line of list.stdout.split("\n")) {
    if (!line) continue;
    const parts = line.split("\t");
    const [name, sha, author, unix, subject] = parts;
    if (!name || !sha || name === `${remote}/HEAD` || name === baseRef) continue;
    if (name.endsWith("/HEAD")) continue;
    const anc = tgGit(t, ["merge-base", "--is-ancestor", name, baseRef]);
    if (anc.ok) continue;
    const aheadBehind = tgGit(t, ["rev-list", "--left-right", "--count", `${baseRef}...${name}`]).stdout;
    let ahead = 0;
    let behind = 0;
    if (aheadBehind) {
      const [b, a] = aheadBehind.trim().split(/\s+/);
      ahead = Number(a ?? 0);
      behind = Number(b ?? 0);
    }
    const diff = tgGit(t, ["diff", "--name-status", `${baseRef}...${name}`]).stdout;
    const changed: ChangedPath[] = [];
    for (const cl of diff.split("\n")) {
      if (!cl) continue;
      const f = cl.split("\t");
      const kind = f[0] ?? "";
      const p = f[1] ?? "";
      if ((kind.startsWith("R") || kind.startsWith("C")) && f[2]) {
        changed.push({ path: p, kind });
      } else if (p) {
        changed.push({ path: p, kind });
      }
    }
    out.push({
      name,
      short_name: name.replace(new RegExp(`^${remote}/`), ""),
      sha,
      author: author ?? "",
      unix_time: Number(unix ?? 0),
      subject: subject ?? "",
      ahead,
      behind,
      changed_files: changed,
    });
  }
  out.sort((a, b) => b.unix_time - a.unix_time);
  return out;
}

function startMerge(t: GitTarget, branchRef: string, base: string): MergeOutcome {
  const dirty = tgGit(t, ["status", "--porcelain=v2", "--untracked-files=no"]).stdout.trim();
  if (dirty) {
    return {
      ok: false,
      conflicted: false,
      conflicted_files: [],
      message: "작업 트리에 커밋되지 않은 변경이 있습니다. 작업 탭에서 커밋하거나 stash하세요.",
    };
  }
  const head = tgGit(t, ["rev-parse", "--abbrev-ref", "HEAD"]).stdout.trim();
  if (head !== base) {
    tgGit(t, ["fetch", "origin", `${base}:${base}`]);
  }
  tgGit(t, ["fetch", "--prune", "origin"]);
  const co = tgGit(t, ["checkout", base]);
  if (!co.ok) {
    return { ok: false, conflicted: false, conflicted_files: [], message: co.stderr.trim() };
  }
  const m = tgGit(t, ["merge", "--no-ff", "--no-edit", branchRef]);
  if (m.ok) {
    return { ok: true, conflicted: false, conflicted_files: [], message: m.stdout.trim() };
  }
  const remaining = tgGit(t, ["diff", "--name-only", "--diff-filter=U"]).stdout
    .split("\n").filter(Boolean);
  if (m.stderr.includes("CONFLICT") || remaining.length > 0) {
    return {
      ok: false,
      conflicted: true,
      conflicted_files: remaining,
      message: m.stderr.trim(),
    };
  }
  tgGit(t, ["merge", "--abort"]);
  return { ok: false, conflicted: false, conflicted_files: [], message: m.stderr.trim() };
}

function mergeState(t: GitTarget): MergeState {
  const inProgress = tgGit(t, ["rev-parse", "-q", "--verify", "MERGE_HEAD"]).ok;
  const files = inProgress
    ? tgGit(t, ["diff", "--name-only", "--diff-filter=U"]).stdout.split("\n").filter(Boolean)
    : [];
  return { in_progress: inProgress, conflicted_files: files };
}

function conflictDetail(t: GitTarget, path: string): ConflictDetail {
  const ours = tgGit(t, ["show", `:2:${path}`]).stdout;
  const theirs = tgGit(t, ["show", `:3:${path}`]).stdout;
  const baseOut = tgGit(t, ["show", `:1:${path}`]);
  const base = baseOut.ok ? baseOut.stdout : null;
  const working = readWorkingTree(t, path);
  const isBinary = !isUtf8(ours) || !isUtf8(theirs);
  const tooLarge = ours.length > 1024 * 1024 || theirs.length > 1024 * 1024;
  return {
    path,
    is_binary: isBinary,
    too_large: tooLarge,
    base,
    ours: isBinary || tooLarge ? "" : ours,
    theirs: isBinary || tooLarge ? "" : theirs,
    working,
  };
}

function isUtf8(s: string): boolean {
  return Buffer.from(s, "utf8").toString("utf8") === s;
}



function resolveConflict(t: GitTarget, path: string, r: Resolution): string[] {
  if (r.type === "ours") tgGit(t, ["checkout", "--ours", "--", path]);
  else if (r.type === "theirs") tgGit(t, ["checkout", "--theirs", "--", path]);
  else writeWorkingTree(t, path, r.content);
  tgGit(t, ["add", "--", path]);
  return tgGit(t, ["diff", "--name-only", "--diff-filter=U"]).stdout.split("\n").filter(Boolean);
}

// ── auto-resolve / backup / sync helpers (mirror the Rust engine) ──────────

interface FileResolution {
  path: string;
  method: "ai" | "ours" | "theirs";
  note: string | null;
}
interface AutoResolveReport {
  resolved: FileResolution[];
  remaining: string[];
  committed: boolean;
  backup_id: string | null;
  message: string;
}
interface BackupEntry {
  id: string;
  created_at: string;
  files: string[];
}

function backupRoot(t: GitTarget): string {
  const base = process.env.GC_BACKUP_DIR ?? join(homedir(), ".config", "com.gitcompanion.app", "backups");
  const slug = t.path.replace(/[^A-Za-z0-9._-]/g, "_");
  return join(base, slug);
}

function mergeHeadBranch(t: GitTarget): string {
  // rev-parse --abbrev-ref MERGE_HEAD just echoes the pseudo-ref name, so
  // resolve the tip sha to an actual branch instead (parity with Rust auto.rs).
  const sha = tgGit(t, ["rev-parse", "MERGE_HEAD"]);
  if (sha.ok) {
    const out = tgGit(t, ["for-each-ref", "--points-at", sha.stdout.trim(), "--format=%(refname:short)"]);
    if (out.ok) {
      let remote = "";
      for (const raw of out.stdout.split("\n")) {
        const line = raw.trim();
        if (!line) continue;
        // prefer the remote branch (our merges target origin/*); else fall
        // back to the first ref (e.g. the local branch).
        if (line.startsWith("origin/")) { return line; }
        if (!remote) remote = line;
      }
      if (remote) return remote;
    }
  }
  return "(병합 대상)";
}

function autoResolveMerge(t: GitTarget, binaryStrategy: "ours" | "theirs"): AutoResolveReport {
  const st = mergeState(t);
  const remaining0 = st.in_progress
    ? tgGit(t, ["diff", "--name-only", "--diff-filter=U"]).stdout.split("\n").filter(Boolean)
    : [];
  if (remaining0.length === 0) {
    if (st.in_progress) {
      return {
        resolved: [],
        remaining: [],
        committed: false,
        backup_id: null,
        message: "해결할 충돌 파일이 없습니다. ‘병합 완료’를 눌러 병합을 마무리하세요.",
      };
    }
    throw new Error("진행 중인 병합이 없습니다. 병합 센터에서 먼저 병합을 시작하세요.");
  }

  // Back up conflicted working files before touching anything.
  const backupDir = join(backupRoot(t), `${Date.now()}_${randomUUID().slice(0, 8)}`);
  const backedUp: string[] = [];
  for (const path of remaining0) {
    const content = readWorkingTree(t, path);
    if (!content) continue;
    mkdirSync(join(backupDir, dirname(path)), { recursive: true });
    writeFileSync(join(backupDir, path), content);
    backedUp.push(path);
  }
  const backupId = backedUp.length > 0 ? backupDir.split(/[\\/]/).pop() ?? null : null;

  // Resolve file by file; failures just leave the file in `remaining`.
  const resolved: FileResolution[] = [];
  let remaining = [...remaining0];
  for (const path of remaining0) {
    const detail = conflictDetail(t, path);
    let method: "ai" | "ours" | "theirs";
    let note: string | null = null;
    if (detail.is_binary || detail.too_large) {
      method = binaryStrategy;
      note = "바이너리/대용량 파일 — 내용 확인 후 병합 탭에서 검토하세요.";
    } else {
      method = detail.base === detail.theirs ? "ours" : "theirs";
      note = "AI 결과를 사용할 수 없어 규칙 기반으로 선택했습니다.";
    }
    tgGit(t, ["checkout", `--${method}`, "--", path]);
    tgGit(t, ["add", "--", path]);
    resolved.push({ path, method, note });
  }
  remaining = tgGit(t, ["diff", "--name-only", "--diff-filter=U"]).stdout.split("\n").filter(Boolean);

  if (remaining.length === 0 && st.in_progress) {
    const branch = mergeHeadBranch(t);
    const commit = tgGit(t, ["commit", "-m", `AI 자동 병합: ${branch}`]);
    if (commit.ok) {
      return {
        resolved,
        remaining,
        committed: true,
        backup_id: backupId,
        message: `충돌 ${resolved.length}개를 자동 해결하고 ‘AI 자동 병합: ${branch}’로 커밋했습니다.`,
      };
    }
    return {
      resolved,
      remaining,
      committed: false,
      backup_id: backupId,
      message: `모든 충돌은 해결됐지만 커밋에 실패했습니다: ${commit.stderr.trim()}. 충돌 전 상태는 백업에 보존되어 있으니 병합 센터의 ‘병합 완료’로 마무리하세요.`,
    };
  }
  return {
    resolved,
    remaining,
    committed: false,
    backup_id: backupId,
    message: `충돌 ${remaining0.length}개 중 ${remaining.length}개를 해결하지 못했습니다. 남은 파일은 병합 센터에서 처리하세요.`,
  };
}

function mergeBackupList(t: GitTarget): BackupEntry[] {
  const root = backupRoot(t);
  if (!existsSync(root)) return [];
  return readdirSync(root)
    .filter((name) => {
      try {
        return statSync(join(root, name)).isDirectory();
      } catch {
        return false;
      }
    })
    .map((id) => {
      const files: string[] = [];
      const walk = (dir: string, rel: string) => {
        for (const name of readdirSync(dir)) {
          const p = join(dir, name);
          const r = rel ? `${rel}/${name}` : name;
          if (statSync(p).isDirectory()) walk(p, r);
          else files.push(r);
        }
      };
      walk(join(root, id), "");
      const millis = Number(id.split("_")[0]);
      const created_at =
        Number.isFinite(millis) && millis > 0 ? new Date(millis).toISOString() : id;
      return { id, created_at, files: files.sort() };
    })
    .sort((a, b) => b.created_at.localeCompare(a.created_at));
}

function mergeBackupRestore(t: GitTarget, backupId: string): number {
  const dir = join(backupRoot(t), backupId);
  if (!existsSync(dir)) throw new Error(`백업을 찾을 수 없습니다: ${backupId}`);
  const files: string[] = [];
  const walk = (d: string, rel: string) => {
    for (const name of readdirSync(d)) {
      const p = join(d, name);
      const r = rel ? `${rel}/${name}` : name;
      if (statSync(p).isDirectory()) walk(p, r);
      else files.push(r);
    }
  };
  walk(dir, "");
  for (const f of files) {
    writeWorkingTree(t, f, readFileSync(join(dir, f), "utf8"));
  }
  return files.length;
}

function syncToBase(t: GitTarget, base: string): MergeOutcome & { conflicted: boolean; conflicted_files: string[] } {
  if (mergeState(t).in_progress) {
    throw new Error("이미 진행 중인 병합이 있습니다. 병합 탭에서 먼저 마무리하세요.");
  }
  tgGit(t, ["fetch", "--prune", "origin"]);
  tgGit(t, ["fetch", "origin", `${base}:${base}`]);
  const head = tgGit(t, ["rev-parse", "--abbrev-ref", "HEAD"]).stdout.trim();
  const args: string[] =
    head === base
      ? ["merge", "--no-edit", `origin/${base}`]
      : ["merge", "--no-ff", "--no-edit", `origin/${base}`];
  const m = tgGit(t, args);
  if (m.ok) {
    return { ok: true, conflicted: false, conflicted_files: [], message: m.stdout.trim() };
  }
  const remaining = tgGit(t, ["diff", "--name-only", "--diff-filter=U"]).stdout.split("\n").filter(Boolean);
  if (m.stderr.includes("CONFLICT") || remaining.length > 0) {
    return { ok: false, conflicted: true, conflicted_files: remaining, message: m.stderr.trim() };
  }
  tgGit(t, ["merge", "--abort"]);
  return { ok: false, conflicted: false, conflicted_files: [], message: m.stderr.trim() };
}

// ── branch / commit / status helpers ────────────────────────────────────────

interface BranchSummary { name: string; is_remote: boolean; upstream: string | null }
function listBranches(t: GitTarget): BranchSummary[] {
  const local = tgGit(t, ["for-each-ref", "refs/heads", "--format=%(refname:short)"]).stdout
    .split("\n").filter(Boolean);
  const remote = tgGit(t, ["for-each-ref", "refs/remotes", "--format=%(refname:short)"]).stdout
    .split("\n").filter(Boolean);
  return [
    ...local.map((n) => ({ name: n, is_remote: false, upstream: `origin/${n}` })),
    ...remote.map((n) => ({ name: n, is_remote: true, upstream: null })),
  ];
}

interface CommitSummary {
  sha: string;
  message: string;
  author: string;
  date: string;
  parents: string[];
}
function listCommits(t: GitTarget, branch: string, count: number): CommitSummary[] {
  const fmt = "%H%x00%P%x00%an%x00%aI%x00%s";
  const out = tgGit(t, ["log", branch, `-n${count}`, `--pretty=${fmt}`]);
  return out.stdout.split("\n").filter(Boolean).map((line) => {
    const [sha, parents, author, date, ...rest] = line.split("\x00");
    return {
      sha: sha ?? "",
      parents: (parents ?? "").split(" ").filter(Boolean),
      author: author ?? "",
      date: date ?? "",
      message: rest.join("\x00"),
    };
  });
}

interface WorkingTreeStatus {
  branch: string | null;
  upstream: string | null;
  ahead: number;
  behind: number;
  files: Array<{ kind: string; path: string; staged: boolean; unstaged: boolean }>;
}
function workingTreeStatus(t: GitTarget): WorkingTreeStatus {
  const branch = tgGit(t, ["rev-parse", "--abbrev-ref", "HEAD"]).stdout.trim();
  const upstream = tgGit(t, ["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"]).stdout.trim();
  const ab = upstream
    ? tgGit(t, ["rev-list", "--left-right", "--count", `${upstream}...HEAD`]).stdout.trim()
    : "";
  const [behind, ahead] = ab ? ab.split(/\s+/).map(Number) : [0, 0];
  const status = tgGit(t, ["status", "--porcelain=v2"]).stdout;
  const files: WorkingTreeStatus["files"] = [];
  for (const line of status.split("\n")) {
    if (!line) continue;
    if (line.startsWith("1 ") || line.startsWith("2 ")) {
      const fields = line.split(" ");
      const xy = fields[1] ?? "";
      const path = fields.slice(8).join(" ");
      const staged = xy[0] !== " " && xy[0] !== "?";
      const unstaged = xy[1] !== " ";
      const kind =
        xy[0] === "A" ? "added" :
        xy[0] === "M" || xy[1] === "M" ? "modified" :
        xy[0] === "D" || xy[1] === "D" ? "deleted" :
        xy[0] === "R" ? "renamed" :
        xy[0] === "C" ? "copied" :
        xy[0] === "U" || xy[1] === "U" ? "conflicted" : "modified";
      files.push({ kind, path, staged, unstaged });
    } else if (line.startsWith("? ")) {
      files.push({ kind: "untracked", path: line.slice(2), staged: false, unstaged: false });
    }
  }
  return { branch: branch || null, upstream: upstream || null, ahead: ahead ?? 0, behind: behind ?? 0, files };
}

// ── plugin ──────────────────────────────────────────────────────────────────

export function gitBridgePlugin(): Plugin {
  return {
    name: "git-companion-bridge",
    configureServer(server: ViteDevServer) {
      server.middlewares.use("/__gc/invoke", async (req, res, next) => {
        if (req.method !== "POST") {
          next();
          return;
        }
        try {
          const body = (await readBody(req)) as InvokeArgs;
          const result = await dispatch(body);
          send(res, 200, result);
        } catch (e) {
          send(res, 400, jsonError("bad_request", (e as Error).message ?? String(e)));
        }
      });
    },
  };
}
