# 현재 상태

기준일: 2026-11-25
단계: **가상 장치 개발 패키지 빌드 완료 · 내장 에코 및 VST3 경로 검색 추가**

## 현재 구현

- `mixlive.exe`: 새 Scene/기존 파일 열기 → 장치·앱·보컬·VST3 설정 → 새 파일 저장 →
  명시적 시작·정지. GPUI Deck 고정 revision `78c0898ccb6feba03e3a3243ac7d4729fd80aee6`,
  GPUI family `=0.3.12`. 웹뷰 없음.
- WASAPI mic + 선택적 물리 AUX + 최대 8개 앱 프로세스 트리 → 모니터·통화·방송의 최대 세 독립 출력 mix. 통화/방송은 각각 선택 사항.
  같은 endpoint의 최대 세 분리 믹스는 6채널까지 한 스트림/clock bridge를 공유한다. 각 앱의 별도 MTA 캡처 클라이언트와
  bounded/Rubato clock bridge는 기존 wasapi 0.27.0 및 sysinfo 0.31.4를 사용한다.
  모든 앱을 준비한 뒤 시작하며, 보컬은 한 번만 처리하고 AUX/앱은 보컬 DSP/VST3를 우회한다.
- GPUI 앱 목록에 추가/삭제, 앱별 채널·trim·mute·모니터/통화/방송 연결 및 경로별 gain을
  제공한다. 새 앱은 모든 경로 off. 물리 AUX도 함께 사용할 수 있다. 중복/겹치는 프로세스 트리,
  모호한 루트, MixLive를 포함한 트리는 거절한다. 앱 이름이나 PID를 추정해 자동 선택하지 않는다.
- 실행 중 mic/AUX/앱별 mute 및 각 목적지로의 send·gain, master/믹스별 감쇠·출력 mute를 조절한다.
  AUX/앱 공통 trim은 기존 Scene 필드에서 시작하고 GUI/CLI에서 실시간 조절한다.
  입력 clock에서 5 ms ramp를 한 번 적용하고 PDC 이후 mute/send gain을 유지한다.
  실제 입력 peak·보컬 체인 전후·gain reduction·출력별 peak를 약 100ms마다 읽는다.
  앱별 peak와 clock 진단도 독립적이다. callback/worker 버퍼는 준비 시 고정 용량으로 만든다.
- 정지 상태의 장치·출력 설정은 기존 장치/앱·채널·보컬·플러그인 설정과 모든 앱의 mixer 값을
  보존한다. 누락/지원 불가 장치와 앱을 명시하며 저장/시작을 거절하고 임의로 대체하지 않는다.
  보컬 설정은 장치 조회 없이 preset·trim·strength·threshold·VST3 순서와 상태를 편집한다.
  기존 CLI의 filesystem-only VST3 표준 경로 검색을 GPUI에서 재사용해 발견 후보를 명시적으로
  추가한다. native DLL 로딩·class/호환성 검증은 시작 시점이며 경로 검색 자체는 이를 보장하지 않는다.
- 최종 활성화된 VST3 체인의 신고 latency 합으로 통화·방송의 AUX/앱 send를 정수 샘플 지연한다.
  모니터는 추가 보상하지 않는다. 지연 뒤에 소스 mute/route gain을 적용하며, 실제 보상이 필요할 때
  2초 상한을 넘으면 시작을 거절한다. 준비 시 할당하며 지연 변경 restart는 기존대로 경로를 중단한다.
  장치 timestamp/clock bridge/OS 지연을 실측·보상하는 것은 아니다.
- 내장 EQ/다이내믹스 뒤에 선택적 8-line orthogonal FDN stereo 리버브를 적용한다.
  기존 biquad 고역 감쇠를 재사용한다. wet 비율(0은 off), 명목 잔향 시간, wet-only pre-delay를
  GPUI/CLI에서 설정하고 저장한다. dry는 추가 지연하지 않으며 stereo VST3 입력을 유지한다.
  실행 중 trim·strength·compressor/deesser threshold·reverb mix/decay를 독립 atomic 목표값과
  10 ms sample ramp로 조절한다. zero strength/mix에서도 준비된 DSP 상태를 갱신한다.
  pre-delay와 프리셋은 정지 상태에서만 변경한다. off 프리셋의 EQ/다이내믹스 파라미터는 비활성이다.
  리버브 뒤에 독립 stereo feedback echo를 추가했다. 최대 1초 delay buffer를 시작 전에 만들고
  mix(0..1), feedback(0..0.85)을 10 ms ramp로 실시간 조절한다. 시간(10..1000 ms)은 정지 중 변경한다.
  AUX/앱은 영향을 받지 않는다. 보컬은 한 번만 처리하고 mute는 리버브와 반복음을 포함해 적용한다.
