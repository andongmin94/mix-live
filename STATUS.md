# 현재 상태

기준일: 2026-11-11
단계: **실행 중 소스별 목적지·보내기 음량 구현, 이번 변경의 Windows 검사 대기**

## 이번 묶음

- 기준 main `99bafbfc7a3e88d79f35d3c55427e86379132e84`.
- `route mic|aux output|send on|off`와 `level mic|aux output|send <dB -96..0>`.
  목소리·MR을 목적지별로 조절하며 source/출력/전체 mute와 경로 fader는 독립적이다.
- 초기 mic/AUX 목적지는 기존 설정을 유지한다. AUX 초기값은 capture 준비 때 적용하고
  장치 시작 후에는 atomic target과 기존 5 ms gain ramp만 사용한다.
- core에 원래 send 순서의 sample envelope를 전달한다. 고정 disabled/mix-minus/mute는
  우회하지 않는다. 보컬 DSP는 한 번만 실행하며 버퍼·경로는 시작 전에 준비한다.
- 경로 제어는 합산 앞이므로 출력 queue 지연이 남는다. 전체 출력 mute는 기존처럼 queue 뒤다.
- Scene schema 2, 장치 바인딩, DSP, VST3, 의존성/lockfile/CI는 변경하지 않는다.
  실행 중 변경은 현재 세션만 바꾸며 파일에 자동 저장하지 않는다.

## 검증

- 변경 대상 소스의 기준 blob을 원격과 대조했다. Git 공백 검사 통과.
- 추가 검사: core envelope 인덱스·믹스마이너스·오류 출력 차단, 경로 명령·mute 독립성,
  보컬 단일 처리와 목적지 분리, 단일 출력의 가변 블록/ramp. 기존 검사를 재사용한다.
- 로컬 Rust 도구체인이 없으므로 컴파일·실행 결과는 이번 묶음의 Windows Actions로 확인한다.
  아직 새 결과 대기 중이며 코드 작성/정적 검사를 실행 성공으로 취급하지 않는다.

## 남은 범위

실제 MiniFuse·가상 케이블의 청취/지연/장시간 안정성, live VST3, GPUI 메인 화면,
앱별 직접 캡처, 자체 가상 장치, ASIO, 전체 Scene 저장·실행 중 체인 교체는 미완료다.
자동 녹음·클라우드 전송·드라이버 설치·공식 릴리스는 하지 않았다.

현재 상태는 이 파일, 이력은 Git, 실행 증거는 해당 commit의 Actions를 따른다.
