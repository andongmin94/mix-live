# 현재 상태

기준일: 2026-11-11
단계: **장치 Scene 저장·재실행 구현. Windows 통합 검사 대기**

## 이번 묶음

- 기준: `461065ec889e6b784dd72d8409e0e1463da6e107`. 미반영 Scene 패치의 기준 파일
  9개를 원격 blob과 대조한 뒤 적용했다. 기존 VST3와 실시간 DSP 코드는 변경하지 않았다.
- `mixlive-audio scene save/show/check/run`: 현재 마이크 입력/채널, 헤드폰·송출
  endpoint, 각 음량, 내장 보컬 프리셋/threshold/강도, queue 목표를 JSON으로 재사용한다.
- save/show는 장치를 조회하거나 시작하지 않는다. check/run은 직접 입력 방식과 같은
  Prepared 경로를 공유해 실행 시 정확한 endpoint ID와 실제 채널·포맷을 다시 확인한다.
- 기존 파일은 덮어쓰지 않는다. 알 수 없는 필드·버전, 필수 값 누락, 잘못된 채널·음량은
  거절한다. 장치가 없다고 기본 마이크로 대체하지 않으며 파일 로딩만으로 송출하지 않는다.
- 기존 serde/serde_json/tempfile 버전·checksum을 유지하고 audio crate에서도 재사용한다.
  새 framework, 실행 파일, CI job, 호환 포맷이나 자동 복구는 추가하지 않았다.

## 검증

- 패치 기준 9개 blob 일치, Git 적용 검사, TOML 파싱, 변경 공백 검사를 통과했다.
- Scene 일반 테스트 3개와 CLI 통합 테스트 2개를 추가했다. Windows compile/test/Clippy와
  실행 파일 검증은 현재 변경 묶음의 기존 Audio CI 결과로 확인한다. 아직 결과 대기 중이다.
- 로컬 cargo/rustc는 없고 GitHub 직접 연결은 DNS 오류다. 로컬 Rust 성공을 주장하지 않는다.

## 미완료

- 실제 장비에서 Scene 저장/재실행, MiniFuse 채널·왕복 지연·장시간 dropout·보컬 청취.
- 현재 Scene은 마이크 하나와 별도 출력 1~2개의 설정이다. 같은 endpoint의 두 채널 쌍을
  독립 출력으로 여는 기능은 아직 없다. 채널 번호만 풀지 않고 공유 장치 클럭을 다뤄야 한다.
- VST3 실시간 마이크 연결과 상용 원본 편집창 수동 수용, GPUI, 앱 캡처, 가상 장치, ASIO.
- 전체 source/mix/VST3 Scene 및 실행 중 전환은 미구현이다. 자동 녹음·클라우드 전송,
  드라이버 설치·공식 릴리스는 하지 않았다.

현재 상태는 이 파일, 변경 이력은 Git, 실행 근거는 해당 commit의 Actions를 따른다.
