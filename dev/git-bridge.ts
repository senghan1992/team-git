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
//   set_ai_config, ai_default_prompt, ai_suggest_resolution,
//   get_ssh_profile, set_ssh_profile,
//   test_ssh_connection, browse_ssh_dir,
//   account_register, account_list, account_delete, account_login, account_logout,
//   account_current, push_credentials_list, push_credential_set, push_credential_delete,
//   project_config_get, project_config_set, project_config_commit.
//
// Peer / team calls (`peer_*`) talk to the same FastAPI backend the desktop
// app uses (`backend/`), with the same device token file, so the inbox you
// see in the browser is the real one. When no backend URL is configured they
// degrade to "no notifications" exactly like the Rust commands.
//
// Repo-bound commands run on a small worker-thread pool (see BridgePool
// below) so one slow SSH repository cannot freeze every other card.

import type { Plugin, ViteDevServer } from "vite";
import { randomUUID } from "node:crypto";
import { spawnSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, readFileSync, readdirSync, realpathSync, renameSync, statSync, unlinkSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { Worker } from "node:worker_threads";
import type { IncomingMessage, ServerResponse } from "node:http";
import { normalizeRemoteUrl } from "../ui/lib/repoMatch";
import { mergeTimeline } from "./bridge-timeline";

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
  /** 병합 관리자가 미리 저장하는 해결 지침. 빈 값이면 기본 프롬프트. */
  system_prompt?: string;
  /** 충돌이 나면 버튼을 누르지 않고 곧바로 자동 해결한다. */
  auto_resolve?: boolean;
  /** 자동 해결로 만든 병합 커밋을 확인 없이 곧바로 push한다. */
  auto_push?: boolean;
  /** 바이너리·대용량 파일 처리: "theirs" | "ours" */
  binary_strategy?: string;
}

/** Rust `ai::DEFAULT_SYSTEM_PROMPT`와 동일한 문구 — 미리보기에서 같은 값을 보여 준다. */
const DEFAULT_AI_PROMPT =
  "git 병합에 실패한 상태입니다. ours(현재 브랜치)와 theirs(병합 대상 브랜치) 양쪽에서 수정한 기능들이 서로 영향받지 않도록 모두 반영하는 최종 코드를 제안하세요. 기능이 깨지지 않게 import/선언 누락, 중복 정의, 끊긴 호출부가 없어야 합니다. 판단 근거 주석 없이 코드만 반환하세요. 직접적인 결합이 불가능하면 양쪽 의도를 모두 만족하는 대안 코드를 제시하세요.";

/** 서버 `/auth` 의 UserPublic 과 같은 모양 — 비밀번호는 오지 않는다. */
interface AccountRecord {
  id: string;
  name: string;
  email: string;
  username: string;
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
  ssh_profile?: Record<string, unknown>;
  peer?: Record<string, unknown>;
  ai?: AiConfigRecord;
  /** Rust 와 같은 자리 — 지금 로그인한 사람과 토큰. */
  session?: { user: AccountRecord; token: string } | null;
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
    const empty: AppSettings = { schema_version: 9, repositories: [] };
    mkdirSync(join(homedir(), ".config", APP_DIR), { recursive: true });
    writeFileSync(p, JSON.stringify(empty, null, 2));
    return empty;
  }
  let s: AppSettings;
  try {
    s = JSON.parse(readFileSync(p, "utf8")) as AppSettings;
  } catch {
    s = { schema_version: 9, repositories: [] };
  }
  return s;
}

function saveSettings(s: AppSettings): void {
  writeFileSync(configPath(), JSON.stringify(s, null, 2));
}

// ── 로그인 (팀 서버의 /auth 를 그대로 호출) ───────────────────────────
//
// 예전에는 여기서 SHA-256 해시와 시드 계정(test/test)을 흉내 냈다. 계정이
// 서버로 옮겨간 뒤에는 흉내를 낼 이유가 없다 — 미리보기에서 가입한 계정이
// 데스크톱 앱에서도 그대로 로그인되어야 한다.

/** 설정에 저장된 팀 서버 주소. 없으면 사용자에게 알려 줄 메시지와 함께 실패. */
function backendUrl(): string {
  const url = String((loadSettings().peer as { backend_url?: string } | undefined)?.backend_url ?? "").trim();
  if (!url) {
    throw new Error(
      "팀 서버 주소가 설정되지 않았습니다. 로그인 화면의 ‘서버 주소’에 입력하세요 (예: http://127.0.0.1:8000).",
    );
  }
  return url.replace(/\/+$/, "");
}

/**
 * `/auth/*` 호출. 실패하면 FastAPI 의 `detail` 문구를 그대로 올린다 —
 * 상태 코드만 보여 주면 어느 항목이 문제인지 알 수 없다.
 */
