# 현재 상태

기준일: 2026-11-11
단계: **라이브 음소거·출력 음량·정상 중지 구현. 이번 변경의 Windows 통합 검사 대기**

## 이번 묶음

- 기준 main `41efe0df558a9d424e3c3e143c6f00caacbfaf04`.
- 직접 run/scene run의 `--interactive`: mic/AUX/첫 출력/send/전체 mute, master/send gain,
  status/stop. 선택하지 않으면 기존 유한 시간 실행이다. EOF/입력 오류도 종료 요청이다.
- 음량/mute는 현재 실행만 바꾸며 Scene schema 2·파일·장치 바인딩은 바꾸지 않는다.
- callback은 atomic target만 읽는다. 약 5 ms sample ramp로 source/output gain을 변경한다.
  전체 mute 해제는 개별 mute를 보존하며 fader 변경도 mute를 해제하지 않는다.
- 기존 최종 master/send gain을 core의 고정 gain에서 출력 리샘플러 뒤로 이동했다.
  최종 clipping/peak 전 적용하므로 큐에 쌓인 음성도 output mute를 우회하지 않는다.
  mute 중에도 큐를 소비한다. source mute는 DSP 뒤 적용해 검출기 상태를 유지한다.
- source meter는 mute 전, stream meter는 gain/mute 후다. mute는 하드웨어 캡처 중단이 아니며
  이미 OS에 보낸 샘플이나 외부 Direct Monitor에는 작용하지 않는다.
- 새 dependency·crate·CI job 없음. VST3/Scene 저장 코드는 유지했다.

## 검증

- 새 검사: 명령/값 검증과 mute 독립성, sample ramp, 이미 채워진 큐에 대한 출력 제어,
  source별 분리 mute, 대화형 CLI 인자와 stdin/EOF/stop. 기존 관련 검사를 변경된 gain 위치에 맞췄다.
- 로컬 Rust 도구체인이 없어 실행 검증은 기존 Windows Actions에서 수행한다. 결과 대기.

## 미완료

- 실제 MiniFuse·가상 케이블에서 live control/청취/왕복 지연/장시간 안정성.
- live VST3, GPUI, 앱 직접 캡처, 가상 장치, ASIO, 전체 Scene/실행 중 장치·보컬 체인 교체.
- 자동 녹음·클라우드 전송·드라이버 설치·공식 릴리스 없음.

현재 상태는 이 파일, 이력은 Git, 실행 근거는 해당 commit의 Actions를 따른다.
