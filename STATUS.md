# 현재 상태

기준일: 2026-11-11
단계: **실시간 VST3 원본 편집창·정상 종료 후 상태 저장 구현, 이번 변경 CI 대기**

## 현재 묶음

- 기준 main `fbd5a9779381f62c370a8b43d2531504f5b31f9c`의 Audio #14 성공을 확인했다.
  중단됐던 보컬 VST3 하나의 worker 처리 경로가 이미 반영돼 있어 재구현하지 않았다.
- `run/scene run --vst3-editor`: 현재 보컬을 처리하는 동일 인스턴스의 원본 창을 연다.
  창 닫기는 editor만 분리하며 송출은 계속한다. 없음/열기 실패는 장치 시작 전에 실패한다.
- `--vst3-state-out`: 정상 종료 때만 새 상태 파일을 저장한다. 기존 파일은 보존한다.
  capture/playback 중지→worker join→editor 제거→최종 parameter flush→processing stop→
  state 취득→native 해제→파일 게시 순서다. 오류/암묵적 Drop에서는 저장하지 않는다.
- Win32 pump와 parameter flush는 기존 mixlive-vst3 코드를 공유한다. 새 패키지·lockfile·CI 변경 없음.
- editor/main 호출은 기존 plugin mutex를 worker와 공유한다. 장치 callback은 기다리지 않지만
  hard-real-time/상용 GUI 안정성은 보장하지 않는다. restart 요청은 기존처럼 중단하며
  preset/latency/bus 자동 재구성은 없다. AUX는 플러그인을 우회한다.

## 검증

- 수정한 editor/windows 기준 소스를 원격 blob과 대조했다. 현재 README는 Audio #14 artifact에서
  가져와 원격 blob과 일치함을 확인했다. 의존성 및 기존 장치 라우팅은 변경하지 않는다.
- 새/확장 검사: editor/state-out 인자, 기존 파일 보존, 없는 editor 거절,
  살아 있는 worker의 파라미터 변경 후 출력, 정상 종료 state flush·재로드·오류 시 저장 금지.
- 로컬 Rust 도구체인 없음. 새 실행 결과는 이번 Windows Actions에서 확인한다.
  GUI 창을 실제 열고 노브/DPI/닫힘을 확인하는 테스트는 이번 자동 검사에 포함되지 않는다.

## 남은 범위

- 실제 MiniFuse·상용 VST3의 GUI/청취·지연·장시간 안정성 수용.
- GPUI, 앱 직접 캡처, 자체 가상 장치, ASIO, 다중 plugin·지연 보상·전체 Scene.
- 자동 녹음·클라우드 전송·드라이버 설치·공식 릴리스 없음.

현재 상태는 이 파일, 이력은 Git, 실행 근거는 commit별 Actions를 따른다.