async function authProxy(
  method: "GET" | "POST" | "PATCH" | "DELETE",
  path: string,
  body?: unknown,
  token?: string,
): Promise<unknown> {
  const base = backendUrl();
  const headers: Record<string, string> = {};
  if (body !== undefined) headers["content-type"] = "application/json";
  if (token) headers.authorization = `Bearer ${token}`;
  let resp: Response;
  try {
    resp = await fetch(`${base}${path}`, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  } catch (e) {
    throw new Error(
      `팀 서버에 연결할 수 없습니다 (${base}). 서버가 실행 중인지 확인하세요: cd backend && uvicorn app.main:app`,
    );
  }
  if (!resp.ok) {
    let detail = "";
    try {
      const j = (await resp.json()) as { detail?: unknown };
      if (typeof j.detail === "string") detail = j.detail;
      else if (Array.isArray(j.detail)) {
        detail = String((j.detail[0] as { msg?: string } | undefined)?.msg ?? "");
      }
    } catch {
      /* 본문이 JSON 이 아니면 상태 코드만 쓴다 */
    }
    throw new Error(detail || `요청 실패 (${resp.status})`);
  }
  if (resp.status === 204) return null;
  return resp.json();
}

/** register/login 응답을 설정 파일의 세션으로 저장한다. */
function saveSessionFromAuth(result: unknown): AccountRecord {
  const auth = result as { user: AccountRecord; token: string };
  const s = loadSettings();
  s.session = { user: auth.user, token: auth.token };
  saveSettings(s);
  return auth.user;
}

function requireSessionToken(s: AppSettings): string {
  const token = s.session?.token;
  if (!token) throw new Error("로그인이 필요합니다.");
  return token;
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
    const remote = `GIT_ASKPASS='${rel.replace(/'/g, `'\\''`)}' GIT_TERMINAL_PROMPT='0' git -C ${shellQuoteArg(t.path)} -c core.quotepath=off push -u origin 'HEAD:${branch.replace(/'/g, `'\\''`)}'`;
    const res = sshRun(t.ssh, remote);
    sshRun(t.ssh, `rm -f '${rel.replace(/'/g, `'\\''`)}'`);
    return { ok: res.ok, stdout: res.stdout, stderr: res.stderr };
  }
  const path = join(tmpdir(), `gc-askpass-${randomUUID()}.sh`);
  writeFileSync(path, script);
  try {
    chmodSync(path, 0o700);
  } catch { /* windows only */ }
  const res = gitWithEnv(t.path, ["push", "-u", "origin", `HEAD:${branch}`], {
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

// `Promise.withResolvers` 는 Node 22+ 에서만 쓸 수 있다. README가 요구하는
// Node 20 에서도 미리보기가 동작해야 하므로 평범한 Promise 로 쓴다.
function readBody(req: IncomingMessage): Promise<unknown> {
  return new Promise<unknown>((resolve, reject) => {
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
  });
}

// ── response helpers ────────────────────────────────────────────────────────

function send(res: ServerResponse, status: number, body: unknown): void {
  res.statusCode = status;
  res.setHeader("content-type", "application/json");
  res.end(JSON.stringify(body));
}


/**
 * 현재 브랜치 이름.
 *
 * `rev-parse --abbrev-ref HEAD` 는 **커밋이 하나도 없는 저장소**에서
 * fatal 과 함께 "HEAD" 를 내놓는다. 방금 `git init` 한 사람에게 브랜치가
 * "HEAD" 로 보이는 셈이다. `symbolic-ref` 는 그 상태에서도 실제 이름(main)을
 * 준다. (Rust 는 `status --porcelain=v2 --branch` 를 써서 원래 옳았다.)
 */
function currentBranch(t: GitTarget): string {
  const sym = tgGit(t, ["symbolic-ref", "--short", "HEAD"]);
  if (sym.ok && sym.stdout.trim()) return sym.stdout.trim();
  const rp = tgGit(t, ["rev-parse", "--abbrev-ref", "HEAD"]);
  return rp.ok ? rp.stdout.trim() : "";
}

// ── 오류 문구 / 경로 (Rust 쪽과 같은 규칙) ────────────────────────────────
//
// 처음 git 을 쓰는 사람이 실제로 마주치는 실패들이다. 여기서 흉내를 내지
// 않으면 미리보기에서는 친절한 문구가, 실제 앱에서는 영어 fatal 이 뜨거나
// (혹은 그 반대로) 어긋난다. Rust: `git/ops.rs`, `git/mod.rs`.

/** `~`, `~/…` 를 홈으로 펼친다. */
function expandTilde(input: string): string {
  const t = input.trim();
  if (t === "~") return homedir();
  if (t.startsWith("~/")) return join(homedir(), t.slice(2));
  return t;
}

/** 커밋 실패 이유를 사람 말로. git 은 "nothing to commit" 을 stdout 에 쓴다. */
function explainCommitFailure(stdout: string, stderr: string): string {
  const all = `${stdout}\n${stderr}`;
  if (
    all.includes("nothing to commit") ||
    all.includes("no changes added to commit") ||
    all.includes("nothing added to commit")
  ) {
    return "커밋할 변경이 없습니다. 파일을 수정한 뒤 다시 커밋하세요.";
  }
  if (all.includes("empty commit message") || all.includes("Aborting commit due to empty")) {
    return "커밋 메시지를 입력하세요.";
  }
  if (all.includes("Please tell me who you are") || all.includes("unable to auto-detect email")) {
    return 'git 사용자 정보가 없어 커밋할 수 없습니다. 터미널에서 한 번 설정하세요:\n  git config --global user.name "이름"\n  git config --global user.email "메일@example.com"';
  }
  if (all.includes("index.lock")) {
    return "다른 git 작업이 진행 중입니다(.git/index.lock). 잠시 후 다시 시도하세요.";
  }
  if (all.includes("unmerged") || all.includes("Unmerged paths")) {
    return "해결하지 않은 충돌이 남아 있습니다. 병합 탭에서 먼저 마무리하세요.";
  }
  const raw = stderr.trim() || stdout.trim();
  return raw || "알 수 없는 이유로 커밋에 실패했습니다.";
}

/** git 의 영어 오류를 사람 말로. 구체적인 원인을 먼저 검사한다. */
function friendlyGitError(stderr: string): string {
  const t = stderr.trim();
  if (t.includes("does not appear to be a git repository") || t.includes("No such remote")) {
    return "이 저장소에는 원격(origin)이 없어서 푸시할 곳이 없습니다.\n터미널에서 원격을 한 번 등록하세요:\n  git remote add origin <저장소 주소>";
  }
  if (t.includes("src refspec") && t.includes("does not match any")) {
    return "푸시할 커밋이 없습니다. 먼저 커밋한 뒤 다시 시도하세요.";
  }
  if (t.includes("has no upstream branch") || t.includes("no upstream configured")) {
    return "이 브랜치는 아직 원격에 없습니다. 앱이 자동으로 만들어 주니 다시 시도하세요.";
  }
  if (t.includes("Repository not found") || t.includes("repository does not exist")) {
    return "원격에서 저장소를 찾을 수 없습니다. 저장소 주소와 접근 권한을 확인하세요.";
  }
  if (t.includes("non-fast-forward") || t.includes("updates were rejected")) {
    return "푸시 거부됨: 원격 브랜치가 로컬보다 앞서 있습니다. 먼저 ‘동기화’로 최신 내용을 받은 뒤 다시 푸시하세요.";
  }
  if (t.includes("failed to push some refs")) {
    return "푸시 실패: 원격에 새 변경이 있습니다. 먼저 ‘동기화’로 받은 뒤 다시 푸시하세요.";
  }
  if (
    t.includes("Could not resolve host") ||
    t.includes("Connection refused") ||
    t.includes("Connection timed out") ||
    t.includes("network")
  ) {
    return "네트워크에 연결할 수 없습니다. 인터넷 연결과 저장소 주소를 확인하세요.";
  }
  if (
    t.includes("Permission denied") ||
    t.includes("permission denied") ||
    t.includes("Authentication failed") ||
    t.includes("authentication") ||
    t.includes("auth")
  ) {
    return "접근 권한이 없습니다. SSH 키가 등록되어 있는지, 또는 아이디/비밀번호가 맞는지 확인하세요.";
  }
  if (t.includes("Host key verification failed")) {
    return "서버의 SSH 호스트 키를 확인할 수 없습니다. 터미널에서 한 번 접속해 호스트를 신뢰 목록에 추가하세요.";
  }
  return t || "알 수 없는 이유로 실패했습니다.";
}

/** 로컬 경로 검증 — 실패 이유마다 다른 문구. */
function checkLocalRepoPath(input: string): { path: string } | { error: string } {
  const p = expandTilde(input);
  if (!p) return { error: "저장소 폴더 경로를 입력하세요." };
  if (!existsSync(p)) {
    return {
      error: `그 경로에 폴더가 없습니다: ${p}\n경로를 다시 확인하세요. 전체 경로(예: /home/이름/projects/my-app)로 입력해야 합니다.`,
    };
  }
  if (!statSync(p).isDirectory()) {
    return { error: `폴더가 아니라 파일입니다: ${p}\n저장소 폴더 자체를 고르세요.` };
  }
  if (!existsSync(join(p, ".git"))) {
    // 하위 폴더를 고른 흔한 실수를 잡아 준다.
    let hint =
      "\ngit clone 으로 받은 폴더를 고르거나, 이 폴더를 저장소로 만들려면 ‘git 저장소로 만들기’를 쓰세요.";
    let up = p;
    for (let i = 0; i < 8; i++) {
      const parent = dirname(up);
      if (parent === up) break;
      up = parent;
      if (existsSync(join(up, ".git"))) {
        hint = `\n혹시 이 폴더를 찾으셨나요? ${up}`;
        break;
      }
    }
    return { error: `이 폴더는 git 저장소가 아닙니다 (.git 이 없습니다): ${p}${hint}` };
  }
  return { path: p };
}

// ── 팀 서버(peer) — Rust `peer.rs` / `commands/peer.rs` 와 같은 서버, 같은 파일 ──
//
// 예전에는 여기가 고정 데이터였다: 읽지 않은 알림은 언제나 2건, 읽음 처리는
// 아무 일도 하지 않았고, 같은 가짜 알림 두 건이 매번 "새로" 도착했다.
// 미리보기로 알림을 판단하던 사람에게는 "읽음 처리가 안 된다"로 보였다.
// 지금은 데스크톱 앱과 같은 서버에 같은 기기 토큰(`peer_token`)으로 붙고,
// 받은 알림은 `inbox.dev.json`(앱은 SQLite `inbox.db`)에 저장해 읽음 상태가
// 남는다. 저장소↔프로젝트 연결(`repo_projects.json`)도 앱과 같은 파일이다.

const PEER_TOKEN_FILE = "peer_token";
const REPO_PROJECTS_FILE = "repo_projects.json";
const DEV_INBOX_FILE = "inbox.dev.json";

function appConfigDir(): string {
  return dirname(configPath());
}

interface PeerSettings {
  backend_url?: string;
  device_token?: string;
  device_id?: string;
  device_name?: string;
}

function peerSettings(s: AppSettings): PeerSettings {
  return (s.peer ?? {}) as PeerSettings;
}

/** Rust `load_or_create_token` — 기기 토큰 파일을 읽거나 새로 만든다. */
function peerToken(): string {
  const p = join(appConfigDir(), PEER_TOKEN_FILE);
  if (existsSync(p)) {
    const t = readFileSync(p, "utf8").trim();
    if (t) return t;
  }
  const token = randomUUID() + randomUUID();
  mkdirSync(appConfigDir(), { recursive: true });
  writeFileSync(p, token);
  return token;
}

async function peerFetch<T>(
  backend: string,
  token: string,
  method: "GET" | "POST" | "PUT" | "DELETE",
  path: string,
  body?: unknown,
): Promise<{ status: number; json: T | null }> {
  const headers: Record<string, string> = { authorization: `Bearer ${token}` };
  if (body !== undefined) headers["content-type"] = "application/json";
  let resp: Response;
  try {
    resp = await fetch(`${backend.replace(/\/+$/, "")}${path}`, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(10_000),
    });
  } catch {
    throw new Error(
      `팀 서버에 연결할 수 없습니다 (${backend}). 서버가 실행 중인지 확인하세요: cd backend && uvicorn app.main:app`,
    );
  }
  let json: T | null = null;
  try {
    json = (await resp.json()) as T;
  } catch {
    json = null;
  }
  return { status: resp.status, json };
}

/** 2xx 가 아니면 FastAPI 의 `detail` 문구로 Error 를 던진다. */
function peerOk<T>(r: { status: number; json: T | null }, what: string): T {
  if (r.status >= 200 && r.status < 300) return r.json as T;
  const detail = (r.json as { detail?: unknown } | null)?.detail;
  throw new Error(typeof detail === "string" ? `${what} 실패: ${detail}` : `${what} 실패 (${r.status})`);
}

function deviceNameFor(s: AppSettings): string {
  return s.session?.user.name?.trim() || process.env.HOSTNAME || "Git Companion";
}

interface DeviceInfo {
  id: string;
  name: string;
  user_id: string;
}

/** 서버에 이 기기를 (같은 토큰으로, 멱등) 등록하고 설정에 남긴다. */
async function registerDevice(backend: string, token: string, name: string): Promise<DeviceInfo> {
  const info = peerOk(
    await peerFetch<DeviceInfo>(backend, token, "POST", "/devices/register", { name }),
    "기기 등록",
  );
  const s = loadSettings();
  s.peer = { ...(s.peer ?? {}), backend_url: backend, device_token: token, device_id: info.id, device_name: name };
  saveSettings(s);
  return info;
}

/** Rust `ensure_device_registered` — 서버가 이 기기를 모르면 등록부터 한다. */
async function ensureDeviceRegistered(): Promise<{ backend: string; token: string; deviceId: string }> {
  const s = loadSettings();
  const peer = peerSettings(s);
  const backend = String(peer.backend_url ?? "").trim() || "http://127.0.0.1:8000";
  const token = peerToken();
  if (peer.device_id) return { backend, token, deviceId: String(peer.device_id) };
  const info = await registerDevice(backend, token, deviceNameFor(s));
  return { backend, token, deviceId: info.id };
}

interface InboxRow {
  id: string;
  project_id: string;
  sender_device_name: string;
  event_kind: string;
  repo_name: string;
  payload: string;
  received_at: string;
  read: boolean;
}

function inboxPath(): string {
  return join(appConfigDir(), DEV_INBOX_FILE);
}

function loadInbox(): InboxRow[] {
  try {
    const parsed = JSON.parse(readFileSync(inboxPath(), "utf8")) as unknown;
    if (Array.isArray(parsed)) return parsed as InboxRow[];
  } catch {
    /* 없거나 깨졌으면 빈 수신함 */
  }
  return [];
}

function saveInbox(rows: InboxRow[]): void {
  const p = inboxPath();
  mkdirSync(dirname(p), { recursive: true });
  const tmp = `${p}.tmp`;
  writeFileSync(tmp, JSON.stringify(rows, null, 2));
  renameSync(tmp, p);
}

interface EventDetail {
  id?: string;
  project_id?: string;
  sender_device_id?: string;
  sender_device_name?: string | null;
  event_kind?: string;
  repo_name?: string;
  payload?: string;
}

/** Rust `peer_poll_now` — 서버에 쌓인 이벤트를 전부 끌어와 수신함에 저장한다. */
async function pollTeamEventsOnce(): Promise<number> {
  const peer = peerSettings(loadSettings());
  // 팀 서버를 설정한 적이 없으면 조용히 넘어간다 (알림은 선택 기능).
  if (!String(peer.backend_url ?? "").trim()) return 0;
  const { backend, token } = await ensureDeviceRegistered();
  const rows = loadInbox();
  let added = 0;
  let reregistered = false;
  for (;;) {
    const r = await peerFetch<{ event: EventDetail | null }>(backend, token, "POST", "/events/poll?wait=0");
    if (r.status === 401 && !reregistered) {
      // 서버 DB 가 초기화되면 로컬 device_id 가 남아 있어도 서버는 이 기기를
      // 모른다 — 같은 토큰으로 재등록하면 폴링이 곧바로 회복된다.
      const s = loadSettings();
      await registerDevice(backend, token, String(peerSettings(s).device_name ?? "").trim() || deviceNameFor(s));
      reregistered = true;
      continue;
    }
    const body = peerOk(r, "알림 폴링");
    const ev = body?.event;
    if (!ev) break;
    rows.push({
      id: randomUUID(),
      project_id: String(ev.project_id ?? ""),
      sender_device_name: String(ev.sender_device_name || ev.sender_device_id || ""),
      event_kind: String(ev.event_kind ?? ""),
      repo_name: String(ev.repo_name ?? ""),
      payload: String(ev.payload ?? ""),
      received_at: new Date().toISOString(),
      read: false,
    });
    added += 1;
    // 서버는 응답 시점에 이 이벤트를 '배달됨'으로 소비한다 — 한 건마다 바로 저장한다.
    saveInbox(rows);
  }
  return added;
}

// 저장소 ↔ 프로젝트 연결. Rust `RepoProjects` 와 같은 파일, 같은 열쇠(실제 경로).
function canonicalRepoPath(r: RepoRecord): string {
  if (r.ssh_host) return r.path;
  try {
    return realpathSync(r.path);
  } catch {
    return r.path;
  }
}

function loadRepoProjects(): Record<string, string[]> {
  try {
    const parsed = JSON.parse(readFileSync(join(appConfigDir(), REPO_PROJECTS_FILE), "utf8")) as unknown;
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) return parsed as Record<string, string[]>;
  } catch {
    /* 없으면 빈 매핑 */
  }
  return {};
}

function saveRepoProjects(m: Record<string, string[]>): void {
  mkdirSync(appConfigDir(), { recursive: true });
  writeFileSync(join(appConfigDir(), REPO_PROJECTS_FILE), JSON.stringify(m, null, 2));
}

function linkRepoProject(r: RepoRecord, projectId: string): void {
  const m = loadRepoProjects();
  const key = canonicalRepoPath(r);
  const list = m[key] ?? [];
  if (!list.includes(projectId)) list.push(projectId);
  m[key] = list;
  saveRepoProjects(m);
}

function unlinkRepoProject(r: RepoRecord, projectId: string): void {
  const m = loadRepoProjects();
  const key = canonicalRepoPath(r);
  const list = (m[key] ?? []).filter((id) => id !== projectId);
  if (list.length === 0) delete m[key];
  else m[key] = list;
  saveRepoProjects(m);
}

function projectsForRepo(r: RepoRecord): string[] {
  return loadRepoProjects()[canonicalRepoPath(r)] ?? [];
}

/**
 * 푸시가 성공하면 팀 서버에 이벤트를 보낸다 — 데스크톱 앱에서는 pre-push hook 이
 * `git-companion hook emit` 으로 하는 일이다. 미리보기에는 그 실행 파일이 없으므로
 * 여기서 같은 payload(`{kind, data:{author, message, sha, repo_name, url, branch}}`)를
 * 같은 규칙(병합 대상 브랜치면 main_push, 아니면 branch_push)으로 만든다.
 * hook 과 마찬가지로 실패해도 푸시 결과에는 영향을 주지 않는다(fail-open).
 */
async function emitPushEvent(r: RepoRecord, t: GitTarget, branch: string): Promise<void> {
  const peer = peerSettings(loadSettings());
  const backend = String(peer.backend_url ?? "").trim();
  const token = String(peer.device_token ?? "").trim() || (peer.device_id ? peerToken() : "");
  if (!backend || !token) return;
  const projects = projectsForRepo(r);
  if (projects.length === 0) return;
  const sha = tgGit(t, ["rev-parse", branch]).stdout.trim() || tgGit(t, ["rev-parse", "HEAD"]).stdout.trim();
  const author = tgGit(t, ["log", "-1", "--format=%an", sha]).stdout.trim() || "unknown";
  const message = tgGit(t, ["log", "-1", "--format=%s", sha]).stdout.trim();
  const url = normalizeRemoteUrl(tgGit(t, ["remote", "get-url", "origin"]).stdout.trim());
  const kind = (await isMergeTargetBranch(r, branch)) ? "main_push" : "branch_push";
  const payload = JSON.stringify({ kind, data: { author, message, sha, repo_name: r.display_name, url, branch } });
  for (const projectId of projects) {
    try {
      const res = await peerFetch(backend, token, "POST", "/events", {
        project_id: projectId,
        event_kind: kind,
        repo_name: r.display_name,
        payload,
      });
      if (res.status >= 300) console.warn(`[gc-bridge] 알림 전송 실패 (${res.status}) project=${projectId}`);
    } catch (e) {
      console.warn(`[gc-bridge] 알림 전송 실패: ${(e as Error).message}`);
    }
  }
}

/** Rust `gpconfig::is_merge_target` — merge_targets → default_base_branch → 등록 기본 브랜치. */
async function isMergeTargetBranch(r: RepoRecord, branch: string): Promise<boolean> {
  const res = (await dispatch({ cmd: "project_config_get", args: { repoId: r.id } })) as
    | { exists?: boolean; config?: { default_base_branch?: string; merge_targets?: string[] } }
    | { kind: string }
    | null;
  const registered = (r.default_branch || "main").trim();
  if (!res || "kind" in res) return branch.trim() === registered;
  const cfg = res.config;
  const fallback = res.exists && cfg?.default_base_branch?.trim() ? cfg.default_base_branch.trim() : registered;
  const targets = cfg?.merge_targets?.length ? cfg.merge_targets : [fallback];
  return targets.includes(branch.trim());
}

// ── IPC dispatch ────────────────────────────────────────────────────────────

export interface InvokeArgs {
  cmd: string;
  args: Record<string, unknown>;
}

function jsonError(kind: string, message: string): unknown {
  return { kind, message };
}

export async function dispatch(invoke: InvokeArgs): Promise<unknown> {
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
        if (!sshCfg) {
          // 로컬 경로는 여기서 검증하고 `~` 를 펼쳐 저장한다 — 펼치지 않으면
          // 등록은 되지만 이후 모든 git 호출이 없는 경로를 향한다.
          const checked = checkLocalRepoPath(a.project_path);
          if ("error" in checked) return jsonError("git", checked.error);
          a.project_path = checked.path;
        } else {
          const inside = remoteGit("rev-parse --is-inside-work-tree");
          if (!inside.ok) {
            return jsonError(
              "git",
              `원격 서버(${a.ssh_host})의 이 경로는 git 저장소가 아닙니다: ${a.project_path}`,
            );
          }
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
      // 아직 git 저장소가 아닌 폴더를 저장소로 만들고 바로 등록한다.
      // "이 폴더는 git 저장소가 아닙니다"에서 막힌 사람에게 앱 안의 다음 걸음.
      case "init_repository": {
        const raw = String(args.path ?? "");
        const p2 = expandTilde(raw);
        if (!p2) return jsonError("git", "저장소 폴더 경로를 입력하세요.");
        if (!existsSync(p2)) return jsonError("git", `그 경로에 폴더가 없습니다: ${p2}`);
        if (!statSync(p2).isDirectory()) return jsonError("git", `폴더가 아닙니다: ${p2}`);
        if (!existsSync(join(p2, ".git"))) {
          const out = git(p2, ["init", "-b", "main"]);
          if (!out.ok) {
            return jsonError("git", `git 저장소로 만들지 못했습니다: ${out.stderr.trim()}`);
          }
        }
        return dispatch({
          cmd: "register_repository",
          args: { args: { project_path: p2, ssh_host: "", ssh_user: "", ssh_key_path: "", ssh_password: "", ssh_port: 22 } },
        });
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
        return startMerge(
          targetOf(r),
          args.branchRef as string,
          args.base as string,
          (args.expectedSha as string | null | undefined) ?? null,
        );
      }
      case "merge_state": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        return mergeState(targetOf(r));
      }
      case "list_merged_remote_branches": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        const t = targetOf(r);
        const base = args.base as string;
        const baseRef = `origin/${base}`;
        const fmt = "%(refname:short)%09%(objectname)%09%(authorname)%09%(committerdate:unix)";
        const list = tgGit(t, ["for-each-ref", "refs/remotes/origin", "--format", fmt]);
        if (!list.ok) return jsonError("git", list.stderr.trim());
        const out: Array<{ name: string; short_name: string; author: string; unix_time: number }> = [];
        for (const line of list.stdout.split("\n")) {
          if (!line) continue;
          const [name, sha, author, unix] = line.split("\t");
          if (!name || !sha || name === baseRef || name.endsWith("/HEAD")) continue;
          if (!tgGit(t, ["merge-base", "--is-ancestor", name, baseRef]).ok) continue;
          out.push({
            name,
            short_name: name.replace(/^origin\//, ""),
            author: author ?? "",
            unix_time: Number(unix ?? 0),
          });
        }
        out.sort((a, b) => a.unix_time - b.unix_time);
        return out;
      }
      case "delete_remote_branch": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        const t = targetOf(r);
        const base = args.base as string;
        const branch = args.branch as string;
        if (branch === base) return jsonError("git", `병합 브랜치(${base})는 삭제할 수 없습니다.`);
        // 삭제 직전 조상 재확인 — Rust와 같은 안전장치.
        if (!tgGit(t, ["merge-base", "--is-ancestor", `origin/${branch}`, `origin/${base}`]).ok) {
          return jsonError("git", `${branch} 브랜치에 아직 ${base}에 없는 커밋이 있습니다 — 삭제하지 않았습니다.`);
        }
        const out = tgGit(t, ["push", "origin", "--delete", branch]);
        if (!out.ok) return jsonError("git", out.stderr.trim());
        tgGit(t, ["fetch", "--prune", "origin"]);
        return null;
      }
      case "base_unpushed_count": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        const t = targetOf(r);
        const base = args.base as string;
        if (!tgGit(t, ["rev-parse", "-q", "--verify", `refs/heads/${base}`]).ok) return 0;
        if (!tgGit(t, ["rev-parse", "-q", "--verify", `refs/remotes/origin/${base}`]).ok) return 0;
        const n = tgGit(t, ["rev-list", "--count", `refs/remotes/origin/${base}..refs/heads/${base}`]).stdout.trim();
        return Number(n) || 0;
      }
      case "branch_file_diff": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        const out = tgGit(targetOf(r), [
          "diff",
          `origin/${args.base as string}...${args.branchRef as string}`,
          "--",
          args.path as string,
        ]);
        if (!out.ok) return jsonError("git", out.stderr.trim());
        return out.stdout;
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
          // 인자를 비우면 설정에 저장된 전략을 쓴다 (Rust 쪽과 같은 규칙).
          loadSettings().ai?.binary_strategy ??
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
      //
      // 실제 배달망은 FastAPI 백엔드(`backend/`)가 담당한다. 여기서는 화면을
      // 눌러 볼 수 있을 만큼만 흉내를 낸다 — 수신자 목록은 프로세스 메모리에
      // 두어 "구성원 동기화" / "제거" 가 눈에 보이게 동작한다.
      // 로그인 화면의 "서버 주소" — 예전에는 이 두 명령이 없어서 shim 의 목
      // 데이터로 떨어졌다. "저장했습니다"라고 말하면서 아무것도 저장하지
      // 않았고, 그래서 로그인이 계속 실패하는데 이유를 알 수 없었다.
      case "peer_get_config": {
        const peer = (loadSettings().peer ?? {}) as Record<string, unknown>;
        return {
          backend_url: String(peer.backend_url ?? ""),
          device_token: String(peer.device_token ?? ""),
          device_id: String(peer.device_id ?? ""),
          device_name: String(peer.device_name ?? ""),
          last_poll_port: null,
        };
      }
      case "peer_set_backend_url": {
        const url = String(args.url ?? "").trim().replace(/\/+$/, "");
        if (!url) return jsonError("bad_request", "서버 주소를 입력하세요.");
        const cfg = loadSettings();
        cfg.peer = { ...(cfg.peer ?? {}), backend_url: url };
        saveSettings(cfg);
        return null;
      }
      // 저장한 주소로 실제 연결되는지 확인한다. 저장만 하고 "됐다"고 말하면
      // 오타 하나로 로그인이 계속 실패하는데 원인을 알 수 없다.
      case "peer_check_backend": {
        const url = String(args.url ?? "").trim().replace(/\/+$/, "") ||
          String((loadSettings().peer as { backend_url?: string } | undefined)?.backend_url ?? "");
        if (!url) return { ok: false, message: "서버 주소가 비어 있습니다." };
        try {
          const r = await fetch(`${url}/healthz`, { signal: AbortSignal.timeout(5000) });
          if (!r.ok) {
            return { ok: false, message: `서버가 응답했지만 상태가 정상이 아닙니다 (${r.status}).` };
          }
          return { ok: true, message: "서버에 연결됩니다." };
        } catch {
          return {
            ok: false,
            message: `연결할 수 없습니다. 주소가 맞는지, 서버가 실행 중인지 확인하세요:\n  cd backend && uvicorn app.main:app`,
          };
        }
      }
      // ── 수신함 (앱의 inbox.db 에 해당) ────────────────────────────────
      case "peer_unread_count": {
        return loadInbox().filter((r) => !r.read).length;
      }
      case "peer_poll_now": {
        try {
          await pollTeamEventsOnce();
        } catch (e) {
          return jsonError("internal", (e as Error).message ?? String(e));
        }
        return null;
      }
      case "peer_list_team_events": {
        const limit = Math.max(0, Number(args.limit ?? 50));
        const unreadOnly = Boolean(args.unreadOnly ?? args.unread_only);
        return loadInbox()
          .filter((r) => !unreadOnly || !r.read)
          .sort((a, b) => (a.received_at < b.received_at ? 1 : a.received_at > b.received_at ? -1 : 0))
          .slice(0, limit);
      }
      case "peer_mark_team_read": {
        const rows = loadInbox();
        const row = rows.find((r) => r.id === String(args.id ?? ""));
        if (!row) return jsonError("db", `team event ${String(args.id ?? "")} not found`);
        if (!row.read) {
          row.read = true;
          saveInbox(rows);
        }
        return null;
      }
      case "peer_mark_all_team_read": {
        const rows = loadInbox();
        let n = 0;
        for (const r of rows) {
          if (!r.read) {
            r.read = true;
            n += 1;
          }
        }
        if (n > 0) saveInbox(rows);
        return n;
      }

      // ── 기기 · 프로젝트 · 구성원 (팀 서버 그대로) ─────────────────────
      case "peer_register_device": {
        const backend = String(args.backendUrl ?? args.backend_url ?? "").trim().replace(/\/+$/, "");
        if (!backend) return jsonError("bad_request", "서버 주소를 입력하세요.");
        try {
          return await registerDevice(backend, peerToken(), String(args.name ?? "").trim() || "Git Companion");
        } catch (e) {
          return jsonError("internal", (e as Error).message ?? String(e));
        }
      }
      case "peer_list_projects": {
        try {
          const { backend, token } = await ensureDeviceRegistered();
          const body = peerOk(
            await peerFetch<{ projects: unknown[] }>(backend, token, "GET", "/projects"),
            "프로젝트 목록",
          );
          return body?.projects ?? [];
        } catch (e) {
          return jsonError("internal", (e as Error).message ?? String(e));
        }
      }
      case "peer_create_project":
      case "peer_join_project": {
        try {
          const { backend, token } = await ensureDeviceRegistered();
          const info = cmd === "peer_create_project"
            ? peerOk(
                await peerFetch<{ id: string }>(backend, token, "POST", "/projects", {
                  display_name: String(args.name ?? "").trim(),
                }),
                "팀 만들기",
              )
            : peerOk(
                await peerFetch<{ id: string }>(backend, token, "POST", "/projects/join", {
                  join_code: String(args.code ?? "").trim(),
                }),
                "팀 합류",
              );
          const repoId = (args.repoId ?? args.repo_id) as string | null | undefined;
          if (repoId) {
            const r = repoById(repoId);
            if (!("error" in r)) linkRepoProject(r, info.id);
          }
          return info;
        } catch (e) {
          return jsonError("internal", (e as Error).message ?? String(e));
        }
      }
      case "peer_leave_project": {
        const projectId = String(args.projectId ?? args.project_id ?? "");
        try {
          const { backend, token, deviceId } = await ensureDeviceRegistered();
          peerOk(
            await peerFetch(backend, token, "DELETE", `/projects/${projectId}/members/${deviceId}`),
            "팀 나가기",
          );
        } catch (e) {
          return jsonError("internal", (e as Error).message ?? String(e));
        }
        const m = loadRepoProjects();
        for (const key of Object.keys(m)) {
          const list = (m[key] ?? []).filter((id) => id !== projectId);
          if (list.length === 0) delete m[key];
          else m[key] = list;
        }
        saveRepoProjects(m);
        return null;
      }
      case "peer_link_repo_to_project":
      case "peer_unlink_repo": {
        const r = repoById(String(args.repoId ?? args.repo_id ?? ""));
        if ("error" in r) return jsonError("repo_not_found", r.error);
        const projectId = String(args.projectId ?? args.project_id ?? "");
        if (cmd === "peer_link_repo_to_project") linkRepoProject(r, projectId);
        else unlinkRepoProject(r, projectId);
        return null;
      }
      case "peer_repos_for_project": {
        const projectId = String(args.projectId ?? args.project_id ?? "");
        return loadSettings()
          .repositories.filter((r) => projectsForRepo(r).includes(projectId))
          .map((r) => ({ repo_id: r.id, display_name: r.display_name, path: r.path }));
      }
      case "peer_list_members": {
        const projectId = String(args.projectId ?? args.project_id ?? "");
        try {
          const { backend, token } = await ensureDeviceRegistered();
          const body = peerOk(
            await peerFetch<{ members: unknown[] }>(backend, token, "GET", `/projects/${projectId}/members/email`),
            "구성원 목록",
          );
          return body?.members ?? [];
        } catch (e) {
          return jsonError("internal", (e as Error).message ?? String(e));
        }
      }
      case "peer_invite_by_email": {
        const projectId = String(args.projectId ?? args.project_id ?? "");
        const email = String(args.email ?? "").trim().toLowerCase();
        if (!email) return jsonError("bad_request", "이메일이 비어 있습니다.");
        try {
          const { backend, token } = await ensureDeviceRegistered();
          const body: Record<string, unknown> = { email, role: String(args.role ?? "member") };
          if (args.name) body.name = String(args.name);
          return peerOk(
            await peerFetch(backend, token, "POST", `/projects/${projectId}/members/email`, body),
            "초대",
          );
        } catch (e) {
          return jsonError("internal", (e as Error).message ?? String(e));
        }
      }
      case "peer_remove_email_invite": {
        const projectId = String(args.projectId ?? args.project_id ?? "");
        const email = String(args.email ?? "").trim().toLowerCase();
        try {
          const { backend, token } = await ensureDeviceRegistered();
          peerOk(
            await peerFetch(backend, token, "DELETE", `/projects/${projectId}/members/email/${encodeURIComponent(email)}`),
            "초대 제거",
          );
          return null;
        } catch (e) {
          return jsonError("internal", (e as Error).message ?? String(e));
        }
      }
      case "peer_local_url": {
        // 미리보기에는 푸시 배달용 sidecar 가 없다 — 앱과 같은 문구로 알린다.
        return jsonError("config", "peer listener not started");
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
      // 병합 탭 상단의 "최근 7일 병합 흐름" (Rust `git::timeline::merge_timeline` 의 쌍둥이).
      case "merge_timeline": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        const t = targetOf(r);
        try {
          return mergeTimeline((a) => tgGit(t, a), "origin", String(args.base ?? r.default_branch ?? "main"), Number(args.days ?? 7));
        } catch (e) {
          return jsonError("git", (e as Error).message ?? String(e));
        }
      }
      case "status": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        return workingTreeStatus(targetOf(r), r.default_branch || "main");
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
        if (!out.ok) return jsonError("git", explainCommitFailure(out.stdout, out.stderr));
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
          branch = currentBranch(t);
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
          out = tgGit(t, ["push", "-u", "origin", `HEAD:${branch}`]);
        }
        // pre-push hook 의 알림 전송에 해당한다 (실행 파일이 없는 미리보기용).
        if (out.ok) await emitPushEvent(r, t, branch);
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
          message: out.ok
            ? out.stdout.trim() || out.stderr.trim()
            : friendlyGitError(out.stderr || out.stdout),
        };
      }
      case "pull": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        const branch = currentBranch(targetOf(r));
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
        const saved = loadSettings().ai;
        // 오래된 config.json 에는 새 필드가 없다 — Rust 쪽 serde(default)와
        // 같은 기본값으로 채워서 UI가 undefined 를 만나지 않게 한다.
        return {
          enabled: saved?.enabled ?? false,
          base_url: saved?.base_url ?? "",
          api_key: saved?.api_key ?? "",
          model: saved?.model ?? "",
          system_prompt: saved?.system_prompt ?? "",
          auto_resolve: saved?.auto_resolve ?? false,
          auto_push: saved?.auto_push ?? false,
          binary_strategy: saved?.binary_strategy || "theirs",
        };
      }
      case "ai_default_prompt": {
        return DEFAULT_AI_PROMPT;
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
      // ── 계정 (팀 서버의 users 테이블) ────────────────────────────────
      //
      // 흉내를 내지 않고 실제 백엔드(`backend/`)의 `/auth/*` 를 그대로
      // 호출한다. 미리보기에서 가입한 계정이 데스크톱 앱에서도 그대로
      // 로그인되어야 하고, 비밀번호 규칙·중복 검사 같은 것을 두 곳에서
      // 따로 구현하면 반드시 어긋난다.
      //
      // 토큰은 Rust 와 같은 자리(설정 파일의 `session`)에 저장한다.
      case "account_register": {
        return authProxy("POST", "/auth/register", {
          name: args.name,
          email: args.email,
          username: args.username,
          password: args.password,
        }).then(saveSessionFromAuth);
      }
      case "account_login_by_password": {
        return authProxy("POST", "/auth/login", {
          username: args.username,
          password: args.password,
        }).then(saveSessionFromAuth);
      }
      case "account_logout": {
        const s = loadSettings();
        const token = s.session?.token;
        if (token) await authProxy("POST", "/auth/logout", undefined, token).catch(() => null);
        s.session = null;
        saveSettings(s);
        return null;
      }
      case "account_current": {
        return loadSettings().session?.user ?? null;
      }
      case "account_refresh": {
        const s = loadSettings();
        const token = s.session?.token;
        if (!token) return null;
        try {
          const user = (await authProxy("GET", "/auth/me", undefined, token)) as AccountRecord;
          s.session = { user, token };
          saveSettings(s);
          return user;
        } catch (e) {
          // 401 이면 세션이 사라진 것 — 그 외(네트워크 등)는 캐시를 유지한다.
          if (String((e as Error).message).includes("401")) {
            s.session = null;
            saveSettings(s);
            return null;
          }
          return s.session?.user ?? null;
        }
      }
      case "account_update_profile": {
        const s = loadSettings();
        const token = requireSessionToken(s);
        const body: Record<string, unknown> = {};
        if (args.name !== undefined && args.name !== null) body.name = args.name;
        if (args.email !== undefined && args.email !== null) body.email = args.email;
        const user = (await authProxy("PATCH", "/auth/me", body, token)) as AccountRecord;
        s.session = { user, token };
        saveSettings(s);
        return user;
      }
      case "account_change_password": {
        const s = loadSettings();
        const token = requireSessionToken(s);
        await authProxy(
          "POST",
          "/auth/me/password",
          {
            current_password: args.currentPassword ?? args.current_password,
            new_password: args.newPassword ?? args.new_password,
          },
          token,
        );
        return null;
      }
      case "account_delete_self": {
        const s = loadSettings();
        const token = requireSessionToken(s);
        await authProxy("DELETE", "/auth/me", undefined, token);
        s.session = null;
        saveSettings(s);
        return null;
      }
      case "account_search": {
        const s = loadSettings();
        const q = String(args.query ?? "").trim();
        if (q.length < 2) return [];
        const token = requireSessionToken(s);
        return authProxy("GET", `/auth/users?q=${encodeURIComponent(q)}`, undefined, token);
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
        if (raw) {
          try {
            return { exists: true, config: normalizeProjectConfig(JSON.parse(raw)) };
          } catch (e) {
            return jsonError("git", `.gpconfig 파싱 실패: ${(e as Error).message}`);
          }
        }
        // 작업 브랜치에 사본이 없으면 병합 브랜치에 커밋된 팀 규칙을 읽는다
        // (Rust `read_config_effective`와 같은 규칙). 없으면 병합 관리자
        // 미지정으로 읽혀 팀원에게 관리자 화면이 뜬다.
        const base = (r.default_branch || "main").trim();
        for (const rev of [`origin/${base}`, base]) {
          const out = tgGit(t, ["show", `${rev}:.gpconfig`]);
          if (!out.ok || !out.stdout.trim()) continue;
          try {
            return { exists: true, config: normalizeProjectConfig(JSON.parse(out.stdout.trim())) };
          } catch {
            continue;
          }
        }
        return { exists: false, config: defaultProjectConfig() };
      }
      case "project_config_set": {
        const r = repoById(args.repoId as string);
        if ("error" in r) return jsonError("repo_not_found", r.error);
        const t = targetOf(r);
        let cfg = normalizeProjectConfig((args.config ?? {}) as ProjectConfigRecord);
        // 저장하는 사람도 구성원으로 자동 포함 (로그인 상태일 때).
        // normalizeProjectConfig 는 항상 members 를 채우지만 타입은 옵셔널이라
        // 지역 변수로 받아 좁힌다.
        const me = loadSettings().session?.user ?? null;
        const members = cfg.members ?? [];
        if (me && !members.some((m) => m.email.toLowerCase() === me.email.toLowerCase())) {
          members.push({ id: me.id, name: me.name, email: me.email, role: "member" });
        }
        cfg.members = members;
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

      // ── SSH (minimal mocks) ─
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
  merged_locally: boolean;
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
    // %(refname:short)는 origin/HEAD를 "origin"으로 줄인다 — 유령 카드 방지.
    if (name === remote || name.endsWith("/HEAD")) continue;
    const anc = tgGit(t, ["merge-base", "--is-ancestor", name, baseRef]);
    if (anc.ok) continue;
    // 로컬 base에는 이미 병합됐지만 push가 안 된 상태 (Rust와 동일한 규칙).
    const mergedLocally =
      tgGit(t, ["rev-parse", "-q", "--verify", `refs/heads/${base}`]).ok &&
      tgGit(t, ["merge-base", "--is-ancestor", name, `refs/heads/${base}`]).ok;
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
      merged_locally: mergedLocally,
    });
  }
  out.sort((a, b) => b.unix_time - a.unix_time);
  return out;
}

function startMerge(
  t: GitTarget,
  branchRef: string,
  base: string,
  expectedSha: string | null = null,
): MergeOutcome {
  // Rust start_merge와 같은 가드 — 진행 중 병합 위에 새 병합을 얹지 않는다.
  if (tgGit(t, ["rev-parse", "-q", "--verify", "MERGE_HEAD"]).ok) {
    return {
      ok: false,
      conflicted: false,
      conflicted_files: [],
      message: "이미 진행 중인 병합이 있습니다. 병합 탭에서 먼저 마무리하거나 중단하세요.",
    };
  }
  const dirty = tgGit(t, ["status", "--porcelain=v2", "--untracked-files=no"]).stdout.trim();
  if (dirty) {
    return {
      ok: false,
      conflicted: false,
      conflicted_files: [],
      message: "작업 트리에 커밋되지 않은 변경이 있습니다. 작업 탭에서 커밋하거나 stash하세요.",
    };
  }
  const head = currentBranch(t);
  if (head !== base) {
    tgGit(t, ["fetch", "origin", `${base}:${base}`]);
  }
  tgGit(t, ["fetch", "--prune", "origin"]);
  const tip = tgGit(t, ["rev-parse", "-q", "--verify", branchRef]);
  if (!tip.ok) {
    return {
      ok: false,
      conflicted: false,
      conflicted_files: [],
      message: `${branchRef} 브랜치를 찾을 수 없습니다 — 방금 원격에서 삭제되었을 수 있습니다. 목록을 새로고침하세요.`,
    };
  }
  const actual = tip.stdout.trim();
  if (expectedSha && actual !== expectedSha && !actual.startsWith(expectedSha)) {
    return {
      ok: false,
      conflicted: false,
      conflicted_files: [],
      message: `검토한 뒤 이 브랜치에 새 push가 있었습니다(또는 히스토리가 바뀌었습니다). 목록을 새로고침해 최신 내용을 확인한 뒤 다시 병합하세요.`,
    };
  }
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
  /** "skipped" 는 `remainingReasons` 항목에만 쓴다 (자동으로 안 고친 파일). */
  method: "ai" | "ours" | "theirs" | "skipped";
  note: string | null;
}
// Rust `git::auto::AutoResolveReport` 와 같은 와이어 형식(camelCase).
interface AutoResolveReport {
  resolved: FileResolution[];
  remaining: string[];
  remainingReasons: FileResolution[];
  committed: boolean;
  backupId: string | null;
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

/// 한쪽만 base에서 바뀐 충돌이면 그 바뀐 쪽을 돌려준다 (Rust `one_sided_change`와
/// 같은 규칙). 양쪽이 모두 바뀌었으면 null — 자동으로 고를 정답이 없다.
function oneSidedChange(detail: { base: string | null; ours: string; theirs: string }):
  | "ours"
  | "theirs"
  | null {
  if (detail.base === null) return null;
  if (detail.base === detail.theirs) return "ours";
  if (detail.base === detail.ours) return "theirs";
  return null;
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
        remainingReasons: [],
        committed: false,
        backupId: null,
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
  //
  // 이 브릿지는 일부러 LLM을 호출하지 않는다 (미리보기에 API 키가 없다). 즉
  // 항상 "AI를 못 쓴 상태"와 같으므로, Rust 쪽과 동일한 안전 규칙을 쓴다:
  // AI가 켜져 있으면 양쪽이 모두 고친 텍스트 파일을 자동으로 한쪽만 남기지
  // 않는다 — 팀원의 커밋이 조용히 사라진 채 커밋/푸시되는 것을 막는다.
  const aiEnabled = loadSettings().ai?.enabled ?? false;
  const textFallback: "ours" | "theirs" | null = aiEnabled ? null : binaryStrategy;
  const resolved: FileResolution[] = [];
  const skipNotes = new Map<string, string>();
  let remaining = [...remaining0];
  for (const path of remaining0) {
    const detail = conflictDetail(t, path);
    let method: "ai" | "ours" | "theirs";
    let note: string | null = null;
    if (detail.is_binary || detail.too_large) {
      method = binaryStrategy;
      note = "바이너리/대용량 파일 — 내용 확인 후 병합 탭에서 검토하세요.";
    } else {
      const side = oneSidedChange(detail) ?? textFallback;
      if (side === null) {
        // 사람에게 넘긴다 — 충돌 상태 그대로 남겨 둔다.
        skipNotes.set(
          path,
          "양쪽에서 모두 수정된 파일입니다. AI 결과를 쓸 수 없어 자동으로 한쪽을 고르지 않았습니다 — 병합 탭에서 직접 확인하세요.",
        );
        continue;
      }
      method = side;
      note = "AI 결과를 사용할 수 없어 규칙 기반으로 선택했습니다.";
    }
    tgGit(t, ["checkout", `--${method}`, "--", path]);
    tgGit(t, ["add", "--", path]);
    resolved.push({ path, method, note });
  }
  remaining = tgGit(t, ["diff", "--name-only", "--diff-filter=U"]).stdout.split("\n").filter(Boolean);
  const remainingReasons: FileResolution[] = remaining.map((path) => ({
    path,
    method: "skipped",
    note: skipNotes.get(path) ?? null,
  }));

  if (remaining.length === 0 && st.in_progress) {
    const branch = mergeHeadBranch(t);
    const commit = tgGit(t, ["commit", "-m", `AI 자동 병합: ${branch}`]);
    if (commit.ok) {
      return {
        resolved,
        remaining,
        remainingReasons,
        committed: true,
        backupId: backupId,
        message: `충돌 ${resolved.length}개를 자동 해결하고 ‘AI 자동 병합: ${branch}’로 커밋했습니다.`,
      };
    }
    return {
      resolved,
      remaining,
      remainingReasons,
      committed: false,
      backupId: backupId,
      message: `모든 충돌은 해결됐지만 커밋에 실패했습니다: ${commit.stderr.trim()}. 충돌 전 상태는 백업에 보존되어 있으니 병합 센터의 ‘병합 완료’로 마무리하세요.`,
    };
  }
  return {
    resolved,
    remaining,
    remainingReasons,
    committed: false,
    backupId: backupId,
    message:
      `충돌 ${remaining0.length}개 중 ${remaining.length}개를 해결하지 못했습니다. ` +
      (textFallback === null
        ? "양쪽에서 모두 수정된 파일은 자동으로 한쪽을 고르지 않습니다 — 병합 탭에서 직접 확인하세요."
        : "남은 파일은 병합 센터에서 처리하세요."),
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
  const head = currentBranch(t);
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
  behind_base: number;
  files: Array<{ kind: string; path: string; staged: boolean; unstaged: boolean }>;
}
function workingTreeStatus(t: GitTarget, base?: string): WorkingTreeStatus {
  const branch = currentBranch(t);
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
      // `2 `(rename) 라인은 <X><score> 필드가 하나 더 있다 (Rust와 동일 규칙).
      const path = fields.slice(line.startsWith("2 ") ? 9 : 8).join(" ");
      const staged = xy[0] !== " " && xy[0] !== "." && xy[0] !== "?";
      const unstaged = xy[1] !== " " && xy[1] !== ".";
      const eff = xy[0] === "." || xy[0] === " " ? xy[1] : xy[0];
      const kind =
        eff === "A" ? "added" :
        eff === "D" ? "deleted" :
        eff === "R" ? "renamed" :
        eff === "C" ? "copied" :
        eff === "U" ? "conflicted" : "modified";
      files.push({ kind, path: path.split("\t")[0] ?? path, staged, unstaged });
    } else if (line.startsWith("u ")) {
      // u <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path> — 필드 11개.
      const fields = line.split(" ");
      const xy = fields[1] ?? "";
      const path = fields.slice(10).join(" ");
      files.push({
        kind: "conflicted",
        path,
        staged: xy[0] !== "." && xy[0] !== " ",
        unstaged: xy[1] !== "." && xy[1] !== " ",
      });
    } else if (line.startsWith("? ")) {
      files.push({ kind: "untracked", path: line.slice(2), staged: false, unstaged: false });
    }
  }
  // Rust와 동일: origin/<base> 트래킹 ref 대비 뒤처짐 (fetch 없이 계산).
  let behindBase = 0;
  if (base) {
    const baseRef = `refs/remotes/origin/${base}`;
    if (tgGit(t, ["rev-parse", "-q", "--verify", baseRef]).ok) {
      const n = tgGit(t, ["rev-list", "--count", `HEAD..${baseRef}`]).stdout.trim();
      behindBase = Number(n) || 0;
    }
  }
  return { branch: branch || null, upstream: upstream || null, ahead: ahead ?? 0, behind: behind ?? 0, behind_base: behindBase, files };
}

