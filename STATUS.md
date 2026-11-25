# 현재 상태

기준일: 2026-11-25
단계: **앱 프로세스 트리 직접 AUX 캡처 구현 · Windows 통합 검증 대기**

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
- 실행 중 정지 후 새 Scene 저장은 경로를 먼저 선택하고, 확정 후 장치 중지·worker join·모든 VST3 상태 수집을 수행한다.
  현재 mute, master/send gain, 네 연결과 gain을 다음 시작 전에 복원한다. 일반 정지/닫기는 변경을 버린다.
  실패 시 이전 Scene 선택 유지, 기존 파일 덮어쓰기 없음. 저장된 믹서 값은 보컬·장치 편집에서도 보존한다.
- 실행 중 각 VST3 슬롯에서 원본 창 열기/닫기·재열기를 지원한다. 기존 인스턴스를 사용하며
  창을 열 수 없으면 해당 요청을 거절하고, 종료/저장 시 기존 native teardown을 사용한다.
  GPUI 메시지 루프에서 Win32 pump를 제거하고 native poll/창 조작은 UI 변경 참조 밖에서 실행한다.
  CLI는 자체 pump를 유지한다. 창 조작 시 기존 fader 엔티티를 유지한다.
- schema 5의 원자적 새 파일 저장을 사용한다. 덮어쓰기 없음, 취소/저장 실패는 이전 Scene 유지,
  성공은 새 파일 선택 후 명시적 시작 대기다. 실행 중 설정 편집은 막는다.
- native 장치 조회/시작/종료는 GPUI foreground의 UI 객체 변경 참조 밖에서 실행한다.
  `Prepared::new/start`, `Active::poll_plugin/report/finish`, `LiveControls`와
  장치 중지 → worker join → plugin 해제 순서를 유지한다.
- 라이브 믹서에서 마이크 입력·내장 보정 후·보컬 체인 후·AUX 입력·각 출력 mix의 실제 sample peak를
  약 100ms마다 dBFS로 표시한다. 소스는 mute 전, 출력은 감쇠·mute·guard 후 장치 채널 매핑 전이다.
  기존 peak 누적을 공유 endpoint의 lane별로 분리했고 진단 문자열은 같은 읽기값을 사용한다.
  내장 gain reduction도 표시한다. 실측 장치·LUFS·true peak·청력 보호 검증은 아니다.
- gpui-deck `78c0898ccb6feba03e3a3243ac7d4729fd80aee6` (0.0.1), GPUI family `=0.3.12`로 갱신했다.
  기존 Select/Input/Button/ScrollArea를 유지하며 라이브 fader는 단일 값 SliderState와 SliderThumb을
  사용한다. Deck과 GPUI 계열의 잠금 파일은 Cargo로 갱신했다.
- AUX는 녹음 endpoint 또는 앱 실행 파일 경로 하나를 선택한다. 앱 root PID는 실행 때만
  조회하며 독립 루트 여러 개·자기 출력 포함 트리를 거절한다. 앱 캡처는 wasapi 0.27.0의
  include-process-tree 클라이언트와 별도 MTA 스레드, 고정 float32 stereo 48 kHz, 기존 AUX
  clock bridge를 쓴다. 전체 데스크톱 폴백·재생 장치 변경·재연결·녹음은 없다.
- 엔진: WASAPI mic + optional mono/stereo AUX → 내장 DSP → 순차 stereo VST3 → mix 1~2개.
  공유 endpoint는 한 스트림, 별도 clock은 bounded queue/Rubato sinc bridge다. AUX는 보컬 체인 우회.
  schema 5는 필수 route.mix를 포함한다. schema 1~4·DeviceScene 호환 경로는 없다.

## 검증

