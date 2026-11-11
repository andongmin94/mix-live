# 현재 상태

기준일: 2026-11-11
단계: **GPUI 장치 선택·새 Scene 저장 구현, Windows 통합 검증 대기**

## 현재 구현

- `mixlive.exe`: 새 Scene → 실제 WASAPI 장치/채널·내장 보정 선택 → 새 파일 저장 →
  명시적 시작·정지 → 라이브 음소거·음량·라우팅. 기존 Scene 열기도 유지한다.
- 장치 목록은 정확한 endpoint ID와 이름, 기본 채널/샘플레이트/형식 또는 오류를 표시한다.
  입력/출력을 구분하며 이름이 같은 장치도 ID로 선택한다. 기본 장치를 자동 선택하지 않는다.
  목록 조회는 스트림/DLL을 열지 않고, 새 Scene을 다시 열면 목록을 갱신한다.
- mic/주 출력은 필수, AUX/추가 송출은 선택이다. 실제 채널 범위, 같은 endpoint의 채널 겹침,
  AUX 목적지, 보컬 sample rate를 기존 검증으로 확인한다. 새 Scene은 VST3 없이 저장한다.
- 저장은 기존 `AudioScene::save_new`의 schema 3/원자적 새 파일 계약을 사용한다. 덮어쓰기 없음.
  취소/실패는 이전 Scene을 유지하고 성공해도 자동 시작하지 않는다. 시작 시 실제 장치를 재검사한다.
- `Prepared::new/start`, `Active::poll_plugin/report/finish`, `LiveControls`를 재사용한다.
  native 조회/시작/종료는 GPUI foreground의 UI 객체 변경 참조 밖에서 실행한다.
  장치 중지 → worker join → plugin 해제 순서를 보존하며 가짜 meter를 표시하지 않는다.
- gpui-design `385b0178fbbaa3d30de79fe04329aab62a257dc0`, GPUI family `=0.3.11` 유지.
  Select/Input을 기존 Button/Switch/Slider/Root와 함께 재사용한다. Cargo가 갱신한 lock 변경은
  앱의 기존 workspace `mixlive-effects` 의존성 한 항목뿐이며 외부 package version 변경은 없다.
- 엔진은 WASAPI mic + optional mono/stereo AUX → 내장 DSP → 순차 stereo VST3 → mix 1~2개다.
  공유 endpoint는 한 스트림, 별도 clock은 bounded queue/Rubato sinc bridge를 사용한다.
  AUX는 보컬 체인을 우회한다. GUI 생성 외의 체인 처리·CLI 기능은 기존과 같다.
- AudioScene schema 3은 시작 설정이다. schema 1/2, DeviceScene, 단일 plugin launch state/class
  옵션의 호환 경로는 없다. 기존 Scene 파일의 ordered effects와 state는 그대로 읽는다.

## 검증

이번 변경의 로컬 `cargo test -p mixlive-app --locked`: **6 passed**.
정확한 ID/방향, 기본값 일치, 실제 채널 범위/겹침, AUX 목적지, 보컬 sample rate,
새 Scene 저장/재열기를 검사했다. 실제 장치/DLL을 열지 않았다.
로컬 Windows MSVC 대상 앱/오디오 all-targets Clippy(`-D warnings`)도 통과했다.
기존 단일 Windows Actions에서 workspace/native fixture/Clippy/release를 통합 검증한다.
GUI smoke는 실제 UIAutomation으로 새 Scene 열기, 장치 미선택 오류, 취소, 정상 종료를 확인한다.
새 Session 검사는 장치 없는 저장/선택, 파일 충돌 시 파일·이전 선택 보존, 자동 시작 없음을 확인한다.

이전 배포 코드 `9421b5d03baff0085ff126977298a1f33bfc0d0e`는
[Audio #18](https://github.com/andongmin94/mix-live/actions/runs/42979977368)에서 Windows 검사
72개, Clippy, release 실행 파일 3개, 빈 GUI 창 생성/닫기가 통과했다. 이번 변경의 결과는
해당 코드 commit의 Actions 완료 후 이 파일에 기록한다.

## 남은 작업과 제한

- GUI의 기존 Scene/보컬 세부/체인 편집, 원본 editor 통합/재열기, 전체 실행 상태 저장.
  실행 중 mute/gain/route는 이번 실행에만 적용되며 저장된 시작 설정을 덮어쓰지 않는다.
- 실제 MiniFuse/가상 케이블/상용 VST3 GUI·청취·왕복 지연·장시간 안정성은 미검증이다.
  자동 검사나 native fixture 통과를 실장비/음질/상용 호환성 검증으로 부르지 않는다.
- 앱별 직접 캡처, 자체 가상 장치, ASIO, plugin 지연 보상, 실행 중 체인 삽입/삭제/순서 변경,
  내장 공간계/음정 보정은 미구현이다. 각 VST3는 단일 active stereo 입출력만 지원한다.
- 모든 restart 요청은 경로 중단 사유이며 in-process crash/hang과 control/worker mutex 위험이
  남는다. native 시작/종료가 멈추면 GUI도 기다릴 수 있다. hard-real-time/자동 복구 보장 없음.
- MiniFuse 모델과 WASAPI/ASIO/물리 단자/loopback 번호 대응은 추정하지 않는다.
  개인 장치 ID/녹음/사용자 상태는 공개 저장소에 넣지 않으며 자동 업로드·드라이버 설치·서명·릴리스 없음.

현재 상태는 이 파일, 이력은 Git, 실행 근거는 코드 commit별 Actions를 따른다.
