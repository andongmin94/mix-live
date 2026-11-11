# 현재 상태

기준일: 2026-11-11
단계: **순차 VST3 보컬 체인·슬롯별 편집창·일괄 상태 저장을 main에 반영하고 Windows 검증·개발 빌드 완료**

## 현재 구현

- WASAPI mic + 선택적 AUX → mic 전용 내장 보컬 DSP → 순서대로 연결한 stereo VST3 체인 →
  목적지별 라우팅/음량 → 출력 mix 1~2개. AUX는 보컬 체인 전체를 우회한다.
- `--vst3 <path>`를 반복해 처리 순서를 지정한다. 같은 DLL의 중복 슬롯도 독립 인스턴스/상태다.
  체인 전체에 worker와 bounded input queue는 하나이며, 단계 사이의 stereo를 유지한다.
- `--vst3-editor <1-based slot>`를 반복해 원하는 슬롯의 실제 라이브 원본 창을 연다.
  창 닫기는 해당 editor만 제거하고 체인은 유지한다. 창 다시 열기는 아직 없다.
- 정상 종료의 `--vst3-chain-out <새 파일>`은 모든 슬롯의 순서·경로·class·state를 한 파일로 저장한다.
  `--vst3-chain <파일>`로 복원한다. 상대 plugin 경로는 체인 파일 기준이며 저장 snapshot은 절대 경로다.
- 장치 중지 → worker join → 모든 editor 제거 → 마지막 편집 flush/processing stop/state 취득 →
  native 해제 → 파일 게시 순서다. 어느 슬롯이든 실패하면 부분 파일을 게시하지 않는다.
  오류 중에도 나머지 슬롯의 종료를 수행하며 기존 파일·암묵적 Drop·실패 세션은 저장하지 않는다.
- 단일 plugin용 `--vst3-state`, `--vst3-class`, `--vst3-state-out` launch 옵션은 제거했다.
  `--vst3-editor`는 이제 슬롯 번호가 필요하다. 별도 단일 VST3 검사/editor/WAV 도구는 유지한다.
- route Scene schema 2는 그대로이며 체인과 별도 파일이다. 실시간 경로/음량 변경을 자동 저장하지 않는다.
- 설정/파일, native 처리, native 검사를 분리했다. dependency·lockfile·CI 작업 변경 없음.

## 실제 검증

코드: `43c37131818c6766734f6a46fbb0d0577e88e923`.
[Audio #16](https://github.com/andongmin94/mix-live/actions/runs/42973390481),
Windows job `130366801662`: 모든 단계 success. Windows Server 2025 / Rust 1.99.1.

| 검사 | 결과 |
|---|---|
| `cargo test --workspace --locked` | audio lib 27 + audio CLI 3 + Scene CLI 2 + core 10 + effects 7 + plugins 7 = 56 passed |
| 기존 native VST3 fixture 별도 검사 | chain/worker/state 3 + editor 2 + loading/state 2 + WAV 2 = 9 passed |
| Clippy all-targets, warnings denied | 통과 |
| Windows release 실행 파일 2개 및 CLI 도움말 | 빌드·실행 성공; 새 chain/editor-slot 옵션 확인 |

총 65 passed, 0 failed. 실제 gain DLL 두 인스턴스의 직렬 처리와 두 번째 인스턴스만 변경하는
동작, AUX 우회, distinct stereo·44.1 kHz·부분 블록을 확인했다. 마지막 편집의 두 state 저장·복원,
잘못된 class/slot/layout, 상대 경로·한글 파일명·중복 DLL, 기존 파일 보존도 검사했다.
두 번째 state 취득이 실패하는 경우 첫 state만 남는 부분 파일을 게시하지 않음을 확인했다.
**실제 오디오 장치나 editor 창을 열어 조작한 검사는 아니다.** Rust 실행 검증은 위 Windows Actions다.

개발 artifact `13834207786`을 내려받아 ZIP SHA256을 대조했다:
`121ec2554c2d6a7149969da855fa2c86f25945318ed8283315fbf1cefca7b2bb` — 일치.
ZIP 무결성·두 x64 PE 실행 파일을 확인했다. README·NOTICE·Cargo.lock은 줄바꿈 정규화 후
준비한 소스와 일치한다. 검증 기록만 바꾸는 후속 커밋은 코드·의존성을 바꾸지 않아 CI를 반복하지 않는다.

## 남은 범위

- 실제 MiniFuse·가상 케이블·상용 VST3 GUI 조작/청취·왕복 지연·장시간 안정성.
- GPUI 메인 화면, 앱별 직접 캡처, 자체 가상 장치, ASIO, plugin 지연 보상, 전체 Scene.
- 실행 중 체인 삽입/삭제/순서 변경, editor 재열기, 내장 공간계/음정 보정과 프리셋 음질 수용.
- 각 슬롯은 단일 active stereo 입출력만 받는다. 모든 restart 요청은 경로 중단 사유다.
  신고 지연 합계는 실측 지연이 아니며 editor/control mutex와 in-process crash/hang 위험은 남아 있다.
- hard-real-time 보장·자동 녹음·클라우드 전송·드라이버 설치·공식 릴리스 없음.

현재 상태는 이 파일, 이력은 Git, 실행 증거는 commit별 Actions를 따른다.
