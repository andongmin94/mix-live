# 현재 상태

기준일: 2026-11-13
단계: **GPUI 보컬 설정·VST3 체인 편집 구현, Windows 통합 검증 대기**

## 현재 구현

- `mixlive.exe`: 새 Scene → 실제 WASAPI 장치/채널·보컬·VST3 선택 → 새 파일 저장 →
  명시적 시작·정지 → 라이브 음소거·음량·라우팅. 기존 Scene 열기도 유지한다.
- 새 Scene과 정지한 Scene의 **보컬 설정**에서 같은 편집 화면을 쓴다. 내장 preset,
  입력 trim, dry/wet, 컴프레서/디에서 threshold와 VST3 추가·순서 변경·삭제를 지원한다.
  기존 Scene 편집은 장치·채널·AUX 목적지·출력 음량·queue 등 나머지 설정을 보존한다.
- VST3 파일/Windows bundle 폴더를 선택하거나 기존 체인 JSON을 목록 뒤에 추가한다.
  같은 DLL도 별도 슬롯이며, class/state가 슬롯과 함께 이동한다. 체인 가져오기 실패는
  기존 목록을 바꾸지 않는다. 상대 경로는 원래 체인 파일 기준이다. DLL은 시작할 때만 연다.
- 저장은 기존 schema 3/원자적 새 파일 계약을 사용하며 덮어쓰지 않는다. 편집/대화상자 취소와
  저장 실패는 이전 Scene을 유지한다. 저장 성공도 자동 시작하지 않으며 실행 중 편집은 막는다.
- 장치 목록은 정확한 ID·이름·기본 채널/샘플레이트/형식 또는 오류를 표시한다. 자동 장치 선택 없음.
  새 Scene을 다시 열면 목록을 갱신한다. 채널 범위/겹침, AUX 목적지, 보컬 설정 검증을 재사용한다.
  기존 Scene의 보컬 편집은 장치를 조회하지 않고 저장하며, 실제 장치는 시작 때 재검사한다.
- `Prepared::new/start`, `Active::poll_plugin/report/finish`, `LiveControls`를 재사용한다.
  native 조회/시작/종료는 GPUI foreground의 UI 객체 변경 참조 밖에서 실행한다.
  장치 중지 → worker join → plugin 해제 순서를 보존하며 가짜 meter를 표시하지 않는다.
- gpui-design `385b0178fbbaa3d30de79fe04329aab62a257dc0`, GPUI family `=0.3.11` 유지.
  기존 Select/Input/Button/ScrollArea를 재사용한다. 긴 체인의 버튼도 키보드 포커스를 받으면
  작업 영역 안으로 스크롤한다. 새 외부 의존성·Cargo.lock 변경은 없다.
- 엔진은 WASAPI mic + optional mono/stereo AUX → 내장 DSP → 순차 stereo VST3 → mix 1~2개다.
  공유 endpoint는 한 스트림, 별도 clock은 bounded queue/Rubato sinc bridge를 사용한다.
  AUX는 보컬 체인을 우회한다. 엔진/파일 형식/CLI 동작은 이번 변경에서 바꾸지 않았다.
- AudioScene schema 3은 시작 설정이다. schema 1/2, DeviceScene, 단일 plugin launch state/class
  옵션의 호환 경로는 없다. GUI 체인 추가는 현재 체인 파일 계약을 재사용한다.

## 검증

이번 변경의 로컬 Linux/Rust 1.100.0: 앱 관련 검사 **11 passed**,
Windows MSVC 대상 앱 all-targets Clippy(`-D warnings`) 통과.
새 검사는 기존 Scene의 전체 설정 보존, 보컬 값 검증, 중복 DLL 슬롯의 독립 class/state,
순서 변경/삭제, 체인 추가 실패의 원자성, 새 Scene·편집 Scene의 저장/재열기를 확인한다.

기존 단일 Windows Actions의 workspace/native fixture/Clippy/release와 GUI 검사를 실행한다.
최종 Windows 결과는 해당 코드 commit의 Actions 완료 후 기록한다. 이전 배포 코드
`48a90d7836a38058ba2c02a71227827b919ed0cc`는
[Audio #19](https://github.com/andongmin94/mix-live/actions/runs/42995739629)에서 Windows tests
78개, Clippy, 실행 파일 3개 빌드, 새 Scene 미선택 오류/취소/정상 종료가 통과했다.

## 남은 작업과 제한

- GUI의 기존 Scene 장치/라우팅 편집, 원본 VST3 editor 통합/재열기, 전체 실행 상태 저장.
  실행 중 mute/gain/route는 이번 실행에만 적용되며 보컬 편집은 저장된 시작 설정을 대상으로 한다.
- 실제 MiniFuse/가상 케이블/상용 VST3 GUI·청취·왕복 지연·장시간 안정성은 미검증이다.
  자동 검사나 native fixture 통과를 실장비/음질/상용 호환성 검증으로 부르지 않는다.
- 앱별 직접 캡처, 자체 가상 장치, ASIO, plugin 지연 보상, 실행 중 체인 삽입/삭제/순서 변경,
  내장 공간계/음정 보정은 미구현이다. 각 VST3는 단일 active stereo 입출력만 지원한다.
- 모든 restart 요청은 경로 중단 사유이며 in-process crash/hang과 control/worker mutex 위험이
  남는다. native 시작/종료가 멈추면 GUI도 기다릴 수 있다. hard-real-time/자동 복구 보장 없음.
- MiniFuse 모델과 WASAPI/ASIO/물리 단자/loopback 번호 대응은 추정하지 않는다.
  개인 장치 ID/녹음/사용자 상태는 공개 저장소에 넣지 않으며 자동 업로드·드라이버 설치·서명·릴리스 없음.

현재 상태는 이 파일, 이력은 Git, 실행 근거는 코드 commit별 Actions를 따른다.
