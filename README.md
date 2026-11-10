# MixLive · 믹스라이브

**내 소리를 만들고, 누구에게 들려줄지 정한다.**

MixLive는 Windows용 실시간 오디오 라우팅·믹싱·보정 앱을 만드는 프로젝트다.
마이크·반주·게임·통화 소리를 선택적으로 섞고, 입력과 최종 믹스에 내장 보정 및
사용자가 소유한 VST3를 적용한다. DAW의 타임라인은 만들지 않는다.

> **현재: 라우팅 코어 + Windows 장치 경로 개발용 CLI.**
> 고품질 보컬 보정 앱이나 가상 마이크 드라이버가 완성된 상태는 아니다.
> 실제 검사와 실장비 미검증 범위는 [STATUS.md](STATUS.md)를 따른다.

## 작성된 경로

- `mixlive-core`: 장치·UI 독립 mono/stereo, source/send/mix gain, mute, 믹스마이너스.
- `mixlive-audio-windows`: WASAPI endpoint ID·기본 포맷 조회, 명시적 입력 채널,
  하나 또는 두 개의 독립 출력, 출력별 채널 매핑·음량, CPAL 장치 스트림.
- 출력마다 bounded SPSC queue와 Rubato sinc resampler. 서로 다른 샘플레이트와
  클럭 차이를 처리하며, 장치 오류·overflow에는 무음으로 멈추고 다른 장치를 고르지 않는다.
- 사전 할당 버퍼로 처리한다. 입력·출력 peak, queue 깊이, 보정 ppm, underrun,
  sample clipping 횟수를 보여준다. 이를 실제 왕복 지연이나 음질 점수로 표시하지 않는다.

**아직 없는 것:** ASIO, 앱별 캡처, 가상 장치 설치, VST3, 보컬 DSP, GPUI 화면,
Scene 저장과 실시간 변경. 기본 gain과 출력 clipping guard는 보컬 보정·limiter가 아니다.

## Windows에서 장치 경로 확인

Rust stable로 빌드하거나 해당 커밋의 Actions 개발용 artifact를 사용한다.
설치·드라이버·시스템 기본 장치 변경 없이 동작한다. 개발 바이너리는 서명된 출시 제품이 아니다.

```powershell
cargo build --release -p mixlive-audio-windows --bin mixlive-audio --locked
.\target\release\mixlive-audio.exe devices
```

`devices`의 **INPUT ID**와 **OUTPUT ID**를 그대로 넣는다. `check`는 스트림을 시작하지 않는다.

```powershell
.\target\release\mixlive-audio.exe check --input "<INPUT ID>" --input-channel 1 --output "<OUTPUT ID>" --output-channels 1,2
.\target\release\mixlive-audio.exe run --input "<INPUT ID>" --input-channel 1 --output "<OUTPUT ID>" --output-channels 1,2 --gain-db -18 --seconds 30
```

반드시 헤드폰을 낮은 하드웨어 음량으로 사용하고 기존 DAW/Windows/인터페이스의
중복 모니터링을 끈다. 출력 sample clipping guard는 청력·피드백·true peak를 보장하지 않는다.

두 번째 출력은 `--send "<별도 OUTPUT ID>" --send-channels 1,2 --send-gain-db -6`으로 추가한다.
이미 설치된 가상 케이블을 쓸 때는 그 **재생 endpoint**로 보내고, Discord/OBS에서는
대응 **녹음 endpoint**를 선택한다. 앱이 케이블을 설치하거나 두 endpoint를 자동 식별하지 않는다.
동일 endpoint를 두 번 여는 경로는 거절한다.

채널 번호는 **선택한 WASAPI endpoint 안의 1-based 채널**이다. MiniFuse ASIO의
3/4, 제조사 루프백, 물리 단자와 같다고 가정하지 않는다. 2채널 장치에서 3/4를 선택하면
시작 전에 오류가 난다. mono 출력에는 `--output-channels 1`을 사용한다.
장치가 제공하는 기본 샘플레이트/포맷으로 열며, 채널이 숨겨진 ASIO 기능은 아직 노출하지 않는다.

`--queue-ms`(기본 30)는 출력별 queue 목표다. 드라이버 버퍼·운영체제·장치 변환 지연이
추가되므로 저지연 공연 보장이 아니다. 먼저 안정된 신호를 확인하고 실제 지연을 측정한다.
`run`은 기본 30초 후 종료되며 `--seconds`로 최대 3600초를 지정할 수 있다. 녹음 파일은 만들지 않는다.

## 개발 검사

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo run -p mixlive-core --example discord_singing --locked
```

합성 예제는 마이크 0.1 + MR 0.2를 통화 송신에, 상대 음성 0.3까지 헤드폰에 보낸다.
기대 peak는 각각 0.300 / 0.600이다. 실제 Discord 연결을 검증하는 예제가 아니다.
Windows CI는 portable tests와 Windows 어댑터 빌드·CLI 도움말만 검사하며 마이크를 열지 않는다.

## 제품·개발 계약

[DESIGN.md](DESIGN.md) · [STATUS.md](STATUS.md) · [AGENTS.md](AGENTS.md) · [NOTICE.md](NOTICE.md)

저장소는 공개다. 제품 라이선스는 미정이며 공식 설치 파일·가상 장치·드라이버 릴리스는 없다.
