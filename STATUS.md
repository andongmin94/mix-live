# 현재 상태

기준일: 2026-11-10
단계: **S0 설계·기초 코어 작성, GitHub 실행 검증 대기**

## 구현 범위

- DESIGN.md: 제품 요구, source/mix/output, 장치·clock 경계, VST3, GPUI,
  사운드 품질 및 단계별 수용 기준.
- `mixlive-core`: mono/stereo 라우팅, source/send/mix gain과 mute, mix-minus 제외,
  가변 블록과 출력 버퍼 재사용, 설정·입력 검증, sample-peak 조회.
- 단위 테스트 8개, 합성 입력 예제 `discord_singing`, portable core용 CI 작업 1개.
- 외부 코드·의존성, UI·장치 어댑터, 실행 가능한 방송 앱은 아직 없다.

## 검증

- 소스·설계와 공개 저장소 포함 범위를 검토했다. 비공개 생성 안내를 제거했다.
- 로컬에는 Rust 도구체인이 없고 `static.rust-lang.org` DNS 조회가 실패하여
  로컬 compile/test/clippy/example은 실행하지 못했다.
- GitHub Actions의 Core workflow에서 테스트·Clippy·합성 예제를 실행하도록 구성했다.
  아직 성공을 확인하지 않았으며 실제 결과 확인 후 이 파일을 갱신한다.
- Windows/MiniFuse/ASIO/Chrome/Discord/OBS/가상 장치/GPUI/VST3: 미구현·미실행.
- 청취 품질, CPU 부하, dropout, clock drift, end-to-end 지연: 미측정.
- 현재 core에는 limiter가 없다. 실제 방송이나 물리 출력용으로 사용하지 않는다.

## 저장소

- `andongmin94/mix-live`: 사용자가 생성한 공개 저장소. 기본 브랜치 `main`.
- 라이선스는 아직 선택하지 않았다. 공개 가시성과 제품 라이선스 부여를 구분한다.
- 현재 커밋과 변경 이력은 Git, 실행 결과는 해당 커밋의 Actions를 따른다.

## 다음 작업

1. 첫 Core CI 결과를 확인하고 실제 실패가 있으면 해당 소스만 수정한다.
2. S1: MiniFuse 실제 모델·OS·드라이버·채널 정보를 확인한 뒤, 선택한 마이크를
   헤드폰/검증된 송출 경로에 보내는 Windows 입출력 한 단위를 구현한다.
3. 장치 경로 위에 보컬 DSP와 VST3를 추가한다. 빈 모듈·복구 체계를 미리 만들지 않는다.
