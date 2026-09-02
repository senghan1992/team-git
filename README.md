# Git Companion

한 프로젝트를 여러 사람이 git으로 함께 관리할 때, **각자 브랜치 → push →
병합 관리자가 병합 브랜치로 모으기 → 팀원 동기화**의 한 바퀴를 터미널 없이
돌릴 수 있게 만든 데스크톱 앱입니다.

전체 흐름과 화면별 사용법은 **[docs/WORKFLOW.md](docs/WORKFLOW.md)** 를 보세요.
데스크톱 앱을 빌드하지 않고 **브라우저에서 바로 보면서 작업**하려면
**[docs/PREVIEW.md](docs/PREVIEW.md)** — `pnpm seed:demo && pnpm dev:web` 두 줄이면 됩니다.

## 이 앱이 푸는 문제

| 문제 | 이 앱의 답 |
| --- | --- |
| 팀원이 push했는지 병합 관리자가 모른다 | push하면 **그 브랜치의 병합 관리자에게만** 알림이 간다. 홈 카드에 "N건 병합하기"로 남는다. |
| AI 에이전트를 쓰기 시작하니 어디가 어떻게 고쳐지는지 안 보인다 | 병합 탭의 **변경 지도** — 파일 기준으로 뒤집어, 여러 브랜치가 손댄 파일을 맨 위에 세우고, 충돌이 덜 나는 **병합 권장 순서**까지 제안한다. |
| 병합하다 충돌이 나면 거기서 멈춘다 | **AI 자동 병합** — 지침(프롬프트)을 미리 저장해 두면 충돌이 난 순간 알아서 고치고 병합 커밋까지 만든다. 원본은 항상 백업되고, 충돌 표시가 남은 파일은 절대 커밋되지 않으며, AI가 못 고친 파일을 통째로 한쪽만 남기지도 않는다(팀원의 커밋이 조용히 사라지지 않게). AI가 고친 병합은 관리자가 결과를 확인한 뒤에야 push된다. |
| 병합된 최신 코드를 내 브랜치에 가져오는 게 번거롭다 | 병합이 push되면 팀원 전원에게 알림 + **동기화** 버튼 한 번. |
| 병합이 끝난 브랜치가 origin에 쌓인다 | 병합 탭의 **브랜치 정리** — 커밋이 전부 병합 브랜치에 들어간 브랜치만 골라 origin에서 지운다. 병합 안 된 커밋이 있으면 거부한다. |
| 버튼이 많아서 뭘 눌러야 할지 모른다 | 홈의 저장소 카드가 상태를 읽어 **다음 할 일 하나**를 제안한다 (충돌 해결 → 병합 → 커밋 → 푸시 → 동기화 순). |

## 화면 구성

- **홈 (저장소)** — 등록된 저장소마다 브랜치·상태·**다음 할 일** 한 장.
- **저장소 → 작업** — 브랜치 전환, 새 브랜치 만들기, 상태 표(diff 미리보기,
  파일별 스테이징), 커밋 / 푸시 / 풀 / 스태시, 동기화.
- **저장소 → 병합** — 변경 지도, 병합 대기 브랜치 목록, 병합 실행,
  블록 단위 충돌 해결(나의 것 / 상대 것 / 직접 편집 / AI 제안), AI 자동 병합,
  백업 복원, 병합 후 push 배너.
- **저장소 → 설정** — `.gpconfig`: 병합 대상 브랜치, 브랜치별 병합 관리자, 구성원.
- **알림** — 받은 알림 + 알림 배달망 설정(팀·수신자).
- **설정** — **AI 자동 병합**, SSH 프로필/연결 테스트, 푸시 자격증명,
  외부 도구 목록. (도구 실행은 저장소 화면 오른쪽 위의 “열기” 버튼)

## 로그인 · 계정

