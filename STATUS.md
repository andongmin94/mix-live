# 현재 상태

기준일: 2026-11-11
단계: **장치/보컬/VST3 체인을 한 Scene으로 저장·재실행하고 Windows 검증·개발 빌드 완료**

## 현재 구현

- WASAPI mic + 선택적 mono/stereo AUX → mic 전용 내장 보컬 DSP → 순차 stereo VST3 체인 →
  목적지별 라우팅/음량 → 출력 mix 1~2개. 공유 endpoint의 분리 채널과 별도 clock bridge를 유지한다.
- source/output/all mute, master/send gain, 경로별 on/off/level을 실행 중 제어한다.
  VST3는 독립 중복 인스턴스·슬롯별 라이브 editor·정상 종료 후 전체 체인 state 저장을 지원한다.
- 이번 변경: AudioScene schema 3. 기존 route와 ordered effects(path/class/state)를 한 JSON에 저장한다.
  `scene save --vst3-chain`은 참조가 아니라 상태 바이트를 포함한다. 원본 체인 JSON은 재실행에
  필요 없지만 실제 DLL/외부 샘플/라이선스는 별도로 필요하다.
- 반복 `--vst3`도 Scene 저장 가능하다. 이때 native loading 없이 선택만 저장하므로 state는 없으며
  실행 시 플러그인의 초기 설정을 사용한다. `effects: []`는 의도적인 무플러그인 Scene이다.
- save/show는 장치나 DLL을 열지 않는다. scene check는 장치를 확인하지만 native 호환성을 검증하지 않는다.
  scene run은 내장 체인만 사용하고 --vst3/--vst3-chain 덮어쓰기를 거절한다.
  실행별 --vst3-editor 슬롯/--vst3-chain-out 새 파일 선택은 가능하다.
- ResolvedPlugins로 CLI/파일 선택과 native 실행을 분리했다. 설정은 한 번 해석하며 기존
  처리·종료 경로를 재사용한다. 상대 경로는 원래 chain/scene 기준, CLI 경로는 저장 전 절대화한다.
- schema 1/2 Scene과 DeviceScene은 제거했다. 이전 개발 Scene은 현재 옵션으로 다시 저장한다.
  체인 파일은 schema 1을 유지하며 별도 VST3 inspect/editor/WAV 도구도 그대로다.
- Scene은 **시작 설정**이다. 실행 중 route/mute/gain과 마지막 VST3 편집 상태까지 전체 Scene으로
  저장하는 세션 snapshot은 아직 없다. 편집 결과는 새 chain-out을 내보내 새 Scene에 넣는다.
- dependency/lockfile/CI 작업 변경 없음. README와 AGENTS의 오래된 CI 안내를 현재 상태로 정리했다.

## 실제 검증

코드: `4ddfd6ebfc473cec17b12b3b5c840b49f8b43848`.
[Audio #17](https://github.com/andongmin94/mix-live/actions/runs/42976718650),
Windows job `130377401138`: 모든 단계 success.

| 검사 | 결과 |
|---|---|
| `cargo test --workspace --locked` | audio lib 29 + audio CLI 3 + Scene CLI 3 + core 10 + effects 7 + plugins 7 = 59 passed |
| 기존 native VST3 fixture 별도 검사 | chain/worker/state 3 + editor 2 + loading/state 2 + WAV 2 = 9 passed |
| Clippy all-targets, warnings denied | 통과 |
| Windows release 실행 파일 2개 및 CLI 도움말 | 빌드·실행 성공 |

총 68 passed, 0 failed. 원래 체인 JSON 삭제 후 Scene 복원, 다른 디렉터리/한글 경로,
중복 DLL의 독립 state, 잘못된 schema/class/slot/override의 실행 전 거절과 기존 파일 보존을 확인했다.
실제 native fixture의 서로 다른 두 상태를 Scene에 넣고 다시 native 로드해 값을 확인했다.
**오디오 장치나 상용 plugin editor 창을 열어 조작한 검사는 아니다.** 로컬 Rust 실행은 하지 않았고
컴파일·실행·Clippy 결과는 위 Windows Actions에서 확인했다.

개발 artifact `13835234375`를 내려받아 ZIP SHA256과 CRC 무결성을 확인했다:
`bc9cfd7c6534d7e5dcb1d67debdebfb60f4e8724788e50fe8df47fed4ccabffb` — 일치.
ZIP에는 README/NOTICE/Cargo.lock과 두 실행 파일 등 6개 파일이 있다.
검증 기록과 AGENTS만 바꾸는 후속 커밋은 코드·의존성 변경이 없어 통과한 CI를 반복하지 않는다.

## 다음 개발 및 미완료

- 다음 우선: GPUI + 사용자의 기존 gpui-design으로 Scene 열기/명시적 시작·정지/라이브 라우팅을
  잇는 작은 실제 화면. 최신 라이브러리 타입을 확인하고 기존 엔진을 재사용한다. 정적 UI 데모로 대체하지 않는다.
- 앱별 직접 캡처, 전체 실행상태 저장, editor 재열기, plugin 지연 보상과 실행 중 체인 변경.
- 실제 MiniFuse/가상 케이블/상용 VST3 GUI·청취·왕복 지연·장시간 안정성 수용은 미완료다.
  MiniFuse 모델과 WASAPI/ASIO/루프백 채널 대응을 추정하지 않는다.
- 자체 가상 장치·ASIO·내장 공간계/음정 보정은 미구현. 각 VST3는 단일 active stereo 입출력만 받는다.
  모든 native restart 요청은 경로 중단 사유이며 in-process crash/hang과 control/worker mutex 위험이 남아 있다.
- hard-real-time 보장·자동 녹음·클라우드 전송·드라이버 설치·공식 릴리스 없음.

현재 상태는 이 파일, 이력은 Git, 실행 근거는 commit별 Actions를 따른다.
