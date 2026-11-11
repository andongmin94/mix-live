# 현재 상태

기준일: 2026-11-11
단계: **동일 출력 endpoint의 분리 채널 라우팅을 main에 반영하고 Windows 통합 검사·개발 빌드 완료**

## 현재 구현

- WASAPI 마이크 → 내장 보컬 → 독립 mix 1~2개. 같은 endpoint의 겹치지 않는 채널은
  한 출력 스트림으로 묶고, 다른 endpoint는 각각의 스트림을 사용한다.
- 보컬 DSP는 한 번 처리한다. 두 mix의 음량은 각각 적용한 뒤 같은 endpoint에 보낼
  stereo mix들을 한 프레임에 넣어 하나의 queue·다채널 resampler·clock 보정을 공유한다.
- mono·역순 stereo·미지정 채널 무음과 공유 경로의 underrun/fault 무음을 지원한다.
  겹치는 채널은 거절하고 현재 장치 기본 채널 수를 넘으면 스트림 시작 전에 오류로 종료한다.
- check는 출력 스트림 수와 mix 매핑을 표시한다. peak·clipping·queue 통계는 stream 단위다.
- 기존 Scene save/show/check/run과 VST3 검사·상태·원본 editor·WAV 도구는 유지한다.
  VST3는 실시간 마이크에 연결하지 않았다. dependency·lockfile·CI job은 변경하지 않았다.

## 실제 검증

코드: `c6dae36096d89bf5c1bc32d43da4bd2921b24a43`.
[Audio #9](https://github.com/andongmin94/mix-live/actions/runs/42939195387),
Windows job `130256673832`: 모든 단계 success. Windows Server 2025 / Rust 1.99.1.

| 검사 | 결과 |
|---|---|
| `cargo test --workspace --locked` | audio 13 + core 8 + effects 7 + plugins 7 + Scene CLI 2 = 37 passed |
| 기존 native VST3 fixture 통합 검사 | editor 2 + loading/state 2 + WAV 2 = 6 passed |
| Clippy all-targets, warnings denied | 통과 |
| Windows release 실행 파일 2개 및 CLI 도움말 | 빌드·실행 성공 |

새 검사에서 44.1→48 kHz 변환 중 두 mix의 샘플 정렬, 독립 음량, mono/역순 매핑,
미지정 채널 무음, 공유 underrun/fault 무음을 확인했다. Scene 왕복에 동일 endpoint의
분리 채널을 포함하고 겹치는 설정은 거절한다. 실제 오디오 장치는 열지 않았다.

개발 artifact `13820391089` ZIP SHA256을 내려받아 대조했다:
`5379a9d85c695205960f8d117255a65ba89cd00771ec30218068b307afe2bff6` — 일치.
두 x64 PE 실행 파일과 ZIP 무결성을 확인했다. Cargo.lock·README·NOTICE는 줄바꿈을
정규화하면 소스와 일치한다. 로컬 Rust 실행은 하지 않았고 위 결과는 Windows Actions다.
이 검증 기록만 바꾸는 후속 커밋은 코드·의존성을 변경하지 않아 CI를 다시 실행하지 않는다.

## 미완료

- 실제 4채널 이상 WASAPI endpoint의 출력, MiniFuse 채널·왕복 지연·장시간 dropout·청취.
  Windows에 stereo로만 보이는 endpoint에 ASIO 3/4를 새로 노출하는 기능은 아니다.
- VST3 실시간 마이크 연결·상용 원본 editor 수동 수용, GPUI, 앱별 캡처, 가상 장치, ASIO.
- 전체 source/VST3 Scene과 실행 중 전환. 현재 입력은 마이크 하나, 출력 mix는 최대 두 개다.
- 자동 음성 녹음·클라우드 전송·드라이버 설치·공식 릴리스는 하지 않았다.

현재 상태는 이 파일, 이력은 Git, 실행 근거는 해당 commit의 Actions를 따른다.
