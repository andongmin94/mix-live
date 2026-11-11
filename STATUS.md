# 현재 상태

기준일: 2026-11-11
단계: **GPUI Scene 실행·라이브 라우팅 화면 구현, Windows 검증·개발 빌드 완료**

## 현재 구현

- `mixlive.exe`: Rust + GPUI + 기존 gpui-design의 네이티브 Scene 화면.
  파일 열기 → 저장 장치/채널/보컬/VST3 순서 요약 → 명시적 시작·정지 → 라이브 제어.
  파일 인자로 실행해도 자동 시작하지 않는다. Scene 열기는 장치나 DLL을 열지 않는다.
- 기존 `Prepared::new/start`, `Active::poll_plugin/report/finish`, `LiveControls`를 재사용한다.
  source/output/all mute, master/send gain, 각 소스→목적지 on/off와 음량이 실제 엔진에 적용된다.
  없는 AUX/send의 제어는 표시하지 않는다. 가짜 meter 없이 실제 엔진 진단/오류만 표시한다.
- 시작/정지/창 닫기의 native 호출은 GPUI foreground task에서 UI 객체 변경 참조 밖에서 수행한다.
  장치 중지 → worker join → plugin 해제 순서를 보존하며 재시작은 저장된 Scene 설정을 사용한다.
  runtime 오류도 같은 종료 경로로 정리하고 슬롯별 원인 메시지를 유지한다.
- GUI는 VST3 editor를 열지 않는다. 현재 CLI editor의 전역 Windows message pump를 GPUI update
  중에 재사용하면 재진입 위험이 있으므로 원본 창 통합은 다음 별도 작업이다. Scene 체인 처리는 지원한다.
- gpui-design `385b0178fbbaa3d30de79fe04329aab62a257dc0`, GPUI family `=0.3.11` 고정.
  Cargo가 잠금 파일을 생성했으며 기존 package version을 제거/업그레이드하지 않았다.
  Apache 고지 두 개를 보존하고 기존 단일 Windows CI에 앱 빌드/빈 창 열기·닫기를 포함했다.
- 오디오 경로는 WASAPI mic + optional mono/stereo AUX → 내장 보컬 DSP → 순차 stereo VST3 →
  목적지별 라우팅/음량 → mix 1~2개다. 공유 endpoint는 한 스트림, 별도 clock은 bounded queue와
  Rubato sinc resampler를 사용한다. AUX는 보컬 체인을 우회한다.
- AudioScene schema 3에 장치/보컬/ordered effects(path/class/state)를 담는다. 시작 설정이며,
  실행 중 route/mute/gain과 마지막 편집 상태를 전체 Scene에 저장하는 기능은 아직 없다.
  schema 1/2, DeviceScene, 단일 plugin launch state/class 옵션의 호환 경로는 없다.

## 검증

코드: `9421b5d03baff0085ff126977298a1f33bfc0d0e`.
[Audio #18](https://github.com/andongmin94/mix-live/actions/runs/42979977368),
Windows job `130387890712`: 모든 단계 success.

| 검사 | 결과 |
|---|---|
| Windows `cargo test --workspace --locked` | app 3 + audio lib 30 + audio CLI 3 + Scene CLI 3 + core 10 + effects 7 + plugins 7 = 63 passed |
| 기존 native VST3 fixture 별도 검사 | chain/worker/state 3 + editor 2 + loading/state 2 + WAV 2 = 9 passed |
| Windows Clippy all-targets, warnings denied | 통과 |
| Windows release 실행 파일 | `mixlive.exe`, `mixlive-audio.exe`, `mixlive-plugins.exe` 빌드 성공 |
| 기존 CLI 도움말 실행 | 통과 |
| 새 GUI 실행 | 장치/플러그인 없는 빈 네이티브 창 생성, 창 닫기 요청, 정상 종료 코드 0 확인 |

Windows tests 총 **72 passed, 0 failed**. 새 Session 검사는 파일 열기에서 장치/DLL에 접근하지
않음, 잘못된 새 선택의 이전 Scene 제거, 미실행 상태의 제어 거절을 확인한다. typed live control
검사는 실제 제어값과 mute/route/gain의 독립성을 확인한다. GUI 검사는 전체 클릭 시나리오나
실장비 시작/정지 검사가 아니다.

로컬 Linux/Rust 1.99.1에서도 관련 테스트 36개와 Windows MSVC 대상 앱 all-targets check,
앱/오디오 all-targets Clippy를 통과했다. 실제 Windows 실행 결과는 위 Actions를 근거로 한다.

[개발 artifact 13837513188](https://github.com/andongmin94/mix-live/actions/runs/42979977368/artifacts/13837513188)를
내려받아 ZIP SHA256/CRC와 파일 9개, 세 실행 파일의 x64 PE header를 확인했다.
SHA256 `ab93b952b03a5aa55fec01945981dbc52aecf5e94aed40f70301391bdea2fa22` — Actions digest와 일치.
공식 릴리스가 아닌 보관 기간 7일의 개발 artifact다. 검증 기록만 바꾸는 후속 commit은 코드를
바꾸지 않으므로 통과한 CI를 반복하지 않는다.

실제 MiniFuse/가상 케이블/상용 VST3 GUI·청취·왕복 지연·장시간 안정성은 미검증이다.
빈 GUI 창 또는 합성/native fixture 성공을 실제 장비/오디오/상용 호환성 검증으로 부르지 않는다.

## 남은 작업과 제한

- GUI의 장치 선택/Scene 생성, 보컬·체인 설정, 원본 editor 통합/재열기, 전체 실행 상태 저장.
- 앱별 직접 캡처, 자체 가상 장치, ASIO, plugin 지연 보상, 실행 중 체인 삽입/삭제/순서 변경,
  내장 공간계/음정 보정은 미구현이다.
- 각 VST3는 단일 active stereo 입출력만 지원한다. 모든 restart 요청은 경로 중단 사유다.
  in-process crash/hang과 control/worker mutex 위험이 남으며 native 시작/종료가 멈추면 GUI도
  기다릴 수 있다. hard-real-time이나 자동 복구를 보장하지 않는다.
- MiniFuse 모델과 WASAPI/ASIO/물리 단자/loopback 채널 대응은 추정하지 않는다.
  자동 녹음·클라우드 전송·드라이버 설치·서명·공식 릴리스 없음.

현재 상태는 이 파일, 이력은 Git, 실행 근거는 코드 commit별 Actions를 따른다.
