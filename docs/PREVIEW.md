# 브라우저로 보면서 작업하기

Tauri 앱이지만 화면 전체가 웹(Vite + TypeScript)이라, **데스크톱 앱을 빌드하지
않고 브라우저에서 그대로 보고 클릭할 수 있다.** git 동작도 흉내가 아니라 진짜다:
`dev/git-bridge.ts`(Vite 개발 서버 플러그인)가 Rust 쪽과 같은 IPC 명령을 받아
실제 `git` 을 실행한다.

## 4줄 요약

```bash
pnpm install
cd backend && python3.11 -m venv .venv && ./.venv/bin/pip install -e ".[dev]"
./.venv/bin/uvicorn app.main:app --port 8000 &   # 계정·알림 서버 (SQLite)
cd .. && pnpm seed:demo && pnpm dev:web          # 데모 데이터 + 접속 주소
```

로그인: `minji` / `minji-demo-pw` (병합 관리자) · `junho` / `junho-demo-pw` (일반 팀원)

> 계정은 **팀 서버의 SQLite** 에 저장된다. `pnpm seed:demo` 가 서버가 떠 있으면
> 위 두 계정을 가입까지 시켜 준다. 서버를 먼저 띄워야 한다:
> `cd backend && ./.venv/bin/uvicorn app.main:app --port 8000`

## 어떤 주소로 열까

`pnpm dev:web` 이 알려 주지만, 상황별로 이렇게 갈린다.

### code-server(브라우저 VS Code) 안에서 작업 중일 때 — 기본

code-server 에는 포트 프록시가 내장되어 있다. **지금 code-server 를 보고 있는
탭의 주소창에서 도메인만 남기고** 경로를 바꾼다:

```
https://<code-server 도메인>/absproxy/5173/
```

예: `https://d1234abcd.cloudfront.net/absproxy/5173/`

로그인 쿠키를 이미 갖고 있는 브라우저라야 열린다(같은 브라우저의 새 탭이면 된다).

> `/proxy/5173/` 가 아니라 `/absproxy/5173/` 다. `/proxy/` 는 경로 접두사를
> 떼어내기 때문에 `/assets/...` 같은 절대경로 자산이 404 가 된다. `dev:web` 은
> Vite 의 `base` 를 `/absproxy/<포트>/` 로 맞춰서 띄운다.
>
> 끝 슬래시는 빼도 된다 — `/absproxy/5173` 로 들어오면 슬래시 붙은 주소로
> 리다이렉트한다.

### 로컬 PC 에서 SSH 로 접속할 때

```bash
GC_BASE=/ pnpm dev:web
```

그리고 내 PC 터미널에서 터널을 연다:

```bash
ssh -N -L 5173:127.0.0.1:5173 <이 서버>
```

브라우저에서 `http://localhost:5173/`.

### 포트가 외부로 열려 있을 때

```bash
GC_HOST=0.0.0.0 GC_BASE=/ pnpm dev:web
```

`http://<서버 IP>:5173/`. 보안 그룹/방화벽에서 그 포트를 열어 둬야 하고,
인증이 없는 상태로 노출되니 임시로만 쓴다.

### "Blocked request. This host is not allowed." 가 뜨면

Vite 5.4+ 는 DNS 리바인딩을 막기 위해 `Host` 헤더를 검사한다. 프록시를 거치면
`Host` 가 프록시 도메인(`*.cloudfront.net` 등)이 되어 걸린다.

`pnpm dev:web` 은 서버가 **루프백(127.0.0.1)에 바인드된 경우** 이 검사를
해제한다(`GC_ALLOWED_HOSTS=all`). 루프백이면 이 머신 안의 프록시만 접속할 수
있으므로 외부에 노출되지 않는다. 그래도 뜬다면 도메인을 직접 지정한다:

```bash
GC_ALLOWED_HOSTS=diehyb9eq4w2q.cloudfront.net pnpm dev:web
```

`GC_HOST=0.0.0.0` 으로 외부에 열었을 때는 자동 해제가 되지 않으므로 위처럼
도메인을 명시해야 한다.

## 환경변수

| 변수 | 기본값 | 뜻 |
| --- | --- | --- |
| `GC_PORT` | `5173` | Vite 포트 |
| `GC_BASE` | `/absproxy/<GC_PORT>/` | Vite base path. `/` 면 프록시 없이 |
| `GC_HOST` | `127.0.0.1` | 바인드 주소 |
| `GC_HMR_CLIENT_PORT` | 프록시 모드에서 `443` | 브라우저가 HMR 소켓에 붙을 포트 |
| `GC_HMR_PROTOCOL` | `wss` | `ws` 또는 `wss` |
| `GC_ALLOWED_HOSTS` | 루프백 바인드면 `all` | `all` 또는 쉼표로 구분한 호스트 목록 |

`pnpm dev` (프록시 없는 원래 스크립트)는 이 값들을 건드리지 않으므로 예전과
똑같이 동작한다.

## 데모 데이터

`pnpm seed:demo` 가 만드는 것:

