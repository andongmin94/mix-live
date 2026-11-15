# 현재 상태

기준일: 2026-11-15
단계: **GPUI 기존 Scene 장치·출력 편집 구현 · Windows 통합 검증 대기**

## 현재 구현

- `mixlive.exe`: 새 Scene 또는 기존 Scene 열기 → 설정 → 새 파일 저장 → 명시적 시작·정지 →
  실제 라이브 음소거·음량·라우팅. 열기·편집·저장만으로 오디오를 시작하지 않는다.
- 정지한 Scene의 **장치·출력 설정**은 현재 WASAPI 목록을 조회하고 저장된 mic/AUX/주 출력/
  추가 송출 ID·채널·AUX 목적지·시작 음량·queue 값을 불러온다. 새 Scene과 같은 폼을 사용한다.
  세 시작 음량(전체/추가 송출/AUX)과 장치별 queue 목표를 새 Scene에서도 편집할 수 있다.
- 폼을 열어도 보컬·VST3 path/class/state·비활성 AUX/send 채널과 음량을 보존한다.
  없는 장치/지원 불가 설정은 저장 ID·채널과 이유를 표시하고 저장을 거절한다. 실제 catalog는
  바꾸지 않으며 다른 장치나 사용 안 함으로 전환하지 않는다. 교체·optional 제거는 직접 선택한다.
- 장치를 바꿀 때만 mono/stereo 채널 시작값을 제안한다. 실제 ID·방향·채널 범위·공유 endpoint
  겹침·AUX 목적지·보컬 sample rate·음량/queue 범위 검증은 기존 경로를 재사용한다.
- **보컬 설정**은 장치를 조회하지 않고 내장 preset/trim/dry-wet/threshold와 VST3를 편집한다.
  파일/Windows bundle 폴더/체인 JSON 추가, 순서 변경·삭제를 지원하며 동일 DLL도 독립 슬롯이다.
  slot의 class/state가 함께 이동하고 상대 경로는 원래 파일 기준이다. DLL은 시작할 때만 연다.
- schema 3의 원자적 새 파일 저장을 사용한다. 덮어쓰기 없음, 취소/저장 실패는 이전 Scene 유지,
  성공은 새 파일 선택 후 명시적 시작 대기다. 실행 중 설정 편집은 막는다.
- native 장치 조회/시작/종료는 GPUI foreground의 UI 객체 변경 참조 밖에서 실행한다.
  `Prepared::new/start`, `Active::poll_plugin/report/finish`, `LiveControls`와
  장치 중지 → worker join → plugin 해제 순서를 유지한다. 가짜 meter는 없다.
- gpui-design 최신 `385b0178fbbaa3d30de79fe04329aab62a257dc0`, GPUI family `=0.3.11` 유지.
  기존 Select/Input/Button/ScrollArea를 재사용하며 새 의존성·Cargo.lock 변경은 없다.
- 엔진: WASAPI mic + optional mono/stereo AUX → 내장 DSP → 순차 stereo VST3 → mix 1~2개.
  공유 endpoint는 한 스트림, 별도 clock은 bounded queue/Rubato sinc bridge다. AUX는 보컬 체인 우회.
  이번 묶음은 엔진·CLI·파일 형식을 바꾸지 않는다. schema 1/2·DeviceScene 호환 경로는 없다.

## 검증

이번 로컬 환경은 Linux/Rust 1.100.0이며 실제 Windows 런타임은 없다.
앱 검사 **15 passed**: 전체 저장 설정 보존(비활성 채널 포함), 장치/시작 음량 편집 후 파일
왕복과 보컬·VST3 상태 보존, 누락 optional 장치의 명시적 유지/제거, 잘못된 음량/queue 거절을
추가했다. 장치 catalog는 합성이며 장치나 DLL을 열지 않는다.
Windows MSVC 대상 앱 all-targets Clippy(`--locked`, `-D warnings`)도 통과했다.

기존 단일 Windows CI에 실제 GUI의 장치 편집 검사를 추가했다. 합성 저장 ID 네 개와 채널,
시작값을 보존하는지, 누락 장치 저장을 거절하는지, 변경 후 취소·재열기가 원래 값을 유지하는지
확인한다. 기존 보컬 편집·재정렬·native 새 파일 저장·정상 종료 검사는 유지했다.
이번 코드의 Windows workspace/native fixture/Clippy/release/GUI 실행 결과는 대기 중이다.

## 남은 작업과 제한

- GUI 원본 VST3 editor 통합/재열기, 전체 실행 상태 저장은 남아 있다. Scene은 시작 설정이며
  실행 중 mute/gain/route와 최신 native plugin 편집 상태의 세션 스냅샷이 아니다.
- 실제 MiniFuse/가상 케이블/상용 VST3 GUI·청취·왕복 지연·장시간 안정성은 미검증이다.
  실제 장치 교체 후 GUI 저장·시작도 실장비에서 확인해야 한다. 자동 검사/native fixture를
  실장비·음질·상용 호환성 검증으로 부르지 않는다.
- 앱별 직접 캡처, 자체 가상 장치, ASIO, plugin 지연 보상, 실행 중 체인 삽입/삭제/순서 변경,
  내장 공간계/음정 보정은 미구현이다. 각 VST3는 단일 active stereo 입출력만 지원한다.
- 모든 restart 요청은 경로 중단 사유다. in-process crash/hang과 control/worker mutex 위험이
  남으며 native 시작/종료가 멈추면 GUI도 기다릴 수 있다. hard-real-time/자동 복구 보장 없음.
- MiniFuse 모델과 WASAPI/ASIO/물리 단자/loopback 번호 대응을 추정하지 않는다.
  개인 장치 ID/녹음/사용자 plugin 상태는 공개하지 않는다. 드라이버 설치·서명·공식 릴리스 없음.

현재 상태는 이 파일, 이력은 Git, 실행 근거는 코드 commit별 Actions를 따른다.
