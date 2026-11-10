# MixLive · 믹스라이브

**내 소리를 만들고, 누구에게 들려줄지 정한다.**

MixLive는 Windows용 실시간 오디오 라우팅·믹싱·보정 앱을 만드는 프로젝트다.
마이크, 반주, 게임, 통화 소리를 선택적으로 섞고, 입력 채널과 최종 믹스에
내장 보정 또는 사용자가 소유한 VST3를 적용한다. DAW의 타임라인을 만들지 않는다.

> **현재: 설계 + Rust 라우팅 기초 소스.**
> 완성된 앱, VST 호스트, 오디오 드라이버 또는 설치 파일이 아니다.
> 실행 검증 결과와 미구현 범위는 [STATUS.md](STATUS.md)에 기록한다.

## 첫 제품 경험

마이크를 선택하고, 듣기 좋은 보컬 체인을 고른다. Chrome의 MR을 추가한다.
`통화로 보내기`에는 목소리와 MR만, `방송으로 보내기`에는 게임·통화 수신음까지
원하는 소리만 넣는다. 헤드폰에서 들을 소리는 따로 정한다.

- **보정:** 사용자가 가진 VST3와 내장 EQ·다이내믹·디에서·공간계.
- **라우팅:** 입력별 처리와 목적지별 믹스, 통화 수신음의 되먹임을 막는 믹스마이너스.
- **출력:** 물리 장치, 검증된 루프백, 향후 MixLive 가상 마이크.
- **품질:** 쉬운 연결뿐 아니라 실제 청취 품질과 저지연이 제품의 합격 조건.

위 항목은 **제품 요구사항**이며, 현재 소스에 모두 구현되어 있다는 뜻이 아니다.

## 현재 소스 범위

`mixlive-core`는 외부 의존성이 없는 Rust 라이브러리다.
입력·출력 장치와 UI 없이 메모리의 오디오 버퍼를 섞는다.

구현한 코드: mono/stereo 입력, source/send/mix gain, mute, 독립 stereo 믹스,
믹스마이너스 제외 규칙, 설정 검증, 사전 할당 출력 버퍼, 가변 블록 처리, sample-peak 조회.
관련 테스트 8개와 합성 입력 예제를 포함한다.

미구현: 실제 장치 입력·출력, 앱 캡처, 가상 장치, VST3, 보컬 DSP, 리샘플링,
지연 보상, 실시간 설정 변경, Scene 저장, GPUI 화면.
현재 core의 gain은 보컬 보정 기능의 대용품이 아니다.

## 개발 명령

Rust stable + Cargo가 설치된 환경에서 저장소 루트에서 실행한다.

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo run -p mixlive-core --example discord_singing --locked
```

예제는 장치를 열지 않고 합성 입력을 처리한다. 기대 출력:

```text
MixLive: synthetic routing only; no devices, effects or virtual microphone.
Discord send: sample peak 0.300
Headphones: sample peak 0.600
Expected: Discord send 0.300; headphones 0.600 (return excluded from send).
```

`0.1` 마이크 + `0.2` MR은 통화 송신에, `0.3` 상대 음성까지 더한 값은 헤드폰에 간다.
실제 마이크·Discord 연결이나 청취 품질을 검증하는 예제가 아니다.

## 설계·개발 규칙

- [DESIGN.md](DESIGN.md): 제품 범위, 오디오 구조, 장치 전략, DSP 품질, VST3, UI, 완료 조건.
- [STATUS.md](STATUS.md): 현재 사실, 실행한 검사, 바로 다음 작업. 작업 이력은 Git을 따른다.
- [AGENTS.md](AGENTS.md): 개발·검증·라이선스·배포 규칙.
- [NOTICE.md](NOTICE.md): 라이선스 결정 대기와 외부 코드 미포함 사실.

저장소는 `andongmin94/mix-live`에서 공개한다. 제품 라이선스는 아직 결정하지 않았고,
설치 파일·패키지·드라이버 릴리스는 제공하지 않는다.
