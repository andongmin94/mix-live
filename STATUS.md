# 현재 상태

기준일: 2026-11-11
단계: **보컬 VST3 하나의 라이브 처리 경로 구현, 이번 변경의 Windows 통합 검사 대기**

## 이번 묶음

- 기준 main `4b989018d95b3a2fe4b8dac906277cd3158ab774`.
- 직접 run/scene run에 --vst3, --vst3-state, --vst3-class 실행 옵션을 추가했다.
  내장 보컬→stereo VST3 하나→기존 목적지별 라우팅. AUX는 VST3를 우회한다.
- 플러그인 state와 class를 확인하고 stereo 단일 입출력만 수용한다. 실패 시 원음으로 우회하지 않는다.
- 장치 callback은 bounded frame queue에 mic/AUX를 함께 넣는다. 전용 worker가 DSP/VST3/mix를
  처리하며, 같은 소스의 두 목적지 때문에 플러그인을 중복 실행하지 않는다.
- 로딩/상태/요청 처리는 main thread, native process는 worker, 해제는 join 뒤 loading thread다.
  plugin mutex는 worker/control 사이에 남아 있다. 완전 lock-free/hard-real-time 보장은 아니다.
- 플러그인 지연 신고값과 입력 queue 용량을 표시한다. 실제 추가 지연/클럭/상용 호환성은 미검증.
- 기존 Scene schema 2는 그대로. 플러그인은 매 실행 명시하며 현재 Scene에 저장하지 않는다.
  VST3 미선택 시 기존 직접 처리, 기존 native editor/WAV 도구는 유지한다.
- 이미 잠긴 vst3-host와 상태 모듈을 재사용했다. 외부 버전/checksum은 변경하지 않는다.
  CI는 기존 native fixture 단계에 audio crate의 ignored 통합 검사만 포함한다.

## 검증

- 기존 capture 테스트를 capture_tests.rs로 분리해 유지했다. 새 검사: stereo 외부 효과와 AUX
  분리, 실패 무음, launch 인자, bounded input overflow, 실제 gain DLL의 worker 처리/상태/종료.
- 로컬 cargo/rustc가 없어서 Rust 실행 검증은 기존 Windows Actions로 수행한다.
  현재 새 컴파일/테스트/Clippy/빌드 결과는 아직 확인 전이다.

## 미완료

- 실제 MiniFuse/상용 VST3에서 청취·왕복 지연·dropout·장시간 안정성.
- live editor/parameter 조절, 다중 plugin 체인, plugin 지연 보상 및 전체 Scene 저장.
- GPUI 메인 화면, 앱 직접 캡처, 자체 가상 장치, ASIO.
- in-process native crash/hang 격리 없음. 자동 녹음/클라우드 전송/드라이버 설치/공식 릴리스 없음.

현재 상태는 이 파일, 이력은 Git, 실행 증거는 해당 commit의 Actions를 따른다.
