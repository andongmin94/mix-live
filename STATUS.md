# 현재 상태

기준일: 2026-11-11
단계: **마이크 + 선택적 AUX 믹싱 구현. 이번 변경의 Windows 통합 검사 대기**

## 이번 묶음

- 기준 main `4d706636876bccff32087afdfb2eb3d09c0543c4`. 기존 WASAPI/DSP/출력 경로에
  mono/stereo AUX 캡처 소스를 추가했다. 앱 이름/PID가 아니라 실제 INPUT endpoint를 선택한다.
- `--aux-input`, `--aux-channels`, `--aux-gain-db`, `--aux-to output|send|both`.
  기본은 send-only. 보컬 체인은 마이크에만 적용하고 AUX 좌우를 유지한 채 목적지별 합산한다.
- 같은 입력 장치의 분리 채널은 한 스트림/클럭에서 추출한다. 겹치는 마이크/AUX 채널은 거절한다.
  다른 입력 장치는 기존 bounded queue/sinc resampler를 재사용해 마이크 클럭에 맞춘다.
- AUX float headroom은 source trim까지 보존하고 출력 보호는 최종 출력에서만 수행한다.
  AUX 부족은 해당 소스만 무음, 장치/비유한 sample/overflow 오류는 전체 경로 중단이다.
- 별도 AUX의 queue/리샘플러 지연을 표시한다. 녹음 시각 정렬·지연 보상·외부 피드백 검출은 하지 않는다.
- Scene schema 2에 AUX 설정을 저장한다. 구식 schema 이행/폴백은 없으며 이전 개발 파일은 재작성한다.
- 새 dependency·crate·CI job 없음. VST3 코드는 변경하지 않았다.

## 검증

- 수정 전 5개 소스 파일을 원격 blob과 대조했다. 변경 공백 검사 통과.
- 관련 검사: AUX 목적지·입력 채널 검증, 보컬과 AUX의 분리·stereo/float headroom·부분 블록,
  44.1→48 kHz AUX 변환/부족/오류, Scene/CLI의 AUX 설정 저장·복원.
- 로컬 Rust 도구체인이 없어 컴파일/테스트/Clippy는 기존 Windows Actions에서 확인한다.
  새 결과는 아직 확인 전이다. 기존 검사 성공을 새 기능의 검증으로 취급하지 않는다.

## 미완료

- 실제 MiniFuse/가상 케이블에서 mic+MR, 직접 모니터와의 시차, 장시간 dropout·왕복 지연·청취.
- Chrome 등 프로세스 직접 캡처, live VST3, GPUI, 자체 가상 장치, ASIO.
- 상용 VST3 editor 수동 수용, 전체 source/VST3 Scene·실행 중 전환.
- 자동 녹음·클라우드 전송·드라이버 설치·공식 릴리스 없음.

현재 상태는 이 파일, 이력은 Git, 실행 근거는 해당 commit의 Actions를 따른다.
