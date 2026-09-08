# push-alert — 앱 없이 push 해도 알림 받기

앱(또는 앱이 설치한 pre-push 훅)이 없는 팀원이 터미널·IDE로 `git push`만 해도
팀 서버에 자동으로 알림이 간다. 병합 관리자는 받은편지함에서 "병합 요청" 카드를
그대로 받고, 병합 대상 브랜치 push는 전 팀원에게 "동기화 안내"가 간다.

방법이 둘이다 (아래 각각 설명):

1. **웹훅 설정 (추천 — 저장소가 GitHub/GitLab/Gitea 에 있을 때)**
   저장소 설정 화면에서 웹훅 등록 몇 번으로 끝난다. 저장소에 코드를
   추가할 필요가 없다. → **아래 "GitHub / Gitea / GitLab 저장소" 섹션**
2. **post-receive 훅 (저장소를 우리 서버가 직접 가질 때)** — bash 훅 설치.
   → 아래 "동작 원리" ~ "설치" 섹션

```
팀원 PC (아무것도 없음)   원격 git 서버(GitHub/GitLab/자체서버)   팀 서버 (docker)
git push ─→ (refs 갱신) ─→ 웹훅 또는 post-receive 훅 ─→ POST /events/server-hook ─→ 기기 폴링 → 앱 알림
```

## 동작 원리

- push 하는 사람의 PC에는 아무것도 설치·설정할 필요가 없다 — 감지는
  **원격 저장소가 있는 쪽**(GitHub/GitLab 의 웹훅, 또는 우리 서버의
  post-receive 훅)에서 한다.
- push는 절대 막지 않는다: 설정이 빠졌거나 팀 서버가 죽어 있어도 push는 그대로
  성공하고, 서버만 "못 보냄" 처리한다.
- 훅이 보내는 내용:
  - 작업 브랜치 push → `branch_push` (병합 관리자에게만 "병합 요청")
  - 병합 대상 브랜치(.gpconfig의 `merge_targets`/`default_base_branch`) push
    → `main_push` (전 팀원 "동기화 안내")
  - `v1.2.3` 형식 태그 push → `release` (전원)
- 팀원이 **앱도 쓰고 있고**, 자신이 터미널로 push한 경우 — 서버가 git
  `user.name`/`user.email`로 그 사람의 기기를 찾아 자기 알림은 빼준다.
- 같은 push가 앱 훅과 서버 훅 두 경로로 들어와도, sha 기준 중복 제거로
  카드는 한 장만 쌓인다.

## 설치 (원격 git 서버에서, 저장소마다 한 번)

저장소의 hooks 폴더에 파일 두 개를 넣는다.

```
# 1. 설정 파일 (push-alert.env.example 을 복사해 값을 채운다)
cp push-alert.env.example <원격저장소>/hooks/push-alert.env
vi   <원격저장소>/hooks/push-alert.env

# 2. 훅 본체
cp post-receive <원격저장소>/hooks/post-receive
chmod +x <원격저장소>/hooks/post-receive
```

- `<원격저장소>` 예: 베어 저장소라면 `/srv/git/team-app.git` (hooks/가 그
  안에 이미 있다). Gitea/Gitea Actions·GitLab 등이 자체 관리하는 저장소는
  아래 "GitHub/Gitea/GitLab 저장소" 참고.
- bare 저장소가 아니라 공유 작업복사본(비베어)이면 `hooks/` 폴더가 없으니
  `mkdir <원격저장소>/hooks`로 만든다. (훅의 `git show 브랜치:.gpconfig` 판정이
  베어든 비베어든 동작한다)

### 값 채우기

| 변수 | 설명 |
|---|---|
| `TEAM_SERVER_URL` | 팀 서버 주소 — 앱 로그인 화면에 넣는 그 주소. 예: `http://192.168.0.10:48111` |
| `HOOK_SECRET` | 팀 서버의 `GC_HOOK_SECRET` 환경변수와 **같은 값** (docker-compose.yml `environment`에 추가). 서버에 이 값이 없으면 이 훅은 동작하지 않는다 |
| `PROJECT_ID` | 이 저장소가 속한 팀 프로젝트의 id (아래 참고) |
| `REPO_NAME` | (선택) 저장소 이름. 기본값 = 베어 저장소 폴더 이름 |
| `REMOTE_URL` | (선택) 원격 주소. 채우면 앱이 remote URL로 저장소를 정확히 찾는다 — 팀원마다 폴더 이름이 달라도 알림이 같은 저장소로 모인다. 예: `git@server:team/team-app.git` |

`PROJECT_ID` 찾는 법 2가지:

```
# 방법 1 — 팀 서버 머신에서 (컨테이너 안 DB)
docker exec -it git-companion-team-server python -c \
  "import sqlite3;c=sqlite3.connect('/data/gc_peer.db');print([r for r in c.execute('select id, display_name from projects')])"

# 방법 2 — 앱이 등록된 기기 토큰으로 API 호출
curl -H "Authorization: Bearer <내기기토큰>" http://<서버IP>:48111/projects
```

## 잘 되는지 확인

서버에서 push 한 번 하고, 훅 로그를 본다:

```
cd /tmp/test-repo && echo x >> a.txt && git commit -am "test" && git push
# 서버 로그 (post-receive 가 남긴 것):
#   [push-alert] ...   ← 아무것도 없으면 성공
```

팀 서버 로그(`docker compose logs -f`)에 `POST /events/server-hook 200`이
찍히면 끝. 403이면 비밀키가 다름, 404면 프로젝트 id 오류다.

## 주의

- **`GC_HOOK_SECRET`이 팀 서버에 없으면** 이 훅은 팀 서버에서 404를 받지만
  push는 정상이다. 서버 환경변수부터 확인할 것.
- 베어 저장소에 `.gpconfig`가 아직 없으면(앱이 한 번도 저장소 설정을 저장하지
  않음) 훅은 bare 저장소의 기본 브랜치(HEAD)만 병합 대상으로 본다.
- post-receive 훅은 저장소 **하나마다** 설치해야 한다. 새 저장소를 만들면
  hooks/ 두 파일을 다시 복사한다.

## GitHub / Gitea / GitLab 저장소

자체 베어 저장소가 아니라 **GitHub(또는 Gitea·GitLab)에 두는 저장소**는 이
bash 훅 대신 그 서비스의 **웹훅(webhook) 설정**으로 끝낸다 — 저장소에 코드를
추가할 필요가 없다. 팀 서버가 그 서비스의 웹훅 형식을 그대로 받아서 처리한다
(팀 서버가 인터넷에서 접근 가능해야 한다 — 사내 GitLab 의 경우 사내망에서
팀 서버로 도달 가능해야 하고). 병합 대상 판정은 웹훅 URL 의 `merge_targets`
쿼리로 지정한다.

### GitHub

저장소 → **Settings → Webhooks → Add webhook** 에서:

| 항목 | 값 |
|---|---|
| Payload URL | `http://<팀서버>:48111/events/server-hook?project_id=<프로젝트id>&merge_targets=develop,release/1.0` |
| Content type | `application/json` |
| Secret | 팀 서버 `GC_HOOK_SECRET` 과 **같은 값** |
| Which events | **Just the push event** (기본값이면 충분) |

- `merge_targets` 는 선택 — 앱의 .gpconfig 병합 대상과 맞추면 기본 브랜치가
  아닌 `develop` 등을 병합 대상(동기화 안내)으로 인식한다. 쉼표로 여러 개.
- 저장 후 **"Test" 버튼(ping)** 을 누르면 팀 서버가 200으로 답한다. 팀 서버
  로그에 `POST /events/server-hook 200` 이 찍히면 연결 성공.
- "Recent Deliveries" 에 200 이 아닌 응답이 보이면 로그를 열어 원인 확인.

### GitLab (사내 mod.lge.com 등)

프로젝트 → **Settings → Webhooks** 에서:

| 항목 | 값 |
|---|---|
| URL | `http://<팀서버>:48111/events/server-hook?project_id=<프로젝트id>&merge_targets=develop,release/1.0` |
| Secret token | 팀 서버 `GC_HOOK_SECRET` 과 **같은 값** |
| Trigger | **Push events** 만 체크 |

- GitLab 은 URL 로 접근 가능한지 검사하니 먼저 확인한다 ("Test" 버튼이
  실패로 표시되면 팀 서버가 GitLab 에서 안 보이는 것 — 팀 서버 머신에서
  `curl http://<팀서버>:48111/healthz` 로 자기 확인도 가능).

### 동작 후 알림 분류

두 서비스 공통: `merge_targets` 에 있는 브랜치(없으면 저장소 기본 브랜치)로의
push → 전 팀원 "동기화 안내"(main_push), 그 외 브랜치 push → 병합 관리자
"병합 요청"(branch_push), `v1.2.3` 형식 태그 push → 릴리스 알림. 삭제 push 는
무시된다. 팀원 이름·이메일은 커밋 작성자 정보를 쓴다.

## 파일 구성

- `post-receive` — 본체 (bash + curl만 있으면 동작, python3 있으면 .gpconfig
  판정이 더 정확)
- `push-alert.env.example` — 설정 템플릿