- `~/gc-demo/origin.git` — 원격 역할을 하는 bare 저장소
- `~/gc-demo/demo-app` — `main` + 팀원 3명이 push한 브랜치
  (`feature/login`, `feature/payment`, `fix/nav`)
- 저장소에 커밋된 `.gpconfig` — 병합 대상 `main`, 병합 관리자 `minji@example.com`
- 앱 설정에 저장소 등록 + AI 자동 병합 켜기
- (팀 서버가 떠 있으면) **팀 알림 데모** — 이 앱의 기기를 서버에 등록해
  `demo-app 팀` 프로젝트를 만들고 저장소와 연결한 뒤, 가상 팀원 기기
  (`~/gc-demo/teammate_token`)가 병합 대기 브랜치 3개의 push 알림을 보낸다.
  알림은 **실제 배달 경로**(팀 서버 → 앱 폴링 → 수신함)로 도착한다.

그래서 열자마자 이 순서로 눌러 볼 수 있다:

1. 로그인하면 5초 안에 **우측 하단에 "… 브랜치가 병합을 기다립니다" 알림**과
   사이드바 **알림 배지**가 뜬다 — 알림 탭에 들어가지 않아도 된다.
   알림 탭에서는 카드별 **읽음 표시**·**모두 읽음**이 남는다 (새로 고쳐도 유지).
2. 홈 카드의 **다음 할 일: 3건 병합하기**
3. 병합 탭 맨 위의 **최근 7일 병합 흐름** — 브랜치들이 언제 작업되어 `main` 에
   어떻게 합쳐졌는지 시간축으로 보인다 (병합 대기 브랜치는 점선).
4. 병합 탭의 **변경 지도** — `src/api/user.ts` 를 `feature/login`(김민지)과
   `feature/payment`(박준호)가 같이 고치고 있다는 경고
5. `feature/login` 병합 → 깨끗하게 통과
6. `feature/payment` 병합 → **충돌** → 자동 해결이 켜져 있으므로 바로 실행됨
7. `junho / junho-demo-pw` 로 바꿔 로그인 → 같은 저장소가 팀원 시점(커밋/푸시/동기화)으로 보인다

팀원이 **지금** push 하는 상황을 보고 싶으면 (브라우저를 보고 있는 채로):

```bash
pnpm demo:push                                   # 박준호가 새 브랜치에 커밋 → push → 알림
pnpm demo:push -- --branch fix/typo --message "fix: 오타"
pnpm seed:demo -- --notify                       # 병합 대기 3건의 알림을 다시 보내기
```

```bash
node dev/seed-demo.mjs --reset   # 처음 상태로 되돌리기
node dev/seed-demo.mjs --clean   # 데모 삭제 + 등록 해제
```

> 미리보기 브릿지는 저장소 명령을 **워커 스레드 4개**에서 돌린다. 느린 SSH
> 저장소 하나가 다른 카드나 알림 폴링을 막지 않도록 — 실제 앱(Rust)도 커맨드를
> 스레드 풀에서 돌리므로 같은 체감이다. 홈 카드는 상태를 먼저 그리고 "병합 대기
> 확인 중…"만 뒤늦게 채운다.

## 알아 둘 점

- **AI 는 실제로 호출되지 않는다.** 브릿지에는 API 키가 없으므로 "AI 를 못 쓴
  상태"처럼 동작한다. 덕분에 안전 규칙(양쪽이 모두 고친 파일은 자동으로 한쪽을
  고르지 않고 사람에게 넘김)을 그대로 확인할 수 있다. 진짜 LLM 으로 보려면
  **설정 → AI 자동 병합**에 Base URL·모델·API 키를 채운다. 그건 Rust 앱에서만
  실제 요청이 나간다.
- **팀 알림(`peer_*`)은 목 데이터다.** FastAPI 백엔드(`backend/`)가 떠 있어야
  진짜로 동작한다. 브라우저에서는 `ui/_dev_shim.ts` 의 샘플 이벤트가 보인다.
- **화면 이동은 URL 에 남지 않는다.** 해시 라우팅이 없어서 새로고침하면 홈으로
  돌아간다. 특정 탭을 반복해서 보려면 잠시 `ui/lib/app.ts` 의
  `let page: Page = { kind: "home" }` 를 바꾸면 된다(저장하면 바로 반영).
- **파일을 저장하면 페이지가 새로 그려진다.** 이 앱은 HMR 핸들러가 없어서 Vite
  가 전체 리로드를 시킨다 — 상태가 초기화되지만 화면 확인에는 오히려 편하다.
  자동 리로드가 안 되면(웹소켓이 프록시를 못 통과하는 경우) 브라우저를 직접
  새로고침하면 된다.
- **Rust 쪽 변경은 이 미리보기에 반영되지 않는다.** 브릿지는 `dev/git-bridge.ts`
  의 TypeScript 구현이다. Rust 를 고쳤으면 브릿지에도 같은 규칙을 반영해야
  미리보기와 실제 앱이 같게 동작한다(현재 병합/충돌/자동 해결/`.gpconfig` 읽기
  규칙은 양쪽이 맞춰져 있다).
