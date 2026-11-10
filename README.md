# MixLive · 믹스라이브

**내 소리를 만들고, 누구에게 들려줄지 정한다.**

MixLive는 Windows용 실시간 오디오 라우팅·믹싱·보정 앱을 만드는 프로젝트다.
마이크·반주·게임·통화 소리를 선택적으로 섞고, 입력과 최종 믹스에 내장 보정 및
사용자가 소유한 VST3를 적용하는 것이 목표다. DAW의 타임라인은 만들지 않는다.

> **현재: 라우팅 코어 + Windows 장치 CLI + 내장 모노 보컬 체인 + 별도 VST3 검사 도구.**
> VST3 검사는 비실시간이며 아직 마이크 체인에 연결하지 않았다.
> 완성된 방송 앱·가상 마이크 드라이버가 아니다. 실제 검사와 실장비·청취 미검증 범위는
> [STATUS.md](STATUS.md)를 따른다.

## 구현 경로

```text
선택한 마이크 → 입력 trim → HPF/EQ → 컴프레서 → 디에서 → 원음/보정 blend
    → 목적지별 보내기 → 최종 출력 음량 → 출력별 clock bridge → 헤드폰/기존 가상 케이블
```

- `mixlive-core`: mono/stereo, source/send/mix gain, mute, 믹스마이너스.
- `mixlive-effects`: 장치·UI 독립 모노 보컬 처리. biquad 0.6.0 필터, RMS soft-knee
  compression, 고역 비중과 레벨을 함께 보는 split-band de-essing. 자동 makeup gain 없음.
- `mixlive-audio-windows`: WASAPI endpoint ID·기본 포맷, 명시적 입력 채널,
  하나 또는 두 개의 독립 출력과 채널 매핑. 같은 보컬 체인을 두 출력에서 중복 처리하지 않는다.
- 출력마다 bounded SPSC queue와 Rubato sinc resampler. 샘플레이트·클럭 경계를 처리하며
  장치 오류·overflow에는 무음으로 멈춘다. 다른 장치를 임의로 선택하지 않는다.
- 오디오 callback에서는 사전 준비된 필터와 버퍼만 사용한다. peak, gain reduction,
  queue, correction ppm, underrun, clipping 횟수는 제어 스레드에서 표시한다.
- `mixlive-vst3`: 설치 경로 조회, 선택한 VST3의 별도 프로세스 로딩·합성 처리·파라미터
  및 상태 저장/복원. `mixlive-audio`의 callback과 별개다.

**미구현:** ASIO, 앱별 캡처, 자체 가상 장치, VST3 라이브 연결·원본 편집창,
reverb/delay, pitch correction, GPUI 화면, Scene 저장, 실행 중 설정 변경.
출력 clipping guard는 전문 리미터가 아니다.

## Windows에서 사용

Rust stable로 빌드하거나 해당 커밋의 Actions 개발 artifact를 사용한다.
드라이버 설치나 시스템 기본 장치 변경은 하지 않는다. 개발 바이너리는 미서명이다.

```powershell
cargo build --release -p mixlive-audio-windows --bin mixlive-audio --locked
.\target\release\mixlive-audio.exe devices
```

표시된 INPUT/OUTPUT ID를 넣는다. `check`는 장치 설정과 보컬 계수만 검사하고 스트림을 시작하지 않는다.

```powershell
.\target\release\mixlive-audio.exe check --input "<INPUT ID>" --output "<OUTPUT ID>" --vocal natural
.\target\release\mixlive-audio.exe run --input "<INPUT ID>" --input-channel 1 --output "<OUTPUT ID>" --output-channels 1,2 --vocal natural --output-gain-db -18 --seconds 30
```

반드시 헤드폰을 낮은 하드웨어 음량으로 사용하고 DAW/Windows/인터페이스의 중복 모니터링을 끈다.
출력 sample clipping guard는 음향 피드백·청력 보호·true peak를 보장하지 않는다.

### 보컬 설정

| 옵션 | 역할 |
|---|---|
| `--vocal off` | 기본값. 보정 우회. 입력 trim과 출력 음량은 계속 적용 |
| `--vocal natural` | 75 Hz HPF, 약한 presence, 2.5:1 compression, 최대 4 dB 고역 억제 |
| `--vocal warm` | 65 Hz HPF, 저역 shelf +1.5 dB, 고역 shelf -1 dB, 2.5:1 compression |
| `--vocal bright` | 85 Hz HPF, 저역 shelf -1.5 dB, presence +2 dB, 고역 shelf +1.5 dB, 3:1 compression, 최대 6 dB 고역 억제 |
| `--input-gain-db 0` | 보정 검출기 앞 디지털 입력 trim, -36..18 dB |
| `--output-gain-db -18` | 보정 뒤 두 믹스의 최종 음량, -96..0 dB |
| `--vocal-threshold-db -24` | 컴프레서 RMS threshold, -60..0 dBFS |
| `--deess-threshold-db -30` | 디에서 고역 threshold, -60..0 dBFS |
| `--vocal-strength 1` | 원음/보정 blend, 0..1. 음량 일치 기능이 아님 |