[Cargo 의존성 갱신](https://github.com/andongmin94/mix-live/actions/runs/45548210980) 성공:
`cargo update --workspace` 및 `cargo metadata --locked`, 생성된 Cargo.lock SHA256 일치.
GPUI Deck 0.0.1과 GPUI family 0.3.12만 사용하며 기존 엔진 패키지 버전은 유지했다.
이전 장치·보컬 편집 코드는 [Audio #26](https://github.com/andongmin94/mix-live/actions/runs/43690650750)에서
Windows 검사 87개, Clippy, release 빌드 및 실제 Scene 편집·저장 검사를 통과했다.
새 Deck revision은 [Audio #27](https://github.com/andongmin94/mix-live/actions/runs/45548374397)에서
Windows workspace, native fixture, Clippy, release 빌드 및 실제 Scene UI 검사를 모두 통과했다.
실행 상태 저장 코드 `851975d8670e7f4ed41d771b30be9cc9eb1127d6`의 [Audio #28](https://github.com/andongmin94/mix-live/actions/runs/45549620656):
Windows 검사 **88개**(workspace 79 + native VST3 fixture 9) 성공. Clippy의 `large_enum_variant`가
CLI Scene Save에 발생하여 release/GUI 검사는 실행되지 않았다.
`6290cf91c828bff2dd9762d1af62cb8b473a3e0e`에서 Save의 큰 RouteOptions만 Box로 두고
mix 설정은 인라인으로 유지하도록 수정했다. 린트 억제 없음.
수정 후 [Audio #29](https://github.com/andongmin94/mix-live/actions/runs/45550324761) 전체 성공:
workspace/native fixture, Clippy, release 빌드 및 실제 Scene 편집·저장 GUI 검사 통과.
원본 창 변경 [Audio #30](https://github.com/andongmin94/mix-live/actions/runs/45551519959)는
반복 poll의 이동된 WeakEntity(E0382)로 컴파일 실패했다. 이번 묶음에서 해당 소유권을 바로잡고
실제 peak 미터를 추가했다. 기존 instrumented synth fixture의 창 attach/detach·재열기 검사,
합성 shared endpoint lane peak 및 mute 검사와 dBFS 표시 검사를 포함한다.
[Audio #31](https://github.com/andongmin94/mix-live/actions/runs/45553328302)는
workspace/native fixture 테스트가 성공했으나 Clippy `items_after_test_module`로 중단됐다.
이번 묶음은 meter 테스트 모듈을 파일 끝으로 이동하고 앱 식별·캡처·설정 경로를 추가한다.
잠금 파일은 crates.io의 확인된 wasapi checksum 및 기존 잠금 의존성에 맞춰 갱신했다.
기존 Windows CI의 --locked 검증은 대기 중이며 실제 앱 오디오·장치·라이브 fader/미터는 미검증이다.

## 남은 작업과 제한

- CLI 전체 Scene snapshot 출력은 남아 있다. 실제 장치에서 정지 후 저장 및 재실행,
  GPUI에서 상용 VST3 원본 창 열기/닫기와 실시간 오디오를 함께 사용하는 경로는 미검증이다.
- 실제 MiniFuse/가상 케이블/상용 VST3 GUI·청취·왕복 지연·장시간 안정성은 미검증이다.
  실제 장치 교체 후 GUI 저장·시작도 실장비에서 확인해야 한다. 자동 검사/native fixture를
  실장비·음질·상용 호환성 검증으로 부르지 않는다.
- 다중 앱 동시 캡처·탭 선택·실제 앱 오디오 검증, 자체 가상 장치, ASIO, plugin 지연 보상, 실행 중 체인 삽입/삭제/순서 변경,
  내장 공간계/음정 보정은 미구현이다. 각 VST3는 단일 active stereo 입출력만 지원한다.
- 모든 restart 요청은 경로 중단 사유다. in-process crash/hang과 control/worker mutex 위험이
  남으며 native 시작/종료가 멈추면 GUI도 기다릴 수 있다. hard-real-time/자동 복구 보장 없음.
- MiniFuse 모델과 WASAPI/ASIO/물리 단자/loopback 번호 대응을 추정하지 않는다.
  개인 장치 ID/녹음/사용자 plugin 상태는 공개하지 않는다. 드라이버 설치·서명·공식 릴리스 없음.

현재 상태는 이 파일, 이력은 Git, 실행 근거는 코드 commit별 Actions를 따른다.
