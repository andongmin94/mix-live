# 현재 상태

기준일: 2026-11-25
단계: **Call/Broadcast 가상 오디오 드라이버 개발 소스 구현 · WDK 검증 대기**

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
  실제 입력 peak·보컬 체인 전후·gain reduction·출력별 peak를 약 100ms마다 읽는다.
  앱별 peak와 clock 진단도 독립적이다. callback/worker 버퍼는 준비 시 고정 용량으로 만든다.
- 정지 상태의 장치·출력 설정은 기존 장치/앱·채널·보컬·플러그인 설정과 모든 앱의 mixer 값을
  보존한다. 누락/지원 불가 장치와 앱을 명시하며 저장/시작을 거절하고 임의로 대체하지 않는다.
  보컬 설정은 장치 조회 없이 preset·trim·strength·threshold·VST3 순서와 상태를 편집한다.
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
  AUX/앱은 영향을 받지 않는다. 보컬은 한 번만 처리하고 mute는 잔향을 포함해 적용한다.
- 실행 중 VST3 원본 창은 슬롯별 열기/닫기/재열기. 같은 실제 인스턴스를 유지하며, GPUI가
  메시지 루프를 소유한다. 창 조작과 native poll은 GPUI 변경 참조 밖에서 수행한다.
- 정지 후 새 Scene 저장은 장치 중지 → 앱 capture thread join → VST3 worker join → 모든
  상태 수집 → 새 파일 게시를 사용한다. 실행 중 모든 소스/출력 mute·route·gain, 보컬 파라미터와 VST3 상태를
  다음 시작 전에 복원한다. 일반 정지/닫기/오류는 저장하지 않으며 파일 덮어쓰기 없음.
  schema 8은 `route.apps` 배열(실행 파일 경로와 슬롯별 설정), 방송 장치·채널·감쇠와 각 믹스의 상태 및 리버브 설정을 저장하고 PID나 잔향 버퍼는 저장하지 않는다.
  schema 1~7, 기존 단일 `aux_app` 필드와 `--aux-app` 옵션은 제거했다. CLI는 반복 `--app` 및
  `appN` 제어 명령을 사용한다.

- `drivers/virtual-audio/`에 고정 Call/Broadcast PnP 하드웨어 ID와 독립 render/capture
  WaveRT miniport를 추가했다. Cableloom의 MIT 고지를 보존한 C++ WDK 경계이며 별도 관리자·IOCTL은 없다.
  48 kHz stereo PCM32, 96-frame 고정 FIFO, QPC 기반 clock, RUN 경계 초기화와 부재 시 무음/폐기를
  제공한다. Rust 오디오/DSP/VST3/Scene schema 8은 변경하지 않았다. 설치된 render ID만 기존 WASAPI에서
  선택할 수 있으며 실제 endpoint 노출은 아직 미검증이다.

## 검증

[Audio #32](https://github.com/andongmin94/mix-live/actions/runs/45555026229) 전체 성공:
이전 단일 앱 캡처 코드 `0eca6085218ac40e9adbaaf2332409faedc9da61`의
Windows 테스트 94개(원본 VST3 창 재열기 fixture 포함), Clippy, locked release 빌드,
앱 목록 CLI와 실제 GPUI Scene 생성·편집·저장 검사를 통과했다.
이전 #30 컴파일 오류 및 #31 Clippy 모듈 배치 오류도 해당 실행에서 해결됐다.

[Audio #33](https://github.com/andongmin94/mix-live/actions/runs/45559585204)은 합성 mono
장치의 stereo 테스트 입력 오류로 중단됐고, `a90fae8`에서 mono 채널 1로 수정했다.
[Audio #34](https://github.com/andongmin94/mix-live/actions/runs/45561046164)는
세 믹스 변경의 Windows 검사 100개, native VST3 fixture, Clippy, locked release 빌드,
CLI 및 실제 GPUI Scene 생성·편집·저장 검사를 모두 통과했다.
[Audio #35](https://github.com/andongmin94/mix-live/actions/runs/45562830490)는
PDC 및 stereo 송출 지연·mute 검사를 포함한 Windows 검사 103개, native fixture, Clippy, locked release
빌드, CLI와 실제 GPUI Scene 생성·편집·저장 검사를 모두 통과했다.
[Audio #36](https://github.com/andongmin94/mix-live/actions/runs/45566374446)의
리버브 포함 workspace와 native VST3 fixture 검사는 성공했다. Clippy도 통과했다.  나머지 Windows 통합 검사는 새 변경 반영 직전 미완료였다.
[Audio #37](https://github.com/andongmin94/mix-live/actions/runs/45567131587)의 실시간 보컬 조절 변경은 workspace 검사, native fixture, Clippy를 통과했다. 나머지 통합 검사는 진행 중이다.
가상 장치 개발은 staged Git blob과 SHA가 동일한 실제 core.hpp/tests.cpp를 로컬 g++ C++17
-Wall -Wextra -Werror로 컴파일·실행했다. PCM32 stereo 데이터, 두 케이블 격리, 링 wrap,
RUN reset, 양쪽 부재 처리, 큰 clock gap과 QPC fractional carry/24시간 누적 계산 검사 성공.
WDK 26100 기반 unsigned 커널 빌드 및 InfVerif /w, /h는 새 Virtual Audio CI에서 검증 대기.
기존 Audio CI와 별도 변경 범위이며 설치·서명·카탈로그 생성은 하지 않는다.

## 남은 작업과 제한

- 실제 MiniFuse/가상 케이블/복수 앱 process loopback/상용 VST3 및 장치+UI 전체 실행은
  미검증이다. queue·plugin 신고값은 실측 왕복 지연이 아니며 sample peak는 LUFS/true peak가 아니다.
  다중 앱 캡처·음질·장시간 안정성을 검증 완료라고 부르지 않는다.
- 탭 선택·자체 Return 장치·ASIO·실행 중
  소스/체인 삽입/삭제·내장 음정 보정·CLI 전체 Scene snapshot 출력은 미구현이다.
  Call/Broadcast driver의 WDK 빌드·실제 로딩·장치 등록/제거·전원 상태·HVCI·HLK·서명·Discord/OBS 수신은 미검증이다.
  실행 중 프리셋/pre-delay 변경은 미구현이다. 리버브/라이브 파라미터의 청취·CPU·실제 상용 VST3 경로는 미검증이다.
- 모든 VST3 restart 요청은 경로 중단 사유다. 각 VST3는 단일 active stereo 입출력만 지원한다.
  in-process crash/hang과 control/worker mutex 위험이 남으며 hard-real-time 보장은 없다.
- 원래 앱 재생 경로·장치 기본값은 변경하지 않는다. 자동 mix-minus 추정/외부 피드백 검출/
  전체 데스크톱 폴백/자동 재연결/녹음 없음. MiniFuse 모델이나 채널 번호를 추정하지 않는다.
  개인 장치 ID/앱 경로/녹음/플러그인 상태는 공개하지 않는다. 드라이버 설치·서명·공식 릴리스 없음.

현재 상태는 이 파일, 이력은 Git, 실행 근거는 코드 commit별 Actions를 따른다.
