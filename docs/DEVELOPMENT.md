# 개발자 문서 (Developer notes)

이 문서는 **코드를 고치거나 빌드 환경을 손보는 사람**을 위한 것입니다. 앱을 쓰는
방법은 [README](../README.md)와 [WORKFLOW.md](WORKFLOW.md)에, 브라우저 미리보기는
[PREVIEW.md](PREVIEW.md)에 있습니다. 아래 내용은 영어 원문 그대로입니다.

## First-run flow

1. Launch the app — the Home view appears.
2. **프로젝트 추가** — enter the repository path. Optionally, fill in SSH host,
   user, and key path if the repo is accessed over SSH. If you authenticate
   with a **user + password** instead of a key, leave the key path empty and
   type the password into **SSH 비밀번호** (this needs `sshpass` on the
   machine; the app tells you when it is missing). Password auth is also
   available from **설정 → SSH 연결 테스트**. For SSH repos you can
   also click **SSH로 찾아보기**: the app connects over SSH and shows a remote
   directory browser (breadcrumb path, up-navigation, hidden/git folders, a
   "git 저장소" badge when the current path is inside a work tree) so you can
   walk to the project instead of typing its path. **이 경로 사용** fills the
   path field with the resolved absolute remote path.
3. Click **등록** — Git Companion pings the remote, reads the remote URL and
   current branch, then saves the repository.
4. **브랜치 선택** — pick the working branch from the dropdown.
5. The repo card on Home now shows **다음 할 일** — press it and the app takes
   you to the right tab (resolve conflicts → merge → commit → push → sync).
6. Optional but recommended for teams: open the repo's **설정** tab and set the
   merge target branch(es) and the per-branch merge manager. It is committed
   into the repo as `.gpconfig`, so everyone sees the same rules.
7. Optional: **설정 → AI 자동 병합** — save the resolver prompt *before* you
   need it, and turn on "충돌이 나면 곧바로 자동 해결".

## Architecture

- **Rust core (`src-tauri/`)** — Tauri commands; git CLI wrapper via `Target`
  enum that routes to either local git or `ssh user@host git -C /path`. SSH
  argv is always passed as separate arguments, never as a shell string.
- **SSH target** — `git::Target::Local(PathBuf)` or
  `git::Target::Ssh { user, host, key, password, path }`. Every git op takes
  `&Target`. Auth: SSH key (`-i`) or user/password via `sshpass -e` (the
  password travels in the `SSHPASS` env var, never on the command line).
  Known-host checking is `StrictHostKeyChecking=accept-new` in both modes
  (`BatchMode=yes` for keys): the first connection to a host records its key
  in `~/.ssh/known_hosts` (TOFU), and every later connection verifies the
  key strictly, so a changed server key still fails loudly.
- **Auth fallback** — when both a key path and a password are set, the app
  tries the password first and falls back to the key if the server rejects
  it (e.g. Ubuntu's default `PermitRootLogin prohibit-password` blocks root
  password logins while still accepting keys). Password auth never puts the
  password on the command line: it is passed via the `SSHPASS` env var to
  `sshpass -e` (`PreferredAuthentications=password`, `PubkeyAuthentication=no`,
  `NumberOfPasswordPrompts=1`).
- **UI (`ui/`)** — Vite + TypeScript + Tailwind v4, no framework. Views: Home
  (repo cards with the suggested next action), Repo (work / merge / project
  config tabs), Team (inbox), Settings (SSH profile, sub-tool launcher, AI
  auto-merge, push credentials). The "next action" rule lives in
  `ui/components/nextAction.ts` as a pure function; the merge-time change map
  in `ui/components/ChangeMap.ts`.
- **pre-push hook** — embedded in the binary via `include_str!`, installed to
  `<config_dir>/com.gitcompanion.app/hooks/pre-push`, symlinked into each
  registered repo's `.git/hooks/`, and re-installed on every app launch so a
  changed template lands without a manual step. Fires `git-companion hook emit`
  for every pushed ref; the binary POSTs the event to the team backend via the
  fan-out API. The shell script carries no branch policy — `hook emit` reads
  `.gpconfig` and decides whether the pushed branch is a merge target
  (`main_push`, notify everyone) or a work branch (`branch_push`, notify the
  merge manager). See `docs/HOOKS.md`.
- **Team backend** (`backend/`) — FastAPI + SQLAlchemy over SQLite. Owns the
  `users` table (login accounts; PBKDF2-hashed passwords, revocable session
  tokens in `user_sessions`) and the notification fan-out. `/auth/*` handles
  register, login, profile, password, logout, delete, and teammate lookup.
- **Accounts in the app** — `src-tauri/src/accounts.rs` talks to `/auth/*` and
  caches only the signed-in user + token in `config.json` (`session`), so the
  app stays signed in offline. It never stores a password or password hash.