// ── plugin ──────────────────────────────────────────────────────────────────

// ── 워커 풀 ─────────────────────────────────────────────────────────────────
//
// git 호출은 전부 spawnSync 다. vite 개발 서버는 스레드가 하나라서, SSH
// 저장소의 병합 대기 조회(수 초)가 도는 동안 다른 저장소 카드·알림 폴링·
// 버튼 응답까지 전부 멈췄다 — 화면이 통째로 얼어 보이던 원인. 실제 앱(Rust)은
// 커맨드를 스레드 풀에서 돌리므로 그런 일이 없다. 미리보기도 같게 보이도록
// 저장소 명령을 워커 스레드에서 돌린다.
//
// 같은 저장소(repoId)의 명령은 항상 같은 워커로 보내 순서를 지킨다(병합 시작 →
// 상태 조회 같은 연쇄가 어긋나지 않게). `peer_*` 는 fetch 기반이라 막히지
// 않고, 수신함 파일을 한 곳에서만 고치도록 메인 스레드에 남긴다.
// 번들에 실패하면(esbuild 없음 등) 예전처럼 단일 스레드로 동작한다.

const WORKER_COUNT = 4;

function bundleWorker(): string | null {
  try {
    const require = createRequire(import.meta.url);
    const esbuild = require("esbuild") as { buildSync: (o: Record<string, unknown>) => void };
    const here = dirname(fileURLToPath(import.meta.url));
    const entry = [
      join(here, "bridge-worker.ts"),
      join(here, "dev", "bridge-worker.ts"),
      join(process.cwd(), "dev", "bridge-worker.ts"),
    ].find((p) => existsSync(p));
    if (!entry) return null;
    const outfile = join(tmpdir(), `gc-bridge-worker-${process.pid}.mjs`);
    esbuild.buildSync({
      entryPoints: [entry],
      bundle: true,
      platform: "node",
      format: "esm",
      target: "node20",
      outfile,
      logLevel: "silent",
      external: ["vite"],
    });
    return outfile;
  } catch (e) {
    console.warn(`[gc-bridge] 워커 번들 실패 — 단일 스레드로 동작합니다: ${(e as Error).message}`);
    return null;
  }
}