- VST3 슬롯마다 명시적 host bypass: 신고 지연만큼 정렬한 dry를 5 ms ramp로 교차 전환한다.
  같은 효과 인스턴스를 계속 처리하며 다른 슬롯과 PDC를 유지한다. 총 신고 지연 상한은 2초다.
  GPUI 실행 중/정지 설정 및 CLI `bypass N on|off`에서 조절하고 Scene/체인에 저장한다.
  음량 일치·CPU 절감·오류 시 dry fallback은 아니다.
- 실행 중 VST3 원본 창은 슬롯별 열기/닫기/재열기. 같은 실제 인스턴스를 유지하며, GPUI가
  메시지 루프를 소유한다. 창 조작과 native poll은 GPUI 변경 참조 밖에서 수행한다.
- 정지 후 새 Scene 저장은 장치 중지 → 앱 capture thread join → VST3 worker join → 모든
  상태 수집 → 새 파일 게시를 사용한다. 실행 중 모든 소스/출력 mute·route·gain, 보컬 파라미터와 VST3 상태를
  다음 시작 전에 복원한다. 일반 정지/닫기/오류는 저장하지 않으며 파일 덮어쓰기 없음.
  schema 10은 에코 설정과 슬롯별 `bypassed`와 `route.apps` 배열(실행 파일 경로와 슬롯별 설정), 방송 장치·채널·감쇠와 각 믹스의 상태 및 리버브 설정을 저장하고 PID나 잔향 버퍼는 저장하지 않는다.
  CLI의 직접 `run`/`scene run --scene-out`도 같은 전체 snapshot을 정상 정지 후 새 파일에 저장한다.
  종료 시 console 명령을 닫아 늦은 입력이 snapshot을 바꾸지 못하게 한다. 기존 파일·chain-out 동시 요청은
  시작 전에 거절한다. VST3 chain schema는 2이며 bypassed는 필수다.
  Scene schema 1~9 및 chain schema 1, 기존 단일 `aux_app` 필드와 `--aux-app` 옵션은 제거했다. CLI는 반복 `--app` 및
  `appN` 제어 명령을 사용한다.

- `drivers/virtual-audio/`에 고정 Call/Broadcast/Return PnP 하드웨어 ID와 독립 render/capture
  WaveRT miniport를 추가했다. Cableloom의 MIT 고지를 보존한 C++ WDK 경계이며 별도 관리자·IOCTL은 없다.
  48 kHz stereo PCM32, 96-frame 고정 FIFO, QPC 기반 clock, RUN 경계 초기화와 부재 시 무음/폐기를
  제공한다. 드라이버는 Rust 오디오/DSP/VST3/Scene 형식과 독립이다. 설치된 render ID만 기존 WASAPI에서
  선택할 수 있으며 실제 endpoint 노출은 아직 미검증이다.
  Return은 외부 앱/DAW의 render→capture→기존 AUX를 위한 별도 장치이며 실제 앱에서 선택·수신한 결과는 미검증이다.
  Return과 같은 앱의 process loopback 중복 캡처, Return으로의 자기 송출은 사용하지 않는다.
  개발용 manage-devices.ps1은 SignTool kernel-policy 및 catalog의 INF/SYS 검증 후에만
  명시적 관리자 승인으로 WDK DevGen/PnPUtil 등록을 수행한다. 고정 ID만 조회·제거하고
  중복 장치·자동 재시작·보안 우회를 허용하지 않는다. 제품 설치기는 별도 미구현이다.

## 검증

