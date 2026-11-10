# 현재 상태

기준일: 2026-11-10
단계: **VST3 검사·상태 복원 Windows 검증 완료. WAV 처리 추가·통합 검사 대기**

## 이번 묶음

- 기존 live mic·내장 보컬·두 출력 경로는 변경하지 않았다.
- `mixlive-plugins render`: 지정 WAV에 stereo VST3 효과 적용, 파일 sample rate 유지,
  mono 복제, float headroom, 부분 블록·명시적 tail, 기존 class/state/parameter 경로 재사용.
- 외부 코드는 기존 worker 경계에서 실행. 부모는 worker의 정상 종료 후에만 새 WAV를 게시한다.
  실패·timeout·잘린 WAV·비유한 sample·추가 bus는 오류이며 dry 우회·자동 재시도는 없다.
- 기존 native fixture 두 개를 재사용한다. 일반 검사 3개와 WAV native 검사 2개 추가.
- Audio #5 artifact의 실제 Cargo.lock을 기준으로 hound 3.5.1만 추가하고 --locked를 복원했다.
  추가 checksum과 runtime 무의존성은 crates.io 공식 index에서 확인했다.

## 실행 근거

- 기준 `b83c38876b8fa2ba0915a157730c3fef038363b5`의
  [Audio #5](https://github.com/andongmin94/mix-live/actions/runs/42831880830)는 success다.
- 내려받은 #5 artifact SHA256 `1aacddd5227b7213d028431a9408b8ad154528ac8238fe1636b92cd8db0fa5b0` 일치.
- 새 WAV 코드의 컴파일·수치·native 검사 결과는 확인 전이다. 결과 확인 후 이 파일만 갱신한다.
- 로컬에 cargo/rustc가 없고 직접 GitHub/Rust 다운로드 DNS가 실패해 Windows Actions로 검사한다.

## 미완료

- MiniFuse 실제 장비·채널·장시간 dropout·왕복 지연, 실제 목소리 청취 수용.
- VST3 live callback·원본 editor, GPUI, 앱 캡처, 자체 가상 장치, ASIO, Scene 저장.
- offline WAV 성공을 live 보정 완료로 취급하지 않는다. plugin 신고 latency는 보상하지 않는다.
- 프리셋 음질은 미수용 시작값이다. 자동 음성 녹음·클라우드 업로드·driver 설치·릴리스 없음.

## 다음

WAV native 검증 결과와 lock을 확정한다. 그 뒤 실제 음성으로 내장/외부 보정의 음량 일치
청취 평가를 지원하고, VST3 lifecycle·실시간 처리 경계를 GPUI 제어와 연결한다.
현재 상태는 이 파일, 변경 이력은 Git, 실행 증거는 해당 commit의 Actions를 따른다.
