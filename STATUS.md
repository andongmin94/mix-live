# 현재 상태

기준일: 2026-11-11
단계: **VST3 원본 편집창·닫은 뒤 상태 저장 구현. Windows 통합 검사 대기**

## 이번 묶음

- `mixlive-plugins edit`: 사용자 VST3 원본 창, 기존 class/state/parameter 설정 재사용.
- 창 닫힘으로 완료하며 --state-out을 명시한 경우에만 새 파일로 저장한다.
  editor 자식 프로세스가 정상 종료한 뒤 부모가 게시한다. 기존 파일은 덮어쓰지 않는다.
- 창과 controller는 자식 메인 스레드 소유. Windows 메시지 dispatch 중 plugin lock을 잡지 않는다.
  GUI 변경을 0-sample parameter flush로 전달하고 component/controller state를 함께 저장한다.
- PluginWindow와 잠긴 winapi 0.3.9를 재사용한다. 신규 패키지 버전이나 별도 GUI framework 없음.
- unsafe는 Win32 메시지 경계만 허용. 오디오/effects/core의 forbid는 유지한다.
- 기존 실시간 mic/DSP/출력 코드는 변경하지 않았다. editor는 오디오 장치를 열지 않는다.

## 검증

- 기준 코드 `10ef699464fd25005382bbdfc2a666d338f637ea`의 Audio #6 성공은 유지된다.
- 새 일반 검사 2개와 기존 fixture를 재사용하는 native 검사 2개를 작성했다.
- 새 변경의 Windows compile/test/Clippy/실행 파일 검사는 아직 결과 확인 전이다.
- 로컬에는 cargo/rustc가 없고 GitHub 직접 DNS 조회가 실패해 원격 Actions에서 검사한다.

## 남은 수용 범위

- 실제 상용 VST3 편집창 표시·노브 조작·DPI·닫힘은 아직 수동 검증하지 않았다.
- VST3 실시간 마이크 연결, GPUI 메인 UI, 앱 캡처, 가상 장치, ASIO, Scene 저장은 미완료.
- MiniFuse 실제 채널·왕복 지연·장시간 dropout와 기본 보컬 프리셋 청취 수용은 미완료.
- editor 설정/비실시간 WAV 성공을 실시간 오디오 또는 음질 수용이라고 표시하지 않는다.
- 사용자 음성·플러그인 상태·유료 바이너리 수집, 드라이버 설치, 공식 릴리스 없음.

## 다음

이번 변경의 검사 결과를 반영한다. 이후 VST3 오디오 처리의 thread/lifecycle/lock 경계를
해결해 실시간 체인에 연결하고, 기존 장치·보정·플러그인 제어를 GPUI 메인 화면으로 묶는다.
현재 상태는 이 파일, 이력은 Git, 실행 증거는 해당 commit의 Actions를 따른다.