- [Audio #37](https://github.com/andongmin94/mix-live/actions/runs/45567131587) 전체 성공:
  `beea453`의 Windows 검사 113개(리버브·실시간 보컬 파라미터·native VST3 포함),
  Clippy, locked release 빌드, CLI 및 실제 GPUI Scene 생성/편집/저장 검사 통과.
  이번 드라이버 변경은 Rust 코드를 수정하지 않으므로 Audio CI를 반복하지 않는다.
- [Virtual Audio #2](https://github.com/andongmin94/mix-live/actions/runs/45568859781):
  기존 두 케이블의 MSVC transport 검사, WDK 26100 x64 컴파일·링크,
  InfVerif /w·/h, Inf2Cat unsigned catalog 생성 성공. 미서명 catalog 거절 및
  두 PnP ID의 부재 확인까지 성공했지만, 예상된 SignTool 실패의 native 종료 코드가
  PowerShell CI wrapper에 전달되어 job 실패 및 artifact 미게시로 기록됐다.
  모든 assertion 통과 시에만 명시적으로 0을 반환하도록 수정했다.
- [Virtual Audio #4](https://github.com/andongmin94/mix-live/actions/runs/45569677959) 성공:
  `08d0007`의 세 고정 INF/PnP ID, transport 격리, WDK/INF/unsigned catalog 및
  미서명 거절·부재 검사 workflow 완료.
  드라이버 서명·실제 설치/로드/제거, WASAPI 장치 노출 및 실제 Call/Broadcast/Return 오디오는
  검증하지 않았다.
- [Audio #38](https://github.com/andongmin94/mix-live/actions/runs/45571558178) 성공:
  `9817ec6`의 CLI 전체 Scene snapshot/종료 입력 차단을 포함한 Windows 통합 검사 통과.
- [Audio #40](https://github.com/andongmin94/mix-live/actions/runs/45574156346): `10961f2`의
  라이브 AUX/앱 trim 및 VST3 슬롯 우회 관련 workspace 테스트, native fixture, Clippy 성공.
  최신 변경으로 대체되어 릴리스 빌드/GPUI 검사 완료 전에 취소됐다.
- 에코와 VST3 설치 경로 검색: 반복 간격/감쇠, 가변 블록, stereo 분리, ramp/reset, capture 경계의 mute,
  Scene/GPUI 저장, native 코드 미실행 경로 검색, GUI 검색/명시적 추가 계약을 검사하도록 추가했다.
  신규 변경의 통합 Audio CI 결과는 아직 확인하지 않았다.

## 남은 작업과 제한

- 실제 MiniFuse/가상 케이블/복수 앱 process loopback/상용 VST3 및 장치+UI 전체 실행은
  미검증이다. queue·plugin 신고값은 실측 왕복 지연이 아니며 sample peak는 LUFS/true peak가 아니다.
  다중 앱 캡처·음질·장시간 안정성을 검증 완료라고 부르지 않는다.
- 탭 선택·ASIO·실행 중
  소스/체인 삽입/삭제·내장 음정 보정은 미구현이다.
  CLI 전체 Scene snapshot의 실제 장치 실행은 미검증이다. 실제 로딩·장치 등록/제거·전원 상태·HVCI·HLK·서명·Discord/OBS 및 DAW Return 수신은 미검증이다.
  실행 중 프리셋/pre-delay/에코 시간 변경은 미구현이다. 리버브/에코/라이브 파라미터의 청취·CPU·실제 상용 VST3 경로는 미검증이다.
- 모든 VST3 restart 요청은 경로 중단 사유다. 각 VST3는 단일 active stereo 입출력만 지원한다.
  in-process crash/hang과 control/worker mutex 위험이 남으며 hard-real-time 보장은 없다.
- 원래 앱 재생 경로·장치 기본값은 변경하지 않는다. 자동 mix-minus 추정/외부 피드백 검출/
  전체 데스크톱 폴백/자동 재연결/녹음 없음. MiniFuse 모델이나 채널 번호를 추정하지 않는다.
  개인 장치 ID/앱 경로/녹음/플러그인 상태는 공개하지 않는다. 드라이버 설치·서명·공식 릴리스 없음.

현재 상태는 이 파일, 이력은 Git, 실행 근거는 코드 commit별 Actions를 따른다.
