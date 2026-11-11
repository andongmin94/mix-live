# 현재 상태

기준일: 2026-11-11
단계: **라이브 음소거·출력 음량·정상 중지를 main에 반영하고 Windows 통합 검사·개발 빌드 완료**

## 현재 구현

- WASAPI 마이크 + 선택적 mono/stereo AUX → 마이크 전용 보컬 DSP → 출력 mix 1~2개.
  같은 입력/출력 endpoint의 분리 채널은 스트림을 공유하고 별도 장치에는 clock bridge를 둔다.
- 직접 run/scene run의 `--interactive`: mic/AUX/첫 출력/send/전체 mute, master/send gain,
  status/stop. 선택하지 않으면 기존 유한 시간 실행이다. EOF/입력 오류도 종료 요청이다.
- callback은 atomic target만 읽는다. 약 5 ms sample ramp로 source/output gain을 변경한다.
  전체 mute 해제는 개별 mute를 보존하며 fader 변경도 mute를 해제하지 않는다.
- 최종 master/send gain은 출력 리샘플러 뒤, clipping guard/peak 앞에 적용한다.
  output mute는 이미 MixLive queue에 쌓인 소리에도 적용한다. mute 중에도 queue를 소비한다.
  source mute는 보컬 DSP 뒤/AUX 합산 전에 적용하며 보컬 검출기 상태를 유지한다.
- source meter는 mute 전, stream meter는 gain/mute 후다. mute는 하드웨어 캡처 중단이 아니며
  이미 OS에 보낸 샘플이나 외부 Direct Monitor에는 작용하지 않는다. source mute에는 출력 queue 지연이 남는다.
- 약 5 ms는 목표값을 관측한 뒤의 ramp 길이이지 명령·OS·드라이버를 포함한 응답시간 보장이 아니다.
- runtime 제어는 현재 실행만 바꾸며 Scene schema 2·파일·장치 바인딩을 변경하지 않는다.
  stop/종료 시 소유 스레드에서 capture/playback을 해제한 뒤 Stopped를 출력한다.
- VST3 검사·상태/editor/WAV 도구는 유지한다. live VST3는 미연결이다.
  새 dependency·crate·CI job 없음. lockfile과 Scene 저장 코드는 변경하지 않았다.

## 실제 검증

코드: `6e351809bdf18503fbb272cc1343611055edb4ba`.
[Audio #11](https://github.com/andongmin94/mix-live/actions/runs/42954838687),
Windows job `130307362991`: 모든 단계 success. Windows Server 2025 / Rust 1.99.1.

| 검사 | 결과 |
|---|---|
| `cargo test --workspace --locked` | audio lib 19 + audio CLI 2 + Scene CLI 2 + core 8 + effects 7 + plugins 7 = 45 passed |
| 기존 native VST3 fixture 통합 검사 | editor 2 + loading/state 2 + WAV 2 = 6 passed |
| Clippy all-targets, warnings denied | 통과 |
| Windows release 실행 파일 2개 및 CLI 도움말 | 빌드·실행 성공; run 도움말의 --interactive 확인 |

이번 검사는 명령·값 검증, mute 독립성, ramp의 연속성·정확한 무음·반복 목표 처리,
이미 채워진 출력 queue에 대한 제어, source별 mute, gain 이동 후 기존 출력 크기를 확인했다.
직접/Scene run 인자와 stdin/EOF/stop도 장치 없이 검사했다. 전체 51 passed, 0 failed.
실제 장치를 열거나 MiniFuse에서 조작·청취한 결과는 아니다.

개발 artifact `13826508336`을 내려받아 ZIP SHA256을 대조했다:
`7864227379c0250eaad71797eb501e16d89b88db1a1ecbe6786218b5bf4d7786` — 일치.
ZIP 무결성 및 두 x64 PE 실행 파일을 확인했다. Cargo.lock·README·NOTICE는 줄바꿈을
정규화하면 준비한 소스와 일치한다. Rust 실행 검증은 Windows Actions에서 수행했다.
이 검증 기록만 바꾸는 후속 커밋은 코드·의존성을 바꾸지 않아 통과한 CI를 다시 실행하지 않는다.

## 미완료

- 실제 MiniFuse·가상 케이블에서 live control/청취/왕복 지연/장시간 안정성.
- live VST3, GPUI, 앱 직접 캡처, 자체 가상 장치, ASIO, 전체 Scene/실행 중 장치·보컬 체인 교체.
- 상용 VST3 원본 editor 수동 수용과 보컬 프리셋의 실제 음질 수용.
- 자동 녹음·클라우드 전송·드라이버 설치·공식 릴리스 없음.

현재 상태는 이 파일, 이력은 Git, 실행 근거는 해당 commit의 Actions를 따른다.
