# 현재 상태

기준일: 2026-11-11
단계: **마이크 + 선택적 AUX 믹싱을 main에 반영하고 Windows 통합 검사·개발 빌드 완료**

## 현재 구현

- WASAPI 마이크 → 내장 보컬 체인 → 출력 mix 1~2개. 선택적 mono/stereo AUX를
  별도 소스로 합산한다. AUX는 실제 INPUT endpoint이며 앱 이름/PID 직접 캡처가 아니다.
- `--aux-input`, `--aux-channels`, `--aux-gain-db`, `--aux-to output|send|both`.
  기본 send-only. 보컬 보정은 마이크에만 적용하고 AUX 좌우와 source trim을 따로 유지한다.
- 같은 입력 endpoint의 분리 채널은 한 capture stream/clock을 공유한다. 채널 겹침은 거절한다.
  별도 입력 endpoint는 기존 bounded queue/sinc resampler로 마이크 처리 클럭에 맞춘다.
- AUX float headroom은 source trim까지 보존하고 sample clipping guard는 최종 출력에서만 적용한다.
  AUX 부족은 해당 소스만 무음, 장치/비유한 sample/overflow 오류는 전체 경로 중단이다.
- 별도 AUX의 queue/리샘플러 지연과 상태를 표시한다. 녹음 시각 정렬·지연 보상·외부 피드백 검출은 없다.
  MR만 분리된 입력을 선택해야 하며 MixLive 출력이나 통화 수신음을 다시 AUX로 넣지 않는다.
- 기존 공유 출력·독립 출력 경로와 VST3 검사/상태/editor/WAV 도구는 유지한다.
  VST3는 아직 실시간 마이크에 연결하지 않았다. 새 dependency·crate·CI job 없음.
- Scene schema 2는 AUX 설정까지 저장한다. 이전 개발 Scene은 현재 옵션으로 다시 저장한다.
  구식 형식 이행·기본 장치 대체·파일을 열자마자 송출하는 동작은 없다.

## 실제 검증

코드: `496c4a2b8f56ecafb224ddbe2b405baea308b605`.
[Audio #10](https://github.com/andongmin94/mix-live/actions/runs/42943204397),
Windows job `130269675764`: 모든 단계 success. Windows Server 2025 / Rust 1.99.1.

| 검사 | 결과 |
|---|---|
| `cargo test --workspace --locked` | audio 16 + core 8 + effects 7 + plugins 7 + Scene CLI 2 = 40 passed |
| 기존 native VST3 fixture 통합 검사 | editor 2 + loading/state 2 + WAV 2 = 6 passed |
| Clippy all-targets, warnings denied | 통과 |
| Windows release 실행 파일 2개 및 CLI 도움말 | 빌드·실행 성공; run 도움말의 AUX 옵션 확인 |

새 검사에서 세 AUX 목적지, mic-only 보정, stereo/mono/역순 매핑, 613-frame 가변 블록,
float headroom, 44.1→48 kHz 변환, 부족/오류 무음을 확인했다. Scene/CLI 검사는 AUX
설정 저장·복원과 잘못된 schema/값의 실행 전 거절을 포함한다. 실제 오디오 장치는 열지 않았다.
수정 기준 소스와 원격 blob, 변경 8개 파일의 원격 blob을 대조했고 변경 공백 검사도 통과했다.

개발 artifact `13821518026` ZIP SHA256을 내려받아 대조했다:
`a3849fca6946d786e58512c49606ae433dcc69f9759f647d1dca611419d358b0` — 일치.
ZIP 무결성과 두 x64 PE 실행 파일을 확인했다. Cargo.lock·README·NOTICE는 줄바꿈을
정규화하면 소스와 일치한다. 로컬 Rust 실행은 하지 않았고 위 결과는 Windows Actions다.
이 검증 기록만 바꾸는 후속 커밋은 코드·의존성을 바꾸지 않아 통과한 CI를 다시 실행하지 않는다.

## 미완료

- 실제 MiniFuse/가상 케이블에서 mic+MR, 직접 모니터와의 시차, 장시간 dropout·왕복 지연·청취.
- Chrome 등 프로세스 직접 캡처, live VST3, GPUI, 자체 가상 장치, ASIO.
- 상용 VST3 editor 수동 수용, 전체 source/VST3 Scene·실행 중 전환.
- 자동 녹음·클라우드 전송·드라이버 설치·공식 릴리스 없음.

현재 상태는 이 파일, 이력은 Git, 실행 근거는 해당 commit의 Actions를 따른다.
