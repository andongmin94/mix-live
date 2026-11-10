# 현재 상태

기준일: 2026-11-10
단계: **S0 설계·기초 코어 원격 반영 및 실행 검증 완료**

## 구현 범위

- DESIGN.md: 제품 요구, source/mix/output, 장치·clock 경계, VST3, GPUI,
  사운드 품질 및 단계별 수용 기준.
- `mixlive-core`: mono/stereo 라우팅, source/send/mix gain과 mute, mix-minus 제외,
  가변 블록과 출력 버퍼 재사용, 설정·입력 검증, sample-peak 조회.
- 단위 테스트 8개, 합성 입력 예제 `discord_singing`, portable core용 CI 작업 1개.
- 외부 코드·의존성, UI·장치 어댑터, 실행 가능한 방송 앱은 아직 없다.

## 실제 검증

코드 커밋: `652939e2e27e8f25fe12e5fd45d7104274a38a95`.
[Core #1](https://github.com/andongmin94/mix-live/actions/runs/42785353106)의
`core` 작업(`129725064549`)과 전체 단계가 성공했다. 로그의 실제 실행 결과:

| 검사 | 결과 |
|---|---|
| `cargo test --workspace --locked` | 8 passed, 0 failed, 0 ignored |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 성공 |
| `cargo run -p mixlive-core --example discord_singing --locked` | 성공: 통화 송신 peak 0.300, 헤드폰 peak 0.600 |
| 로컬 준비 파일과 GitHub 트리 SHA 비교 | 동일: `5b70b7813763981d864557a2bb3e7300bf1a2711` |

실행 환경은 GitHub-hosted Ubuntu 26.04.1, Rust stable 1.99.1이다.
로컬에는 Rust 도구체인이 없고 다운로드 DNS 조회도 실패하여 로컬 Rust 검사는 실행하지 않았다.
이 상태 기록은 코드·의존성·workflow를 바꾸지 않는 문서 변경이다. 문서만 바꾸는 커밋은
Core CI를 다시 실행하지 않으며 위 코드 커밋의 결과를 사용한다.

## 미완료 범위

- Windows/MiniFuse/ASIO/Chrome/Discord/OBS/가상 장치/GPUI/VST3: 미구현·미실행.
- 청취 품질, CPU 부하, dropout, clock drift, end-to-end 지연: 미측정.
- 예제의 Discord/MR은 합성 버퍼 이름이다. 실제 앱 연결이나 실제 음성 청취를 의미하지 않는다.
- 현재 core에는 limiter가 없다. 실제 방송이나 물리 출력용으로 사용하지 않는다.

## 저장소

- `andongmin94/mix-live`: 사용자가 생성한 공개 저장소. 기본 브랜치 `main`.
- README, 설계·개발 규칙·출처, Rust workspace/core/테스트/예제, CI 등 16개 파일 반영.
- 라이선스는 아직 선택하지 않았다. 공개 가시성과 제품 라이선스 부여를 구분한다.
- 현재 커밋과 변경 이력은 Git, 실행 결과는 해당 커밋의 Actions를 따른다.

## 다음 작업

S1: 선택 가능한 Windows 장치·입출력 채널을 조회하고, 선택한 마이크를 헤드폰과
검증된 송출 경로에 보내는 한 단위를 구현한다. MiniFuse 모델·드라이버·채널은
실장비 확인 전 고정하지 않는다. 장치 경로가 동작한 뒤 보컬 DSP와 VST3를 추가한다.
빈 모듈·복구 체계를 미리 만들지 않는다.
