# 현재 상태

기준일: 2026-11-10
단계: **S1 Windows 장치 경로 작성, 해당 커밋의 CI 검증 진행**

## 이번 구현

- 기존 S0 코어는 변경하지 않았다. 첫 검증은 Core #1 (`42785353106`), 8 tests 성공.
- `mixlive-audio-windows`와 `mixlive-audio` CLI를 추가했다.
- WASAPI endpoint ID·기본 포맷 조회, 입력 1채널 선택, 출력 채널 매핑,
  마이크→독립 출력 1~2개, 입력/두 번째 믹스 음량.
- 출력별 bounded SPSC queue + Rubato sinc rate conversion 및 queue 기반 clock drift 조절.
- 출력 sample clipping guard, peak/queue/underrun/clipping 계수. 장치 오류에 자동 전환하지 않는다.
- 포맷/채널 사전 검사(check)는 스트림을 시작하지 않는다. run은 명시적 ID로만 동작한다.
- 새 테스트 6개: 채널 매핑, CLI 검증, rate/stereo 보존, ±500ppm 합성 drift,
  starvation/overflow/clip guard, 동일 capture의 두 믹스 분리·오류 시 무음.

## 검증

- 로컬 TOML 파싱 및 Git diff 공백 검사는 통과했다.
- 이 환경에는 Rust가 없고 외부 GitHub DNS도 해석되지 않는다. Rust 검사는 Windows Actions로 실행한다.
- CI는 Windows 단일 job으로 현재 workspace test, Clippy, release binary, 무장치 CLI help를 검사한다.
- 새 의존성의 Cargo.lock은 첫 실행에서 생성한 뒤 검토·커밋하고 locked 모드로 고정한다.
- **아직 새 코드 컴파일/테스트 성공을 확인하지 않았다.** 결과 확인 뒤 이 기록을 갱신한다.

## 미완료

- S1 실장비 수용: MiniFuse 모델·WASAPI 채널·driver/rate/buffer, 실제 통화·헤드폰 출력,
  실측 왕복 지연, 장시간 dropout/drift 및 청취는 미검증. CI의 합성 drift 검사는 실장비 증명이 아니다.
- ASIO, Chrome/app capture, 자체 가상 장치, GPUI, VST3, 보컬 DSP, Scene 저장은 미구현.
- CLI 경로의 gain과 sample clipping guard는 Signature Sound 또는 mastering limiter가 아니다.
- 새 기능은 개발용이며 공식 바이너리 릴리스·드라이버 설치·서명은 수행하지 않는다.

## 다음

1. 현재 변경의 Windows CI 실패만 수정하고 lockfile과 실행 결과를 확정한다.
2. 실제 장치 경로를 검증하고, 그 경로 위에 보컬 DSP 및 VST3를 추가한다.
현재 상태는 이 파일, 이력은 Git, 실행 근거는 각 커밋의 Actions를 따른다.