interface PoolSlot {
  worker: Worker;
  pending: Map<number, { resolve: (v: unknown) => void; reject: (e: Error) => void }>;
  dead: boolean;
}

class BridgePool {
  private readonly slots: PoolSlot[] = [];
  private seq = 0;
  private closed = false;

  constructor(private readonly bundle: string, size: number) {
    for (let i = 0; i < size; i++) this.slots.push(this.spawn(i));
  }

  private spawn(index: number): PoolSlot {
    const slot: PoolSlot = { worker: new Worker(this.bundle), pending: new Map(), dead: false };
    slot.worker.on("message", (m: { id: number; result: unknown }) => {
      const p = slot.pending.get(m.id);
      if (!p) return;
      slot.pending.delete(m.id);
      p.resolve(m.result);
    });
    const die = (why: string) => {
      if (slot.dead) return;
      slot.dead = true;
      for (const p of slot.pending.values()) {
        p.reject(new Error(`미리보기 브리지 워커가 중단되었습니다: ${why}`));
      }
      slot.pending.clear();
      if (!this.closed) this.slots[index] = this.spawn(index);
    };
    slot.worker.on("error", (e) => die(e.message));
    slot.worker.on("exit", (code) => {
      if (code !== 0) die(`exit ${code}`);
    });
    return slot;
  }

