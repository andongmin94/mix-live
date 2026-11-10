# 현재 상태

기준일: 2026-11-10
단계: **VST3 검사·상태 복원·WAV 처리 Windows 통합 검사 통과. 실시간 VST3는 미연결**

## 현재 구현

- 기존 WASAPI mic→내장 보컬→독립 출력 1~2개 경로는 이번 묶음에서 변경하지 않았다.
- `mixlive-plugins`: bundle 목록, 별도 프로세스 검사, class/parameter/bus/신고 지연,
  component/controller 상태 저장·복원. 잘못된 값과 중복 parameter ID는 로딩 전에 거절한다.
- `render`: mono/stereo PCM 8/16/24/32-bit·float32 WAV를 한 stereo VST3 효과에 적용한다.
  파일 sample rate 유지, mono 복제, stereo 분리, float headroom, 부분 블록·명시적 tail 지원.
- WAV는 블록 단위로 처리한다. 기존 파일을 덮어쓰지 않고 worker의 정상 종료 후에만 게시한다.
  잘린 WAV·NaN/inf·지원하지 않는 bus·native 실패는 출력 없이 종료한다. dry 우회·재시도 없음.
- Audio #5가 생성한 lock을 반영하고 hound 3.5.1만 추가했다. 이후 모든 제품 검사는 --locked다.

## 실제 검증

코드 커밋: `10ef699464fd25005382bbdfc2a666d338f637ea`.
[Audio #6](https://github.com/andongmin94/mix-live/actions/runs/42839533180)의
Windows job `129910719799` 전체 성공. Windows Server 2025 / Rust 1.99.1.

| 검사 | 실제 결과 |
|---|---|
| `cargo test --workspace --locked` | audio 7 + core 8 + effects 7 + plugins 5 = 27 passed, 0 failed |
| 기존 native fixture를 사용하는 별도 통합 검사 | 기존 검사·상태 복원 2 + WAV 처리 2 = 4 passed, 0 failed |
| Clippy, all-targets, warnings denied | 통과 |
| 두 Windows release 실행 파일 빌드 | 통과 |
| audio / run / inspect / render 도움말 실행 | 통과; 장치를 열지 않음 |

native 검사는 실제 DLL 로딩, 별도 프로세스 상태 복원, Gain 0.25 PCM 결과,
613-frame stereo/부분 블록, 44.1 kHz mono 24-bit+44-frame tail, 0 dBFS 초과 float 값 보존,
기존 파일 보존 및 잘린 입력·비유한 값·악기·잘못된 DLL의 출력 미게시를 확인했다.
fixture 두 개는 기존 것을 재사용했고 설치하거나 제품에 포함하지 않았다.

내려받은 artifact `13776510150` SHA256:
`2911ab40d96c6948f62bdee67432f7b9a70e708ad9b6ea91ff5e6f70266d35ad` — GitHub 값과 일치.
Cargo.lock 내용은 저장소와 동일하며 Windows 체크아웃의 CRLF만 다르다. 두 exe를 확인했다.
로컬에는 cargo/rustc가 없어 로컬 Rust 검사를 하지 않았다. 위 결과는 실제 Windows Actions다.
이 상태 기록만 갱신하는 커밋은 코드·의존성을 바꾸지 않으며 통과한 CI를 다시 실행하지 않는다.

## 미완료

- MiniFuse 실제 장비·채널·장시간 dropout·왕복 지연, 실제 목소리 청취 수용.
- VST3 live callback·원본 editor, GPUI, 앱 캡처, 자체 가상 장치, ASIO, Scene 저장.
- offline WAV 검증은 live 보정 수용이 아니다. plugin 신고 latency 보상·자동 잔향 탐지 없음.
- 기본 보컬 프리셋은 청취 미수용 시작값이다. 자동 녹음·클라우드 업로드·driver 설치·릴리스 없음.

## 다음

기존 로딩/상태 경계를 재사용하면서 VST3의 lifecycle·실시간 처리 요구를 만족하는 경로를
구현하고 GPUI 제어에 연결한다. 파일 렌더는 음질 비교에 사용할 수 있지만 음량 일치와
실제 청취 평가는 별도이며, 제품을 파일 편집 DAW로 확장하지 않는다.
현재 상태는 이 파일, 변경 이력은 Git, 실행 증거는 해당 commit의 Actions를 따른다.
