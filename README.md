# Git Companion

한 프로젝트를 여러 사람이 git으로 함께 관리할 때, **각자 브랜치 → push →
병합 관리자가 병합 브랜치로 모으기 → 팀원 동기화**의 한 바퀴를 터미널 없이
돌릴 수 있게 만든 데스크톱 앱입니다.

```bash
git clone https://github.com/senghan1992/team-git.git
```

- **설치하기** → 아래 [설치 · 빌드](#설치--빌드) — 설치 파일 배포 없이 소스를 받아 직접 빌드합니다. 도구 설치부터 실행까지 **명령어를 순서대로 복사해 붙여넣기만** 하면 됩니다 (Windows / Linux / macOS).
- **사용법(스크린샷)** → 아래 [팀 6명이 시작하는 법](#팀-6명이-시작하는-법--화면으로-보는-사용법) — 실제 화면과 함께 역할별로.
- 전체 흐름과 화면별 사용법(상세) → **[docs/WORKFLOW.md](docs/WORKFLOW.md)**
- 빌드 없이 **브라우저에서 바로 보면서 작업** → **[docs/PREVIEW.md](docs/PREVIEW.md)** — `pnpm seed:demo && pnpm dev:web` 두 줄이면 됩니다.

## 이 앱이 푸는 문제

| 문제 | 이 앱의 답 |
| --- | --- |
| 팀원이 push했는지 병합 관리자가 모른다 | push하면 **그 브랜치의 병합 관리자에게만** 알림이 간다. 홈 카드에 "N건 병합하기"로 남는다. |
| AI 에이전트를 쓰기 시작하니 어디가 어떻게 고쳐지는지 안 보인다 | 병합 탭의 **변경 지도** — 파일 기준으로 뒤집어, 여러 브랜치가 손댄 파일을 맨 위에 세우고, 충돌이 덜 나는 **병합 권장 순서**까지 제안한다. |
| 병합하다 충돌이 나면 거기서 멈춘다 | **AI 자동 병합** — 지침(프롬프트)을 미리 저장해 두면 충돌이 난 순간 알아서 고치고 병합 커밋까지 만든다. 원본은 항상 백업되고, 충돌 표시가 남은 파일은 절대 커밋되지 않으며, AI가 못 고친 파일을 통째로 한쪽만 남기지도 않는다(팀원의 커밋이 조용히 사라지지 않게). AI가 고친 병합은 관리자가 결과를 확인한 뒤에야 push된다. |
| 병합된 최신 코드를 내 브랜치에 가져오는 게 번거롭다 | 병합이 push되면 팀원 전원에게 알림 + **동기화** 버튼 한 번. |
| 병합이 끝난 브랜치가 origin에 쌓인다 | 병합 탭의 **브랜치 정리** — 커밋이 전부 병합 브랜치에 들어간 브랜치만 골라 origin에서 지운다. 병합 안 된 커밋이 있으면 거부한다. |
| 버튼이 많아서 뭘 눌러야 할지 모른다 | 홈의 저장소 카드가 상태를 읽어 **다음 할 일 하나**를 제안한다 (충돌 해결 → 병합 → 커밋 → 푸시 → 동기화 순). |

## 팀 6명이 시작하는 법 — 화면으로 보는 사용법

역할은 두 가지뿐입니다: **병합 관리자 1명**(팀원들의 브랜치를 main으로 모으는
사람)과 **일반 팀원**. 아래 순서 그대로 따라 하면 됩니다. 더 깊은 내용은
[docs/WORKFLOW.md](docs/WORKFLOW.md)에 다 있습니다.

### 0. 처음 열면 — 로그인은 선택

![첫 실행 화면](docs/images/01-welcome.png)

**저장소 열고 시작하기**를 누르면 커밋·푸시·병합 전부 바로 쓸 수 있습니다.
로그인은 **팀 알림**(push 알림, 동기화 알림)에만 필요하니, 팀으로 쓸 거라면
관리자가 서버를 띄운 뒤 각자 **계정 만들기**로 가입하세요:

```bash
# 팀에서 한 명이, 모두가 접속할 수 있는 컴퓨터에서 (한 번만)
cd team-git/backend
python3 -m venv .venv && ./.venv/bin/pip install -e ".[dev]"
./.venv/bin/uvicorn app.main:app --host 0.0.0.0 --port 8000
```

로그인 창의 **서버 주소**에 그 컴퓨터 주소(`http://서버IP:8000`)를 넣습니다.
**이메일이 곧 팀 안의 신분**입니다 — 병합 관리자 지정이 이메일로 매칭되므로
팀에서 쓰는 이메일로 가입하세요.

### 1. 관리자가 한 번만 — 저장소 등록 + 팀 규칙

**+ 저장소 추가**로 프로젝트 폴더를 등록하고, 저장소의 **설정** 탭에서
구성원과 **브랜치별 병합 관리자**를 지정합니다. 이 규칙은 `.gpconfig`
파일로 **저장소 안에 커밋**되므로, 팀원 모두가 같은 규칙을 봅니다.

![저장소 설정 — 구성원과 병합 관리자](docs/images/06-config.png)

팀원들은 각자 `git clone` 받은 폴더를 똑같이 **+ 저장소 추가**로 등록하면
끝입니다 (폴더 이름이 서로 달라도 됩니다 — 앱이 원격 주소로 알아봅니다).

### 2. 팀원의 하루 — 브랜치 → 커밋 → 푸시

작업 탭에서 **새 브랜치**(예: `feature/로그인`)를 만들고 평소처럼 코딩합니다.
바뀐 파일이 상태 표에 나타나고, 파일 이름을 누르면 변경 내용(diff)을
미리 볼 수 있습니다. **커밋 → 푸시** 두 번이면 관리자에게 알림이 갑니다.

![작업 탭 — 상태 표와 커밋/푸시](docs/images/04-work.png)

> ⚠ **main에서 직접 작업하지 마세요.** 항상 내 브랜치를 만들어 작업하고,
> main은 병합 관리자만 만집니다. 홈 카드의 **다음 할 일**이 지금 해야 할
> 것을 알려 주니, 뭘 눌러야 할지 모르겠으면 홈으로 가면 됩니다.

### 3. 관리자의 하루 — 알림 받고 병합

팀원이 푸시하면 관리자의 홈 카드에 **"N건 병합하기"**로 쌓입니다.

![홈 — 다음 할 일](docs/images/02-home.png)

병합 탭의 **변경 지도**가 "누가 어느 파일을 고치고 있는지"를 파일 기준으로
보여 주고, 같은 파일을 여러 명이 고치면 **충돌이 덜 나는 병합 순서까지
제안**합니다. 파일 칩을 누르면 실제 변경(diff)을 보고 나서 병합할 수 있습니다.

![병합 탭 — 변경 지도와 권장 순서](docs/images/03-merge.png)

**main(으)로 병합**을 누르면 병합→push→팀원 전원에게 "동기화하세요" 알림까지
한 번에 진행됩니다.

### 4. 충돌이 나면 — 블록 단위로 고른다

충돌은 파일 전체가 아니라 **겹친 블록 단위**로 "내 것 / 가져온 것 / 직접
편집" 중에서 고릅니다. 아직 안 고른 블록에는 **미결정** 표시가 붙어 실수로
넘어갈 수 없습니다. 설정에서 AI를 켜 두면 **AI 자동 병합**이 저장해 둔
지침대로 알아서 고치고, 결과를 관리자가 확인한 뒤에만 push합니다.

![충돌 해결 — 블록 선택과 팀 알림](docs/images/05-conflict.png)

오른쪽 아래 알림처럼, 병합이 push되는 순간 팀원들에게는 **"내 브랜치에
동기화"** 버튼이 뜹니다 — 팀원은 그 버튼 한 번으로 최신 main을 자기
브랜치에 반영하고 작업을 계속합니다. 이게 한 바퀴입니다.

### 안전장치 — 작업은 사라지지 않습니다

- 커밋하지 않은 변경이 있으면 동기화·병합·브랜치 전환이 **먼저 거부**되고
  무엇을 하면 되는지 알려 줍니다 (조용히 덮어쓰는 일 없음).
- AI 자동 병합은 손대기 전에 **원본을 백업**하고, 충돌 표시가 남은 파일은
  절대 커밋하지 않으며, 확신이 없으면 사람에게 남깁니다.
- 검토한 뒤 팀원이 push를 더 했다면 병합이 멈추고 새로고침을 요구합니다.
- 브랜치 정리는 커밋이 전부 main에 들어간 브랜치만 지울 수 있습니다.
- 팀 서버가 꺼져 있어도 커밋·푸시·병합은 전부 정상 동작합니다 — 알림만
  보관됐다가 서버가 살아나면 자동 재전송됩니다.

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
  새로 만든(untracked) 파일도 함께 보관되고, 복원 중 충돌이 나면 스태시
  항목을 지우지 않은 채 알려 줍니다.
- **외부 도구** — 저장소 화면의 **열기** 버튼으로 VS Code·Cursor·터미널 등을
  저장소 경로에서 바로 띄웁니다. 명령은 앱이 실행 중인 컴퓨터에서 돌아가므로
  SSH로 등록한 저장소에서는 버튼이 나타나지 않습니다.

## 설치 · 빌드

이 앱은 설치 파일을 따로 배포하지 않습니다 — **소스를 받아 내 컴퓨터에서 직접
빌드**합니다. 아래 명령을 위에서부터 순서대로 복사해 붙여넣기만 하면 됩니다.
개발 도구가 하나도 없는 컴퓨터 기준으로 도구 설치 30분 + 첫 빌드 10~20분이
걸리고, 두 번째 빌드부터는 몇 분이면 끝납니다.

Tauri는 크로스 컴파일을 지원하지 않으므로 **쓸 OS에서 빌드합니다** —
Windows용 `.exe`는 Windows에서, Linux용은 Linux에서 만듭니다.

### Windows에서 빌드하기 (.exe)

#### 1단계 — 도구 설치 (컴퓨터당 한 번만)

**PowerShell**을 열고(시작 메뉴에서 "PowerShell" 검색) 아래 네 줄을 한 줄씩
붙여넣으세요. 중간에 설치 동의 창이 뜨면 진행하면 됩니다.

```powershell
winget install --id Git.Git -e
winget install --id Rustlang.Rustup -e
winget install --id OpenJS.NodeJS.LTS -e
winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

- 마지막 줄(C++ 빌드 도구)은 몇 GB를 내려받아 **가장 오래 걸립니다**. 끝날 때까지 기다리세요.
- `winget`이 없는 오래된 Windows라면 아래에서 직접 내려받아 설치하세요
  (빌드 도구는 설치 화면에서 **"C++를 사용한 데스크톱 개발"** 워크로드에 체크):
  - Git for Windows: <https://git-scm.com/download/win>
  - Rust: <https://rustup.rs>
  - Node.js LTS: <https://nodejs.org>
  - Visual Studio Build Tools 2022: <https://visualstudio.microsoft.com/ko/visual-cpp-build-tools/>

설치가 끝나면 **PowerShell 창을 닫고 새로 여세요** (방금 설치한 도구들이
인식되려면 새 창이어야 합니다). 그리고 확인:

```powershell
git --version
cargo --version
node --version
```

세 줄 모두 버전 번호가 나오면 준비 끝입니다.

#### 2단계 — pnpm과 Tauri CLI 설치 (컴퓨터당 한 번만)

```powershell
npm install -g pnpm
cargo install tauri-cli --version "^2.0"
```

두 번째 줄은 Tauri 빌드 도구를 컴파일하느라 5~10분 걸립니다. 한 번만 하면 됩니다.

#### 3단계 — 소스 받고 빌드

```powershell
git clone https://github.com/senghan1992/team-git.git
cd team-git
pnpm install
cargo tauri build
```

첫 빌드는 10~20분 걸립니다 (Rust가 의존성 전부를 컴파일합니다). 끝에
`Finished` 와 번들 경로가 출력되면 성공입니다.

#### 4단계 — 실행

설치해서 쓰려면 (시작 메뉴에 등록됨):

```powershell
& ".\target\release\bundle\nsis\Git Companion_0.1.0_x64-setup.exe"
```

설치 없이 바로 실행해 보려면:

```powershell
& ".\target\release\git-companion.exe"
```

- 처음 실행할 때 파란 **"Windows의 PC 보호"** 창이 뜨면 **추가 정보 → 실행**을
  누르세요 (서명되지 않은 앱이라 뜨는 정상 경고입니다).
- 팀 push 알림까지 쓰려면 훅이 앱을 찾을 수 있어야 합니다. 아래 한 줄이면 됩니다
  (실행 후 새 터미널부터 적용):

```powershell
setx GIT_COMPANION_BIN "$PWD\target\release\git-companion.exe"
```

#### 문제가 생기면

| 증상 | 해결 |
| --- | --- |
| `'cargo'(또는 git, node, pnpm)은(는) … 인식되지 않습니다` | PowerShell 창을 닫고 새로 여세요. 그래도 안 되면 해당 도구를 1·2단계대로 다시 설치. |
| `error: linker 'link.exe' not found` / `failed to find tool "lib.exe"` | C++ 빌드 도구가 없다는 뜻 — 1단계의 마지막 `winget` 명령(Build Tools)을 다시 실행하고 끝까지 기다리세요. |
| `cargo tauri build`에서 `no such command: tauri` | 2단계의 `cargo install tauri-cli --version "^2.0"`이 안 끝났거나 실패한 것 — 다시 실행. |
| 빌드가 20분 넘게 걸린다 | 첫 빌드는 원래 깁니다. 멈춘 게 아니라 컴파일 중이면 그대로 두세요. |
| 앱은 뜨는데 저장소 등록·커밋이 전부 실패 | Git for Windows가 없는 컴퓨터입니다 — `git --version`으로 확인하고 설치하세요. |
| SSH 저장소에 비밀번호 인증이 안 됨 | Windows에서는 SSH **키 인증만** 지원합니다 (`sshpass` 없음). |

### Linux에서 빌드하기 (Debian/Ubuntu)

터미널에 순서대로 붙여넣으세요:

```bash
# 1) 시스템 의존성 (한 번만)
sudo apt update && sudo apt install -y libwebkit2gtk-4.1-dev libssl-dev \
  libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev \
  build-essential curl wget file git

# 2) Rust + Node 22 + pnpm + Tauri CLI (한 번만)
curl https://sh.rustup.rs -sSf | sh -s -- -y && . "$HOME/.cargo/env"
curl -fsSL https://deb.nodesource.com/setup_22.x | sudo -E bash - && sudo apt install -y nodejs
sudo npm install -g pnpm
cargo install tauri-cli --version "^2.0"

# 3) 소스 받고 빌드
git clone https://github.com/senghan1992/team-git.git
cd team-git
pnpm install
cargo tauri build --no-bundle

# 4) 실행 (pre-push 알림 훅이 찾을 수 있게 PATH에 링크)
sudo ln -sf "$PWD/target/release/git-companion" /usr/local/bin/git-companion
git-companion
```

`.deb`/`AppImage` 패키지가 필요하면 `--no-bundle`을 빼고 `cargo tauri build`
하면 `target/release/bundle/` 아래에 생깁니다.

### macOS에서 빌드하기

```bash
# 1) 도구 (한 번만) — Homebrew가 없으면 https://brew.sh 에서 먼저 설치
xcode-select --install
curl https://sh.rustup.rs -sSf | sh -s -- -y && . "$HOME/.cargo/env"
brew install node && npm install -g pnpm
cargo install tauri-cli --version "^2.0"

# 2) 소스 받고 빌드
git clone https://github.com/senghan1992/team-git.git
cd team-git
pnpm install
cargo tauri build     # .app / .dmg → target/release/bundle/
```

### 개발 모드로 실행

코드를 고치면서 바로 확인하려면:

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
