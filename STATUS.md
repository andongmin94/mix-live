# 현재 상태

기준일: 2026-11-11
단계: **VST3 원본 편집창 경로 구현·Windows 빌드/상태 검사 통과. 실제 GUI·라이브 VST3 수용은 남음**

## 이번 묶음

- `mixlive-plugins edit`: 사용자 VST3 원본 창, 기존 class/state/parameter 설정 재사용.
- 창 닫힘으로 완료하며 --state-out을 명시한 경우에만 새 파일로 저장한다.
  editor 자식 프로세스가 정상 종료한 뒤 부모가 게시한다. 기존 파일은 덮어쓰지 않는다.
- 창과 controller는 자식 메인 스레드 소유. Windows 메시지 dispatch 중 plugin lock을 잡지 않는다.
  GUI 변경을 0-sample parameter flush로 전달하고 component/controller state를 함께 저장한다.
- PluginWindow와 잠긴 winapi 0.3.9를 재사용한다. 신규 패키지 버전이나 별도 GUI framework 없음.
- unsafe는 Win32 메시지 경계만 허용. 오디오/effects/core의 forbid는 유지한다.
- 기존 실시간 mic/DSP/출력 코드는 변경하지 않았다. editor는 오디오 장치를 열지 않는다.

## 실제 검증

코드 커밋: `1f378c36aee23da21cae450ac10dbb003a039914`.
[Audio #7](https://github.com/andongmin94/mix-live/actions/runs/42844917616)의
Windows job `129929729403` 및 모든 단계가 success다. Windows Server 2025 / Rust 1.99.1.

| 검사 | 실제 결과 |
|---|---|
| `cargo test --workspace --locked` | audio 7 + core 8 + effects 7 + plugins 7 = 29 passed, 0 failed |
| 기존 fixture의 native 통합 검사 | editor 2 + 기존 로딩/상태 2 + WAV 2 = 6 passed, 0 failed |
| Clippy, all targets, warnings denied | 통과 |
| 두 Windows release 실행 파일 | 빌드 성공 |
| audio / run / inspect / render / edit 도움말 | 실제 실행 성공, 장치는 열지 않음 |

새 검사는 명시적 저장 인자, 기존 파일 보존, editor 미지원/잘못된 DLL의 저장 미발생,
0-sample parameter 처리 후 0.29 설정값의 저장과 새 프로세스 복원을 확인했다.
실제 창을 열어 노브를 조작하는 positive GUI 테스트는 수행하지 않았다.
기존 fixture를 재사용했고 유료 플러그인 설치·새 검증 프레임워크는 추가하지 않았다.

내려받은 artifact `13777412007`의 SHA256은 GitHub 값과 일치한다:
`782a0c120c3a6950f261e0962a08e5a0c2ce4b21f346485e1b5fbc5bcdf7f66d`.
두 exe와 안내 파일을 확인했고 Cargo.lock은 CRLF 차이를 제외하면 저장소와 동일하다.
로컬 cargo/rustc는 없어 로컬 Rust 검사는 미실행이다. 위 결과는 실제 Windows Actions다.
이 검증 기록만 갱신하는 커밋은 코드/의존성을 바꾸지 않아 CI를 다시 실행하지 않는다.

## 남은 수용 범위

- 실제 상용 VST3 편집창 표시·노브 조작·DPI·닫힘은 아직 수동 검증하지 않았다.
- VST3 실시간 마이크 연결, GPUI 메인 UI, 앱 캡처, 가상 장치, ASIO, Scene 저장은 미완료.
- MiniFuse 실제 채널·왕복 지연·장시간 dropout와 기본 보컬 프리셋 청취 수용은 미완료.
- editor 설정/비실시간 WAV 성공을 실시간 오디오 또는 음질 수용이라고 표시하지 않는다.
- 사용자 음성·플러그인 상태·유료 바이너리 수집, 드라이버 설치, 공식 릴리스 없음.

## 다음

VST3 오디오 처리의 thread/lifecycle/lock 경계를 해결해 실시간 체인에 연결하고,
기존 장치·보정·플러그인 제어를 GPUI 메인 화면으로 묶는다.
현재 상태는 이 파일, 이력은 Git, 실행 증거는 해당 commit의 Actions를 따른다.
