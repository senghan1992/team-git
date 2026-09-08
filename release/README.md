# release — 설치 파일 공유 폴더

이 폴더에는 **빌드된 설치 파일**이 버전별로 들어갑니다. `git pull` 한 번으로
누구나 받아서 실행할 수 있습니다.

| 파일 | 내용 |
| --- | --- |
| `Git-Companion-<버전>-setup.exe` | Windows 설치 파일 (NSIS — 시작 메뉴 등록, 자동 업데이트 대응). **팀원이 받을 파일.** |
| `Git-Companion-<버전>.msi` | Windows MSI 패키지 (있을 때만 — 기업 배포용) |

## 설치 파일을 여기에 넣는 법 (Windows)

```powershell
pnpm tauri build
node dev/stage-installer.mjs

git add release/
git commit -m "release: v<버전> 설치 파일"
git push
```

`dev/stage-installer.mjs` 가 `src-tauri/target/release/bundle/` 아래의 빌드
결과물을 버전명이 들어간 이름으로 이 폴더에 복사하고, 실행할 git 명령까지
안내합니다.

## 팀원이 받는 법

```powershell
git pull
.\release\Git-Companion-0.1.6-setup.exe
```

기존 버전이 설치되어 있어도 그 위에 바로 설치됩니다 (설치기가 백그라운드
알림 프로세스를 먼저 정리합니다). 자세한 내용은 README 의
"설치 파일 공유하기 (배포)" 섹션을 참고하세요.