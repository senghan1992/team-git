# Git Companion

A lightweight desktop app for teams that work with git. Register local or
SSH-accessible repositories, commit and push from a single diff-pane UI, and
manage your team through a shared backend — invite teammates by email and
receive their push events in the team inbox.

## Features

- **Default UI language: Korean** — all user-facing strings are displayed in Korean.
- **SSH auto-discovery** — register a repository via local path; if accessed over
  SSH, Git Companion discovers the SSH host/user/key from your SSH config and
  uses it for all remote operations.
- **In-app commit/push/pull** — stage files, write a commit message, and push
  without touching the terminal. The work tab polls status every few seconds so
  teammates' pushes surface automatically.
- **Merge center** — branches that still need to land on the base branch:
  remote tips (teammates' pushes) and local unpushed branches alike, each with
  ahead/behind, changed files, collapsible commit list, overlap warnings
  between branches, one-click merge into the base branch, block-level conflict
  resolution (ours/theirs/Manual + AI suggestions) and a final push banner.
  Auto-refresh option polls the remote every 20 s so the merge manager never
  misses a push.
- **Pull conflicts flow into the merge center** — pulling a divergent branch
  merges it locally; any conflicts are handed off to the same resolver UI.
- **Stash management** — save, restore, and delete stash entries (per-index)
  from the work tab when you need a clean tree before merging.
- **Team by email** — invite teammates through the backend; push events are
  delivered to every team member's inbox.
- **Sub-tool launcher** — configure external tools (VS Code, Cursor, tmux, etc.)
  and launch them directly at a repository from within the app.

## Install / build

### System dependencies (Debian/Ubuntu)

```bash
sudo apt install -y libwebkit2gtk-4.1-dev libssl-dev libgtk-3-dev \
  libayatana-appindicator3-dev librsvg2-dev build-essential curl wget file
```

### Rust toolchain

```bash
curl https://sh.rustup.rs -sSf | sh -s -- -y
rustup default stable
```

### Node + pnpm

Node ≥ 20 and `pnpm` are required for the frontend build.

```bash
npm install -g pnpm
```

### Tauri CLI

```bash
cargo install tauri-cli --version "^2.0"
```

### Build & run (development)

```bash
pnpm install
cargo tauri dev
```

### Build release binary

```bash
cargo tauri build --no-bundle
# binary lands at target/release/git-companion
```

After installing the binary, ensure it is on your `PATH` so the pre-push hook
can find it. See `docs/HOOKS.md`.

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
5. Start working: stage files, commit, push, pull.

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
- **UI (`ui/`)** — Vite + TypeScript + Tailwind v4. Views: Home (project
  registration), Repo (diff pane + commit/push/pull), Team (inbox), Settings
  (SSH profile + sub-tool launcher).
- **pre-push hook** — embedded in the binary via `include_str!`, installed to
  `<config_dir>/com.gitcompanion.app/hooks/pre-push`, symlinked into each
  registered repo's `.git/hooks/`. Fires `git-companion hook emit` for every
  pushed ref; the binary POSTs the event to the team backend via the fan-out
  API.
- **Team backend** — a separate FastAPI service. Members are invited by email.
  Push events are fan-out to all project members via long-poll.

## Security notes

- SSH private keys, SSH passwords, and SSH profile settings are stored in
  plaintext in `~/.config/com.gitcompanion.app/config.json`. Store only key
  files with restricted permissions (`chmod 600`); prefer SSH keys over
  passwords where possible.
- The `tauri.conf.json` CSP restricts `script-src` to `'self'`.
- All team backend communication is over HTTPS in production.

## Testing

```bash
cargo test --manifest-path src-tauri/Cargo.toml
```

40 tests covering: git log parsing, status porcelain v2, config schema
migration, hook emission, commit/push/pull operations (including the
pull-conflict → merge-resolver handoff), stash save/pop/drop by index, and the
merge center flow.

```bash
cd backend && python -m pytest -q
```

12 backend tests covering events, membership, and project management.

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
│   ├── lib/{ipc,format,app}.ts
│   ├── components/{Sidebar,RepositoryCard,TeamInbox,TeamPanel,
│   │              StatusTable,Toast}.ts
│   ├── views/{HomeView,RepoView,SettingsView}.ts
│   └── styles/{tokens,app}.css
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
│   │   ├── config_store.rs          # schema v4: Repository, SshProfile,
│   │   │                             # ExternalTool, AppSettings
│   │   ├── pre_push_hook.rs
│   │   ├── commands/{mod,repo,git,config,external,peer}.rs
│   │   ├── git/{mod,ops,branches,log,status}.rs
│   │   └── notify/{mod,store}.rs
│   └── tests/git_ops.rs
├── backend/                         # team backend (FastAPI)
│   ├── app/
│   │   ├── routes/{events,members,projects,devices}.py
│   │   ├── models.py
│   │   └── schemas.py
│   └── tests/
└── docs/{HOOKS,PEER}.md
```