  run(body: InvokeArgs): Promise<unknown> {
    const key = String(body.args?.repoId ?? body.args?.repo_id ?? body.cmd);
    let h = 0;
    for (const ch of key) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
    const slot = this.slots[h % this.slots.length]!;
    const id = ++this.seq;
    return new Promise((resolve, reject) => {
      slot.pending.set(id, { resolve, reject });
      slot.worker.postMessage({ id, body });
    });
  }

  close(): void {
    this.closed = true;
    for (const s of this.slots) void s.worker.terminate();
  }
}

/** 메인 스레드에서 처리할 명령 — 수신함 파일의 단일 작성자를 지킨다. */
function runsOnMainThread(cmd: string): boolean {
  return cmd.startsWith("peer_");
}

export function gitBridgePlugin(): Plugin {
  return {
    name: "git-companion-bridge",
    configureServer(server: ViteDevServer) {
      const bundle = bundleWorker();
      const pool = bundle ? new BridgePool(bundle, WORKER_COUNT) : null;
      server.httpServer?.once("close", () => pool?.close());
      console.log(
        pool
          ? `[gc-bridge] 저장소 명령을 워커 ${WORKER_COUNT}개에서 처리합니다 (${bundle})`
          : "[gc-bridge] 워커를 띄우지 못해 단일 스레드로 동작합니다 — 느린 저장소가 화면 전체를 멈출 수 있습니다.",
      );
      // 경로를 접두사로 마운트하지 않고 직접 비교한다. code-server 같은
      // 리버스 프록시 뒤에서 볼 때는 vite 를 base(`/absproxy/5173/`) 아래에서
      // 띄우므로 요청 경로가 `/absproxy/5173/__gc/invoke` 로 들어온다.
      // 플러그인 미들웨어는 vite 의 base 제거 미들웨어보다 먼저 실행되기
      // 때문에, 접두사 마운트는 그 경우 매칭에 실패한다.
      server.middlewares.use(async (req, res, next) => {
        const path = (req.url ?? "").split("?")[0] ?? "";
        if (req.method !== "POST" || !path.endsWith("/__gc/invoke")) {
          next();
          return;
        }
        try {
          const body = (await readBody(req)) as InvokeArgs;
          const result = pool && !runsOnMainThread(body.cmd) ? await pool.run(body) : await dispatch(body);
          send(res, 200, result);
        } catch (e) {
          send(res, 400, jsonError("bad_request", (e as Error).message ?? String(e)));
        }
      });
    },
  };
}