## Security notes

- **Login passwords never reach this app.** They are sent once to the team
  server over the wire and stored there as PBKDF2-HMAC-SHA256 (16-byte salt,
  210k iterations). `config.json` holds a revocable session token, not a
  password or its hash. Run the backend over HTTPS in production — the token
  is a bearer credential.
- SSH private keys, SSH passwords, and SSH profile settings are stored in
  plaintext in `~/.config/com.gitcompanion.app/config.json`. Store only key
  files with restricted permissions (`chmod 600`); prefer SSH keys over
  passwords where possible.
- The `tauri.conf.json` CSP restricts `script-src` to `'self'`.
- All team backend communication is over HTTPS in production.

## Testing

```bash
# Rust — git ops, config schema, hook emission, merge/auto-merge, AI transport
cargo test --manifest-path src-tauri/Cargo.toml

# Frontend — pure logic (no DOM): conflict parser, commit-graph lanes,
#            change map, next-action priority
pnpm test:ui

# Type check only
pnpm typecheck

# 브라우저에서 눌러 보며 확인 (docs/PREVIEW.md)
pnpm seed:demo && pnpm dev:web

# Team backend (계정·알림) — 33 tests
cd backend && ./.venv/bin/python -m pytest -q
```

`pnpm test:ui` bundles every `ui/**/*.test.ts` with esbuild and runs it under
node — see `dev/run-ui-tests.mjs`. The tests use plain assertions, so no test
framework is in the dependency tree.

> Building the Rust crate needs the WebKitGTK development headers
> (`javascriptcoregtk-4.1`); the per-OS install commands are in the README's
> **설치 · 빌드** section. `cargo test` cannot link without them.

## Layout

```
git-program/
├── Cargo.toml                       # workspace root
├── package.json                     # ui deps
├── vite.config.ts
├── tsconfig.json
├── index.html
├── ui/                              # frontend sources
│   ├── main.ts
│   ├── lib/{ipc,format,app,session,sshTest}.ts
│   ├── components/{Sidebar,RepositoryCard,nextAction,ChangeMap,
│   │              MergeCenter,MergeTimeline,ProjectConfigPanel,TeamInbox,
│   │              TeamPanel,StatusTable,GitGraph,conflictParser,Modal,Toast}.ts
│   ├── views/{HomeView,RepoView,SettingsView}.ts
│   └── styles/{tokens,app,timeline}.css
├── dev/                             # browser preview + tooling
│   ├── git-bridge.ts                # Vite plugin: real git behind the app's IPC
│   ├── bridge-worker.ts             # worker-thread entry (repo commands off the main thread)
│   ├── bridge-timeline.ts           # TS twin of git/timeline.rs for the preview
│   ├── serve-web.mjs                # `pnpm dev:web` — vite behind a proxy path
│   ├── seed-demo.mjs                # `pnpm seed:demo` — demo repo + team + notifications
│   ├── demo-teammate.mjs            # `pnpm demo:push` — a teammate pushes + notifies
│   └── run-ui-tests.mjs             # `pnpm test:ui`
├── docs/{PREVIEW,WORKFLOW,HOOKS,PEER,DEVELOPMENT}.md
├── src-tauri/
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── build.rs
│   ├── capabilities/main.json
│   ├── icons/
│   ├── templates/pre-push
│   ├── src/
│   │   ├── main.rs                  # GUI launcher + `hook emit` subcommand
│   │   ├── lib.rs                   # tauri builder + invoke_handler
│   │   ├── error.rs
│   │   ├── config_store.rs          # Repository, SshProfile, AiConfig,
│   │   │                             # ExternalTool, Account, AppSettings
│   │   ├── pre_push_hook.rs
│   │   ├── accounts.rs               # /auth/* client (login, profile, search)
│   │   ├── gpconfig.rs               # .gpconfig: merge targets + managers
│   │   ├── ai.rs                     # resolver prompt + OpenAI-compatible call
│   │   ├── commands/{mod,repo,git,auto,config,project,account,
│   │   │            external,peer,ai}.rs
│   │   ├── git/{mod,ops,branches,log,status,merge,auto,sync,fetch,push,timeline}.rs
│   │   └── notify/{mod,store,webhook}.rs
│   └── tests/*.rs
├── backend/                         # team backend (FastAPI)
│   ├── app/
│   │   ├── models.py                # User, UserSession, Device, Project, …
│   │   ├── auth.py                  # PBKDF2 password hashing + token hashing
│   │   ├── routes/{auth,events,members,projects,devices}.py
│   │   ├── models.py
│   │   └── schemas.py
│   └── tests/
└── PRODUCT.md / DESIGN.md           # product brief + design system
```