이름은 사운드 설계의 **출발값**이며 청취 수용을 마친 고급 프리셋이라는 뜻이 아니다.
입력 trim은 ADC에서 이미 찌그러진 소리를 복원하지 못한다. 기본 -18 dB 출력 감쇠는
컴프레서 뒤에 있어 검출기를 둔감하게 만들지 않는다. 기존 `--gain-db`는 제거했다.
압축 후 소리가 작아질 수 있으며 이를 숨기려고 자동 makeup gain을 더하지 않는다.
A/B 청취는 원본/보정본 음량을 별도로 맞춰 평가해야 한다.

보컬 프리셋의 입력은 32..192 kHz, off는 8..192 kHz다. 출력 샘플레이트는 기존
clock bridge가 처리한다. 별도 lookahead buffer는 없지만 IIR의 위상·군지연과
장치·queue·리샘플러 지연이 있어 '무지연' 또는 실측 저지연이라고 표시하지 않는다.
`vocal_peak`는 최종 감쇠 전 sample peak, `comp_gr`/`deess_gr`는 직전 표시 이후 최대
gain reduction이다. blend 강도를 낮추면 이 값은 wet 경로의 억제량이지 최종 감쇠량이 아니다.

### 두 출력과 채널

두 번째 출력: `--send "<별도 OUTPUT ID>" --send-channels 1,2 --send-gain-db -6`.
가상 케이블이 이미 설치돼 있다면 그 **재생 endpoint**로 보내고 Discord/OBS에서 대응
**녹음 endpoint**를 선택한다. MixLive가 케이블을 설치하거나 자동 연결했다고 표시하지 않는다.

채널은 선택한 WASAPI endpoint 안의 1-based 번호다. MiniFuse ASIO 3/4, 제조사 루프백,
물리 단자와 같다고 가정하지 않는다. 없는 채널·같은 출력 endpoint 두 번 선택은 거절한다.
mono 출력은 `--output-channels 1`. 장치 기본 샘플레이트와 포맷으로 연다.
`--queue-ms` 기본 30은 queue 목표이지 드라이버 버퍼나 왕복 지연이 아니다.
`run`은 기본 30초 후 끝나며 최대 `--seconds 3600`. 음성 녹음 파일은 만들지 않는다.

## 사용자 VST3 검사

```powershell
cargo build --release -p mixlive-vst3 --bin mixlive-plugins --locked
.\target\release\mixlive-plugins.exe list
.\target\release\mixlive-plugins.exe inspect "C:\Program Files\Common Files\VST3\Example.vst3"
```

플러그인 고유 ID, 버스·파라미터·신고 지연, 합성 PCM 처리 결과를 확인한다.
`--set`, `--state-in`, `--state-out` 사용법과 제한은 [VST3 안내](crates/mixlive-vst3/README.md)에 있다.
검사 시 마이크/스피커를 열지 않으며, 성공해도 실시간 호환성이나 음질 수용을 뜻하지 않는다.
검사는 자식 프로세스에서 한 번 수행한다. native crash 분리이지 권한 sandbox가 아니다.
신뢰하고 적법하게 설치한 플러그인만 선택한다.

## 개발 검사

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo run -p mixlive-core --example discord_singing --locked
```

합성 routing 예제는 통화/헤드폰 peak 0.300 / 0.600을 계산한다. 실제 Discord가 아니다.
Windows CI는 core·clock·DSP 수치 검사, VST3 native fixture·상태 복원, 어댑터 빌드와 CLI 도움말을
검사하며 마이크를 열지 않는다. native fixture 검사는 별도 명령으로 실행되므로 상세 명령은
[VST3 안내](crates/mixlive-vst3/README.md)를 따른다. 빌드 통과와 실제 장비·청취 수용을 구분한다.

## 제품·개발 계약

[DESIGN.md](DESIGN.md) · [STATUS.md](STATUS.md) · [AGENTS.md](AGENTS.md) · [NOTICE.md](NOTICE.md)

저장소는 공개다. 제품 라이선스는 미정이며 공식 설치 파일·가상 장치·드라이버 릴리스는 없다.
