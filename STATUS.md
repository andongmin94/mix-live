# 현재 상태

기준일: 2026-11-11
단계: **실행 중 소스별 목적지·보내기 음량을 main에 반영하고 Windows 통합 검사·개발 빌드 완료**

## 현재 구현

- WASAPI 마이크 + 선택적 mono/stereo AUX → 마이크 전용 보컬 DSP → 출력 mix 1~2개.
  같은 입력/출력 endpoint의 분리 채널은 스트림을 공유하며 별도 장치는 clock bridge를 사용한다.
- `--interactive`에서 기존 source/output/전체 mute와 master/send gain에 더해
  `route mic|aux output|send on|off`, `level mic|aux output|send <dB -96..0>`을 지원한다.
  예: 내 헤드폰 보컬만 끄고 송신 보컬 유지, 상대에게 보내는 MR만 추가 감쇠.
- 경로 on/off·level·source mute·출력 mute는 독립이다. 꺼둔 경로의 level을 바꾸어도
  켜지지 않는다. 없는 AUX/두 번째 출력 조작이나 잘못된 값은 기존 설정을 바꾸지 않는다.
- 초기 마이크 목적지와 AUX의 --aux-to/Scene 설정을 유지한다. 보컬 DSP는 한 번만 처리한다.
  core의 고정 disabled/mix-minus/mute를 우회하지 않는 sample-wise send envelope를 추가했다.
- callback에서는 사전 준비된 envelope 버퍼와 기존 5 ms gain ramp·atomic target을 사용한다.
  경로 제어는 합산 전이어서 출력 queue 지연이 남는다. 전체 출력 mute는 기존처럼 queue 뒤다.
  이미 OS에 보낸 소리·외부 Direct Monitor를 제어하거나 외부 피드백을 자동 검출하지 않는다.
- 현재 실행의 변경만 적용한다. Scene schema 2·파일·장치 바인딩·보컬 설정은 변경하지 않는다.
  VST3 검사/상태/editor/WAV 도구는 유지했으며 live VST3는 아직 연결하지 않았다.
- 변경은 core/control/capture와 README/STATUS뿐이다. 의존성·lockfile·CI 작업 추가 없음.

## 실제 검증

코드: `8a591f742ca7a283522d143a6064d5bb2ea4936b`.
[Audio #12](https://github.com/andongmin94/mix-live/actions/runs/42958267676),
Windows job `130318451223`: 모든 단계 success. Windows Server 2025 / Rust 1.99.1.

| 검사 | 결과 |
|---|---|
| `cargo test --workspace --locked` | audio lib 22 + audio CLI 2 + Scene CLI 2 + core 10 + effects 7 + plugins 7 = 50 passed |
| 기존 native VST3 fixture 통합 검사 | editor 2 + loading/state 2 + WAV 2 = 6 passed |
| Clippy all-targets, warnings denied | 통과 |
| Windows release 실행 파일 2개 및 CLI 도움말 | 빌드·실행 성공 |

총 56 passed, 0 failed. 새 5개 검사는 경로별 선택/음량과 mute의 독립성, 보컬 단일 처리,
613-frame 가변 블록, 단일 출력의 ramp 연속성, core send 순서·disabled/mix-minus 유지,
잘못된 envelope의 이전 출력 차단을 확인했다. 실제 오디오 장치를 열지 않았다.
변경 기준 파일의 blob 대조 및 Git 공백 검사도 통과했다. 로컬 Rust 실행은 하지 않았으며
컴파일·실행·Clippy 결과는 위 Windows Actions에서 확인했다.

개발 artifact `13828511369`를 내려받아 SHA256을 대조했다:
`4a620d7a9189f83668c46e7098eecd61ab7f5857a5423b2489ee800aea9cf64e` — 일치.
ZIP 무결성·두 x64 PE 실행 파일을 확인했고 README는 줄바꿈 정규화 후 준비한 소스와 일치한다.
이 검증 기록만 바꾸는 후속 커밋은 코드·의존성을 변경하지 않아 통과한 CI를 다시 실행하지 않는다.

## 남은 범위

- 실제 MiniFuse·가상 케이블에서 목적지별 조작/청취/왕복 지연/장시간 안정성.
- live VST3, GPUI 메인 화면, 앱별 직접 캡처, 자체 가상 장치, ASIO.
- 전체 Scene 저장·실행 중 체인 교체, 상용 VST3 editor와 기본 보컬 프리셋의 실제 수용.
- 자동 녹음·클라우드 전송·드라이버 설치·공식 릴리스 없음.

현재 상태는 이 파일, 이력은 Git, 실행 증거는 해당 commit의 Actions를 따른다.
