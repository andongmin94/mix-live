# 현재 상태

기준일: 2026-11-11
단계: **같은 출력 endpoint의 두 채널 경로 구현, Windows 통합 검사 대기**

## 이번 묶음

- 기준 main `c777c0d92710db2afc80f4b686953d2f09948367`. 기존 마이크·보컬·Scene 경로를 확장했다.
- 동일 WASAPI endpoint의 1/2·3/4 등 겹치지 않는 채널 경로를 한 출력 스트림으로 묶는다.
  서로 다른 endpoint는 각각의 스트림을 유지한다. 실제 resolved device identity로 그룹화한다.
- 보컬 DSP는 한 번 처리하고 두 mix의 음량은 기존 core에서 각각 적용한다. 같은 endpoint로
  가는 두 stereo mix는 프레임 단위로 함께 queue에 넣고 한 4채널 resampler로 처리한다.
  독립 queue/ratio로 같은 장치의 두 mix가 어긋나게 만들지 않는다.
- mono·stereo 역순 매핑, 미지정 채널 무음, 공유 경로의 underrun/fault 무음을 지원한다.
  겹치는 물리 채널은 거절하며 실제 장치 기본 채널 수를 넘는 설정은 스트림 시작 전 거절한다.
- 기존 Scene 형식을 그대로 사용한다. 새 dependency·crate·CI job·호환 계층은 없다.
- check는 실제 출력 스트림 수와 mix 매핑을 표시하고 통계는 endpoint/stream 단위다.

## 검증

- 새 관련 테스트: 두 mix의 리샘플링 샘플 정렬, 독립 음량+채널 출력, mono/역순/겹침 매핑.
- 기존 Scene 왕복 검사에 동일 endpoint의 분리 채널을 넣고, 잘못된 설정 검사에 겹침을 넣었다.
- 기존 독립 출력·clock drift·오류 무음 검사를 현재 frame API에 맞춰 갱신했다.
- Rust 실행 결과는 현재 묶음의 Windows Actions로 확인한다. 아직 새 CI 결과는 확인 전이다.
  로컬 cargo/rustc는 없으며 직접 GitHub 연결은 DNS 오류다. 미실행 검사를 통과라고 쓰지 않는다.

## 남은 수용

- 실제 4채널 이상 WASAPI endpoint에서 출력 확인, MiniFuse 채널·왕복 지연·장시간 dropout·청취.
  Windows에 stereo로만 노출되는 장치의 ASIO 3/4를 새로 만드는 기능은 아니다.
- VST3 실시간 마이크 연결·상용 원본 editor 수동 수용, GPUI, 앱별 캡처, 가상 장치, ASIO.
- 현재 Scene은 마이크 하나와 출력 mix 1~2개용이다. 전체 source/VST3 Scene·실행 중 전환은 남아 있다.
- 자동 음성 녹음·클라우드 전송·드라이버 설치·공식 릴리스는 하지 않았다.

현재 상태는 이 파일, 이력은 Git, 실행 근거는 해당 commit의 Actions를 따른다.
