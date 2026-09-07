; NSIS 설치/삭제 훅 — tauri.conf.json 의 bundle.windows.nsis.installerHooks 가
; 이 파일을 installer.nsi 에 삽입한다.
;
; 배경: 이 앱은 백그라운드 피어 리스너(gc-peer-listener.exe)를 별도 프로세스로
; 띄운다. 예전 버전은 앱이 종료돼도 리스너가 그대로 남아 자기 자신의 exe 파일을
; 잠그므로, 새 버전 설치 때
;   Error opening file for writing ...\gc-peer-listener.exe
; 오류로 설치가 실패했다. 파일을 쓰기/지우기 전에 남은 리스너를 먼저 내려
; 덮어쓰기가 가능하게 한다.
;
; nsis_tauri_utils 는 tauri 번들러가 이미 설치기에 넣는 플러그인으로,
; 메인 앱 프로세스를 찾아 종료할 때 쓰는 것과 같은 함수다 (utils.nsh 의
; CheckIfAppIsRunning 과 동일한 메커니즘).
!macro NSIS_HOOK_PREINSTALL
  DetailPrint "Closing leftover gc-peer-listener..."
  !if "${INSTALLMODE}" == "currentUser"
    nsis_tauri_utils::KillProcessCurrentUser "gc-peer-listener.exe"
  !else
    nsis_tauri_utils::KillProcess "gc-peer-listener.exe"
  !endif
  Pop $0
  Sleep 300
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  DetailPrint "Closing leftover gc-peer-listener..."
  !if "${INSTALLMODE}" == "currentUser"
    nsis_tauri_utils::KillProcessCurrentUser "gc-peer-listener.exe"
  !else
    nsis_tauri_utils::KillProcess "gc-peer-listener.exe"
  !endif
  Pop $0
  Sleep 300
!macroend