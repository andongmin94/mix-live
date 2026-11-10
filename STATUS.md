# 현재 상태

기준일: 2026-11-10
단계: **내장 보컬 체인 Windows 검사 통과. VST3 비실시간 검사·상태 도구 구현, 통합 검사 대기**

## 현재 구현

- Windows WASAPI mic→내장 보컬 체인→독립 출력 1~2개. HPF/tone filters, RMS soft-knee
  compressor, split-band de-esser, off/natural/warm/bright, 입력 trim/출력 음량 분리.
- 새 `mixlive-plugins`: 경로 목록, 지정한 VST3의 별도 프로세스 검사, 클래스·파라미터·버스·
  신고 지연·합성 처리 결과. normalized parameter 변경, component/controller 상태 저장/복원.
- 저장은 새 파일에만 원자적으로 수행. 클래스/포맷 불일치·잘못된 parameter·native failure는
  오류로 보고하며 원본 마이크 송출이나 자동 재시도로 바꾸지 않는다.
- VST3는 검사 도구에만 사용한다. 기존 `mixlive-audio`의 실시간 경로에는 연결하지 않았다.

## 실행 근거

- 보컬 코드 `aadd03ccbc323d719d217696bc12fa80016b6ff2`의
  [Audio #4](https://github.com/andongmin94/mix-live/actions/runs/42819662786)는 success다.
  이전 회차가 중단되어 기록하지 못했던 결과를 이번에 확인했다.
- 이번 VST3 변경의 컴파일·테스트·native fixture 결과는 아직 확인 전이다.
- 기존 Windows CI job 하나에 작은 설정/파일 검사와 source-built native fixture 검사만 추가했다.
  새 검증 프레임워크·오디오 장비 검사·유료 플러그인 설치는 추가하지 않았다.
- 로컬에는 Rust 도구체인이 없고 GitHub 직접 연결 DNS도 실패해 원격 Actions에서 검증한다.

## 경계와 미완료

- S1 MiniFuse 실제 모델·채널·driver, clock drift/dropout, 음성 청취와 왕복 지연은 미수용.
- 내장 프리셋은 청취 미수용 시작값이다. sample clipping guard는 전문 limiter가 아니다.
- VST3 native editor·라이브 오디오 연결은 미구현. 선택한 라이브러리의 realtime runner에
  내부 mutex가 남아 있어 callback에 넣지 않았다. 합성 offline 처리 성공과 실시간 수용은 다르다.
- ASIO, 앱별 capture, 자체 가상 장치, GPUI, 공간계/pitch, Scene 저장, 실행 중 설정 변경 미구현.
- plugin state에는 사용자 데이터가 들어갈 수 있어 저장소·artifact에 포함하지 않는다.
- 음성 녹음·클라우드 전송·드라이버 설치·공식 릴리스는 하지 않았다.

## 다음

검사 결과를 반영하고 실제 생성된 lockfile을 저장소에 고정한다. VST3 제어 경계를 GPUI
picker/editor 및 상태 저장에 연결하면서, 오디오 callback의 lock/할당·lifecycle 요구를
충족하는 live host 경로를 별도로 검증한다. 내장 보컬 체인의 실제 청취 수용도 남아 있다.
현재 상태는 이 파일, 이력은 Git, 실행 근거는 각 commit의 Actions를 따른다.
