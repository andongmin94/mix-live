# 현재 상태

기준일: 2026-11-10
단계: **S1 Windows 장치 경로 구현·Windows 빌드 성공, 실장비 수용 대기**

## 구현

- S0 코어 유지. 새 `mixlive-audio-windows`와 `mixlive-audio` CLI 추가.
- WASAPI endpoint ID·기본 포맷 조회, 입력 1채널 선택, 출력별 채널 매핑.
- 마이크→독립 출력 1~2개, 입력/두 번째 믹스 음량. `check`는 스트림을 시작하지 않는다.
- 출력별 bounded SPSC queue와 Rubato sinc resampler, queue 기반 clock drift 조절.
- peak/queue/underrun/clipping 계수. 장치 오류에는 무음·종료, 다른 장치를 자동 선택하지 않는다.
- 직접 의존성 CPAL 0.18.2, Rubato 5.0.1, rtrb 0.4.0, clap 4.6.7을 Cargo.lock에 고정했다.

## 실제 검증

코드 커밋: `e5afb0d9034b338567246c1f732ff4a49124f979`.
[Audio #2](https://github.com/andongmin94/mix-live/actions/runs/42788397613)
Windows job `129735319470` 전체 성공 (Windows Server 2025, Rust 1.99.1).

| 검사 | 결과 |
|---|---|
| workspace tests | 기존 core 8 + 새 audio 6 = 14 passed, 0 failed, 0 ignored |
| Clippy all targets, warnings denied | 성공 |
| release Windows exe build | 성공 |
| exe `--help` | Windows에서 실제 실행 성공; 장치를 열지 않음 |
| artifact 내려받기 | 2파일(exe, Cargo.lock), 게시된 SHA256과 일치 |

새 검사는 채널 매핑·잘못된 CLI 설정·44.1→48 kHz stereo 보존·각 30초 합성 ±500ppm
clock drift·underrun/overflow/clip guard·한 capture의 두 믹스 분리와 fault 무음을 확인했다.
첫 CI의 실제 lockfile을 저장소에 반영하고 `--locked`를 복원했다. 의존성을 다시 선택하지
않은 빌드 결과는 이 변경의 Actions에서 확인한다. 로컬 Rust 도구체인은 없어 미실행이다.

## 미완료

- 실제 MiniFuse 모델·WASAPI 채널·드라이버·버퍼, 통화/헤드폰 출력, 장시간 dropout,
  실제 clock drift, 청취, 왕복 지연은 미검증. 합성 테스트를 실장비 검증으로 취급하지 않는다.
- ASIO, Chrome/app capture, 자체 가상 장치, GPUI, VST3, 보컬 DSP, Scene 저장은 미구현.
- 현재 출력은 기존 물리/가상 케이블의 재생 endpoint를 사용한다. 새 가상 마이크를 설치하지 않는다.
- gain과 sample clipping guard는 보컬 보정·transparent/true-peak limiter가 아니다.
- 개발용 exe는 서명된 출시 제품이 아니다. 녹음·드라이버 설치·공식 릴리스는 하지 않았다.

## 다음

S1 실장비 수용은 별도 유지한다. 구현 경로를 바탕으로 보컬 DSP와 VST3를 추가하되,
고품질 보정 완료는 실제 목소리의 음량 일치 청취 평가와 지연 검증으로 판정한다.
현재 상태는 이 파일, 이력은 Git, 실행 근거는 각 커밋의 Actions를 따른다.
