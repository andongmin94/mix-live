# 현재 상태

기준일: 2026-11-11
단계: **실시간 보컬 VST3·원본 편집창·종료 후 상태 저장을 main에 반영하고 Windows 검증·개발 빌드 완료**

## 현재 구현

- WASAPI mic + 선택적 mono/stereo AUX → mic 전용 내장 보컬 DSP → 선택적 stereo VST3 하나 →
  목적지별 라우팅/음량 → 출력 mix 1~2개. 공유 endpoint의 분리 채널과 별도 장치 clock bridge를 유지한다.
- 중단됐던 live VST3 경로가 기준 main `fbd5a9779381f62c370a8b43d2531504f5b31f9c`에 이미
  반영돼 있고 Audio #14가 성공했음을 확인했다. 이번에는 그 경로에 editor/state 종료를 연결했다.
- `run/scene run --vst3 <path> --vst3-editor`: 실제 보컬 처리와 동일한 인스턴스의 원본 창.
  일반 노브 변경은 기존 호스트의 editor→processor 전달을 사용한다. 창 닫기는 editor만
  분리하고 송출은 유지한다. 창 다시 열기는 미구현. 요청한 editor가 없으면 시작 전에 거절한다.
- `--vst3-state-out <새 파일>`을 지정한 정상 종료만 저장한다. 입력/출력 중지 → worker join →
  editor 제거 → 마지막 parameter flush → processing stop → 상태 취득 → native 해제 → 파일 저장.
  기존 파일은 보존하고 오류·restart·암묵적 Drop에서는 저장하지 않는다. `--vst3-state`로 재사용한다.
- Win32 메시지 처리는 plugin mutex를 잡지 않은 main/로딩 스레드에서 수행한다.
  다른 editor/control API는 worker와 mutex를 공유하므로 GUI 조작 중 무중단·hard-real-time은 미보장이다.
  장치 callback에는 plugin 로딩/창/파일 작업을 넣지 않는다. crash/hang은 in-process라 격리되지 않는다.
- 모든 plugin restart 요청은 중단 사유다. bus/latency/oversampling뿐 아니라 일부 preset/parameter
  갱신도 중단될 수 있다. 자동 재구성·dry fallback·플러그인 지연 보상·다중 체인은 없다.
- 기존 interactive source/output mute·경로별 on/off/level, Scene schema 2와 별도 VST3 파일 도구 유지.
  VST3 launch 옵션과 실행 중 변경은 Scene에 자동 저장하지 않는다. 새 dependency·lockfile·CI 변경 없음.

## 실제 검증

코드: `a376941e1c0430acb00bd2356df854706da74220`.
[Audio #15](https://github.com/andongmin94/mix-live/actions/runs/42969632199),
Windows job `130354822728`: 모든 단계 success. Windows Server 2025 / Rust 1.99.1.

| 검사 | 결과 |
|---|---|
| `cargo test --workspace --locked` | audio lib 26 + audio CLI 3 + Scene CLI 2 + core 10 + effects 7 + plugins 7 = 55 passed |
| 기존 native VST3 fixture 별도 검사 | live worker/state 2 + editor 2 + loading/state 2 + WAV 2 = 8 passed |
| Clippy all-targets, warnings denied | 통과 |
| Windows release 실행 파일 2개 및 도움말 | 빌드·실행 성공; --vst3-editor/--vst3-state-out 확인 |

총 63 passed, 0 failed. 실제 native DLL의 실행 중 parameter 변경이 보컬 출력에 반영되고
AUX는 그대로 유지됨을 확인했다. 마지막 변경의 flush·저장·재로드, 오류/Drop 시 저장 금지,
기존 파일 보존과 editor 없는 fixture의 명시적 거절을 검사했다.
**창을 실제 열어 마우스로 노브/DPI/닫힘을 조작한 검사는 아니다. 오디오 장치도 열지 않았다.**
로컬에는 Rust 도구체인이 없어 컴파일·실행·Clippy는 위 Windows Actions에서 수행했다.

개발 artifact `13832318301`을 내려받아 ZIP SHA256을 대조했다:
`c43adec3db735bfe3244cb7b0136934e9548e8962f0773d52c0509b4d43a1ebf` — 일치.
ZIP 무결성·두 x64 PE 실행 파일을 확인했다. README·NOTICE·Cargo.lock은 줄바꿈 정규화 후
준비한 소스와 일치한다. 마지막 검증 기록 커밋은 문서만 변경하므로 통과한 CI는 반복하지 않는다.

## 남은 범위

- 실제 MiniFuse·가상 케이블·상용 VST3 GUI의 조작/청취·왕복 지연·장시간 안정성.
- GPUI 메인 화면, 앱별 직접 캡처, 자체 가상 장치, ASIO, 다중 plugin·지연 보상·전체 Scene.
- 내장 공간계/음정 보정과 기본 프리셋의 실제 음질 수용.
- 자동 녹음·클라우드 전송·드라이버 설치·공식 릴리스 없음.

현재 상태는 이 파일, 이력은 Git, 실행 근거는 commit별 Actions를 따른다.
