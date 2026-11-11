# 현재 상태

기준일: 2026-11-11
단계: **장치 Scene 저장·재실행을 main에 반영하고 Windows 통합 검사·개발 빌드 완료**

## 현재 구현

- WASAPI 마이크 → 내장 보컬 체인 → 서로 다른 출력 endpoint 1~2개.
- `mixlive-audio scene save/show/check/run`: 입력/출력 ID·채널, 각 음량, 보컬 프리셋·
  강도·threshold, queue 목표를 JSON으로 저장·재사용한다. 저장과 조회는 장치를 열지 않는다.
- check/run은 직접 입력 방식과 같은 Prepared 경로를 공유한다. 현재 장치·채널·포맷을
  다시 확인하며 파일 로딩만으로 송출하거나 누락 장치를 기본 마이크로 대체하지 않는다.
- 기존 파일 덮어쓰기, 지원하지 않는 schema, 알 수 없는 필드, 필수 값 누락,
  잘못된 채널·음량을 거절한다. Scene에 음성은 저장하지 않고 개인 endpoint ID는 Git에서 제외한다.
- 별도 VST3 도구의 검사·상태 저장/복원·원본 편집창·WAV 처리 경로는 유지했다.
  VST3는 아직 실시간 마이크 체인에 연결하지 않았다.
- Scene은 기존 serde/serde_json/tempfile을 재사용한다. 패키지 버전·checksum과 기존 CI는 변경하지 않았다.

## 실제 검증

코드 커밋: `5670151487aab641f0e726d045e8103d7448017e`.
[Audio #8](https://github.com/andongmin94/mix-live/actions/runs/42935865491)의
Windows job `130245676013` 전체 성공. Windows Server 2025 / Rust 1.99.1.

| 검사 | 실제 결과 |
|---|---|
| `cargo test --workspace --locked` | 단위 32개 + Scene CLI 통합 2개 = 34 passed, 0 failed |
| 기존 VST3 fixture의 별도 native 통합 검사 | editor 2 + loading/state 2 + WAV 2 = 6 passed, 0 failed |
| Clippy, all targets, warnings denied | 통과 |
| Windows release 실행 파일 2개 | 빌드 성공 |
| audio/run/inspect/render/edit 도움말 | 실행 성공; audio 도움말에 scene 명령 포함 |

이번에 추가한 Scene 단위 3개·CLI 2개는 네 프리셋과 두 출력 설정의 왕복,
한글·공백 경로, 기존 파일 보존, 잘못된 설정·버전의 장치 실행 전 거절을 확인했다.
가짜 endpoint ID로 파일 작업만 검사했으며 실제 오디오 장치를 열지 않았다.
미반영 패치는 원격 기준 파일과 대조했고 업로드한 11개 파일도 로컬 blob과 일치했다.

개발 artifact `13818850905`의 ZIP SHA256을 내려받아 대조했다:
`de414ac7c45d695275355a7bb05779502f19e2e96700cda38d725c193162bd31` — 일치.
두 Windows exe와 안내 파일을 확인했다. Cargo.lock·README·NOTICE는 CRLF를 제외하면 소스와 동일하다.
로컬에는 cargo/rustc가 없어 Rust 실행 검증은 Windows Actions에서 수행했다.
검증 기록만 바꾸는 후속 커밋은 코드·의존성을 변경하지 않아 통과한 CI를 다시 실행하지 않는다.

## 미완료

- 실제 장비에서 Scene 저장/재실행, MiniFuse 채널·왕복 지연·장시간 dropout·보컬 청취.
- 같은 endpoint의 1/2·3/4 등 두 채널 쌍을 동시에 사용하는 출력. 공유 스트림·클럭 구현이 필요하다.
- VST3 실시간 마이크 연결, 상용 원본 편집창 수동 수용, GPUI, 앱 캡처, 자체 가상 장치, ASIO.
- 현재 Scene은 마이크 하나와 별도 출력 1~2개용이다. 전체 source/mix/VST3 Scene과 실행 중 전환은 미구현.
- 자동 음성 녹음·클라우드 전송·드라이버 설치·공식 릴리스는 하지 않았다.

현재 상태는 이 파일, 변경 이력은 Git, 실행 근거는 해당 commit의 Actions를 따른다.