**로그인은 선택입니다.** 저장소 등록·커밋·푸시·병합·충돌 해결·AI 자동 병합은
로그인 없이 그대로 동작하고, 팀원 push 알림과 구성원 검색만 계정이 필요합니다.
첫 화면에서 **저장소 열고 시작하기**를 누르면 바로 쓸 수 있습니다.

계정은 **팀 서버의 SQLite**(`users` 테이블)에 저장됩니다. 그래서 어느 컴퓨터에서든
같은 아이디로 로그인하고, 팀원 검색(`.gpconfig` 구성원 추가)도 이 컴퓨터에서
로그인한 적 있는 사람이 아니라 **팀 전체**에서 찾습니다.

- 비밀번호는 PBKDF2-HMAC-SHA256(솔트, 21만 회)으로 해싱해 저장합니다 —
  평문도, 단순 SHA-256도 저장하지 않습니다.
- 앱은 **로그인한 사람과 토큰만** 로컬에 캐시합니다. 그래서 재시작·오프라인에도
  로그인이 유지되고, 새 로그인·회원가입·프로필 변경은 서버가 필요합니다.
- 마이페이지(사이드바의 내 이름) — 프로필 수정, 비밀번호 변경, 로그아웃,
  다른 계정으로 로그인, 회원 탈퇴.
- 서버 주소는 로그인 화면의 **서버 주소**에서 정합니다.

```bash
cd backend
python3.11 -m venv .venv && ./.venv/bin/pip install -e ".[dev]"
./.venv/bin/uvicorn app.main:app --port 8000
```

## 그 밖의 기능

- **기본 UI 언어는 한국어** — 모든 문구가 한국어로 표시됩니다.
- **SSH 저장소 지원** — 원격 서버의 저장소를 SSH(키 또는 비밀번호)로 등록하고,
  원격 디렉터리 브라우저로 경로를 찾아갈 수 있습니다.
- **병합 브랜치는 main이 아니어도 됩니다** — `.gpconfig`의 병합 대상 브랜치를
  `develop`, `release/1.0` 등으로 자유롭게 정할 수 있고, 알림 분류도 그 설정을
  따릅니다 (`main`이 하드코딩된 곳은 없습니다).
- **스태시 관리** — 병합 전에 작업 트리를 비워야 할 때 저장/복원/삭제.
- **외부 도구** — 저장소 화면의 **열기** 버튼으로 VS Code·Cursor·터미널 등을
  저장소 경로에서 바로 띄웁니다. 명령은 앱이 실행 중인 컴퓨터에서 돌아가므로
  SSH로 등록한 저장소에서는 버튼이 나타나지 않습니다.

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
npm install -g pnpm     # or: corepack enable pnpm
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

Rust 툴체인이나 WebKitGTK 없이 화면만 보려면 브라우저 미리보기를 쓰세요
(git 동작은 실제로 실행됩니다 — [docs/PREVIEW.md](docs/PREVIEW.md)):

```bash
pnpm seed:demo    # 데모 저장소 + 팀원 브랜치 3개 (한 번만)
pnpm dev:web      # 접속 주소를 출력합니다
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
> (`javascriptcoregtk-4.1`); see **System dependencies** above. `cargo test`
> cannot link without them.

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
│   │              MergeCenter,ProjectConfigPanel,TeamInbox,TeamPanel,
│   │              StatusTable,GitGraph,conflictParser,Modal,Toast}.ts
│   ├── views/{HomeView,RepoView,SettingsView}.ts
│   └── styles/{tokens,app}.css
├── dev/                             # browser preview + tooling
│   ├── git-bridge.ts                # Vite plugin: real git behind the app's IPC
│   ├── serve-web.mjs                # `pnpm dev:web` — vite behind a proxy path
│   ├── seed-demo.mjs                # `pnpm seed:demo` — demo repo + branches
│   └── run-ui-tests.mjs             # `pnpm test:ui`
├── docs/{PREVIEW,WORKFLOW,HOOKS,PEER}.md
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
│   │   ├── git/{mod,ops,branches,log,status,merge,auto,sync,fetch,push}.rs
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
