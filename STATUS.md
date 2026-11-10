# 현재 상태

기준일: 2026-11-10
단계: **S2 내장 보컬 체인 구현·통합 검사 대기. S1 실장비 수용은 미완료**

## 이번 변경

- `mixlive-effects`: HPF + 3 tone filters, RMS soft-knee compressor, split-band de-esser.
- off/natural/warm/bright, dry/wet strength, 입력 trim, 압축/디에서 threshold.
- 입력 trim → 보정 한 번 → 독립 sends → 최종 출력 감쇠. 원음 dry fallback 없음.
- CLI 보정 옵션과 보정 후 peak·압축/디에서 억제량. 실행 중 설정 변경은 아직 없음.
- `--gain-db` 제거, `--input-gain-db`와 `--output-gain-db`로 역할 분리. 출력 기본 -18 dB.
- biquad 0.6.0과 필요한 libm 0.2.16만 lock에 추가. 기존 버전은 변경하지 않음.
- 새 DSP 검사 7개, 실제 capture 경로의 DSP/두 출력 연결 검사 1개. 기존 CI job 사용.

## 검증 상태

- 이번 변경의 컴파일·테스트·Windows exe 결과는 아직 확인 전이다.
- 로컬 Rust 도구체인이 없고 GitHub 직접 연결 DNS도 실패했다. 원격 GitHub Actions에서 검사한다.
- S1 기존 검증: `f11be5f044d43cda1dc948fe072a0735c81feafc`,
  [Audio #3](https://github.com/andongmin94/mix-live/actions/runs/42789882779):
  core 8 + audio 6 통과, Clippy·Windows release 빌드·도움말 실행 성공.
- 이번 신규 검사는 bypass/float headroom, HPF response, compressor knee/ratio/release,
  high-band de-essing, block-size independence, invalid-input silence, sends 이전 1회 DSP를 대상으로 한다.

## 아직 수용하지 않은 범위

- MiniFuse 모델·채널·driver, 장시간 실제 clock drift/dropout, 실제 음성 청취, 왕복 지연.
- 프리셋은 청취 미수용 시작값이다. 수치 검사 통과를 고품질·상용 플러그인 동급이라 부르지 않는다.
- sample clipping guard는 transparent/true-peak limiter가 아니다. DSP에 lookahead가 없다고
  장치·queue 지연이나 IIR group delay까지 0이라고 쓰지 않는다.
- ASIO, Chrome/app capture, 자체 가상 장치, GPUI, VST3, 공간계, pitch correction,
  Scene/플러그인 상태 저장, 실시간 parameter 변경은 미구현. S2 전체 완료가 아니다.
- 음성 녹음·클라우드 전송·드라이버 설치·공식 릴리스를 하지 않는다.

## 다음

이번 통합 검사 결과를 반영한 뒤 VST3 effect 1개의 실제 로딩/처리/상태 저장 경로를 만든다.
보컬 프리셋의 품질 수용은 사용자가 동의한 실제 목소리의 음량 일치 청취와 장비 지연 측정으로 별도 수행한다.
현재 기록은 이 파일, 이력은 Git, 실행 증거는 각 커밋의 Actions를 따른다.
