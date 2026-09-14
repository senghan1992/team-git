# release — 설치 파일 공유 폴더

이 폴더에는 **빌드된 설치 파일**이 버전별로 들어갑니다. `git pull` 한 번으로
누구나 받아서 실행할 수 있습니다.

| 파일 | 내용 |
| --- | --- |
| `Git-Companion-<버전>-setup.exe` | Windows 설치 파일 (NSIS — 시작 메뉴 등록, 자동 업데이트 대응). **팀원이 받을 파일.** |
| `Git-Companion-<버전>.msi` | Windows MSI 패키지 (있을 때만 — 기업 배포용) |

## 설치 파일을 여기에 넣는 법

Windows:

```powershell
pnpm tauri build
node dev/stage-installer.mjs
```

Linux 크로스빌드 (Windows 설치 파일 생성 가능):

```bash
pnpm tauri build --bundles nsis --target x86_64-pc-windows-gnu
node dev/stage-installer.mjs
```

`dev/stage-installer.mjs` 가 빌드 결과물을 버전명이 들어간 이름으로 이 폴더에
복사하고, 실행할 git 명령까지 안내합니다.

그 다음:

```
git add release/
git commit -m "release: v<버전> 설치 파일"
git push
```

> ⚠️ NSIS 설치기에는 메인 앱 + 백그라운드 리스너만 포함됩니다.
> 터미널에서 `git push` 할 때 팀 알림을 보내는 `git-companion` CLI 는
> 설치기에 들어있지 않으므로, PATH 등록이 필요한 경우 함께 배포해야 합니다.

## 팀원이 받는 법

```powershell
git pull
.\release\Git-Companion-0.1.6-setup.exe
```

기존 버전이 설치되어 있어도 그 위에 바로 설치됩니다 (설치기가 백그라운드
알림 프로세스를 먼저 정리합니다). 자세한 내용은 README 의
"설치 파일 공유하기 (배포)" 섹션을 참고하세요.