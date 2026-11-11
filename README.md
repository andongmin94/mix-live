# MixLive · 믹스라이브

**내 소리를 만들고, 누구에게 들려줄지 정한다.**

MixLive는 Windows용 실시간 오디오 라우팅·믹싱·보정 앱을 만드는 프로젝트다.
마이크·반주·게임·통화 소리를 선택적으로 섞고, 입력과 최종 믹스에 내장 보정 및
사용자가 소유한 VST3를 적용하는 것이 목표다. DAW의 타임라인은 만들지 않는다.

> **현재: 라우팅 코어 + Windows 마이크/AUX 믹서 CLI + 내장 모노 보컬 체인 + 별도 VST3 검사·편집·WAV 도구.**
> VST3 검사는 비실시간이며 아직 마이크 체인에 연결하지 않았다.
> 완성된 방송 앱·가상 마이크 드라이버가 아니다. 실제 검사와 실장비·청취 미검증 범위는
> [STATUS.md](STATUS.md)를 따른다.

## 구현 경로

```text
선택한 마이크 → 입력 trim → HPF/EQ → 컴프레서 → 디에서 → 원음/보정 blend
    → 목적지별 보내기 ─┐
선택적 AUX → AUX trim ─┴→ 믹스 합산 → 최종 출력 음량 → 출력별 clock bridge → 헤드폰/송출
(AUX가 별도 입력 장치면 먼저 기존 clock bridge로 마이크 처리 클럭에 맞춘다.)
```

- `mixlive-core`: mono/stereo, source/send/mix gain, mute, 믹스마이너스.
- `mixlive-effects`: 장치·UI 독립 모노 보컬 처리. biquad 0.6.0 필터, RMS soft-knee
  compression, 고역 비중과 레벨을 함께 보는 split-band de-essing. 자동 makeup gain 없음.
- `mixlive-audio-windows`: WASAPI endpoint ID·기본 포맷, 명시적 입력 채널,
  하나 또는 두 개의 독립 믹스와 채널 매핑. 같은 endpoint의 서로 다른 채널 쌍도 지원한다.
  같은 보컬 체인을 두 믹스에서 중복 처리하지 않는다. AUX는 보컬 체인을 거치지 않는 별도 stereo 소스다.
- 물리 출력 endpoint마다 bounded SPSC queue와 Rubato sinc resampler. 샘플레이트·클럭 경계를 처리하며
  장치 오류·overflow에는 무음으로 멈춘다. 다른 장치를 임의로 선택하지 않는다.
- 오디오 callback에서는 사전 준비된 필터와 버퍼만 사용한다. peak, gain reduction,
  queue, correction ppm, underrun, clipping 횟수는 제어 스레드에서 표시한다.
- `mixlive-vst3`: 설치 경로 조회, 선택한 VST3의 별도 프로세스 로딩·합성 처리·파라미터
  및 상태 저장/복원. `mixlive-audio`의 callback과 별개다.

**미구현:** ASIO, 앱별 캡처, 자체 가상 장치, VST3 라이브 연결,
reverb/delay, pitch correction, GPUI 화면, 앱/VST3까지 포함한 전체 Scene, 실행 중 설정 변경.
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
물리 단자와 같다고 가정하지 않는다. 없는 채널과 같은 endpoint 안에서 겹치는 채널 선택은 거절한다.
mono 출력은 `--output-channels 1`. 장치 기본 샘플레이트와 포맷으로 연다.
`--queue-ms` 기본 30은 queue 목표이지 드라이버 버퍼나 왕복 지연이 아니다.
`run`은 기본 30초 후 끝나며 최대 `--seconds 3600`. 음성 녹음 파일은 만들지 않는다.

### 같은 장치의 1/2와 3/4로 보내기

선택한 WASAPI **endpoint의 기본 구성이 4채널 이상일 때만** 다음 구성이 가능하다.
같은 OUTPUT ID를 두 번 지정하되 채널을 분리한다. `check`로 실제 채널 수를 먼저 확인한다.

```powershell
.\target\release\mixlive-audio.exe check --input "<INPUT ID>" --output "<같은 OUTPUT ID>" --output-channels 1,2 --send "<같은 OUTPUT ID>" --send-channels 3,4 --send-gain-db -6
.\target\release\mixlive-audio.exe run --input "<INPUT ID>" --output "<같은 OUTPUT ID>" --output-channels 1,2 --send "<같은 OUTPUT ID>" --send-channels 3,4 --send-gain-db -6 --vocal natural --seconds 30
```

이때 `check`는 `playback streams: 1`과 두 mix의 채널 매핑을 표시한다. 두 mix의 음량은
각각 적용하지만, 한 프레임에 묶어 **같은 queue·4채널 리샘플러·출력 스트림**을 사용한다.
두 채널 쌍이 따로 버퍼를 소비하거나 서로 다른 drift 보정을 받지 않는다. 다른 endpoint를
선택하면 기존처럼 각 endpoint의 clock bridge를 사용한다. 통계의 `stream0`/`stream1`은
물리 스트림 단위이며, 공유 출력의 peak·clipping 횟수는 두 mix를 합친 통계다.

출력 채널이 겹치면 실행 전에 거절한다. 예를 들어 같은 endpoint의 `1,2`와 `2,3`은
허용하지 않는다. mono 채널 또는 역순 stereo 매핑도 가능하며 미지정 채널은 무음이다.
Windows에 stereo endpoint만 보이면 `3,4`를 만들 수 없다. 제조사 ASIO 채널·하드웨어
loopback을 자동 노출하거나 설정하지 않으며, MiniFuse 모델별 동작은 실장비 검증이 남아 있다.

## 마이크 + MR/AUX를 선택적으로 섞기

`--aux-input`은 **INPUT 목록의 캡처 endpoint**다. 인터페이스의 실제 루프백/라인 입력 또는
이미 설치된 가상 케이블의 녹음 쪽을 지정한다. Chrome 프로세스를 직접 잡거나 Windows 앱별
재생 장치를 자동으로 바꾸는 기능이 아니다. 소스에는 의도한 MR만 넣고, MixLive 출력이나
Discord 수신 음성이 섞인 전체 루프백을 되돌리지 않는다. 외부 피드백은 자동 검출하지 않는다.

| 옵션 | 의미 |
|---|---|
| `--aux-input "<AUX INPUT ID>"` | 선택한 입력만 명시적으로 연다. 생략하면 마이크 경로만 실행 |
| `--aux-channels 1,2` | 입력 내부 1-based stereo pair; 한 번호면 mono 복제, `2,1`이면 좌우 반전 |
| `--aux-gain-db -12` | AUX만 줄이는 source trim, -96..0 dB. 마이크 보정과 별도 |
| `--aux-to send` | 기본값. `--send` 믹스에만 합쳐 기존 MR 모니터와 중복하지 않음 |
| `--aux-to output` | 첫 출력에만 합침. 두 번째 출력에는 보정된 마이크만 보냄 |
| `--aux-to both` | 두 믹스에 합침. MR을 다른 경로로 듣지 않을 때만 선택 |

```powershell
.\target\release\mixlive-audio.exe check --input "<MIC INPUT ID>" --output "<HEADPHONE OUTPUT ID>" --send "<CALL OUTPUT ID>" --aux-input "<MR CAPTURE ID>" --aux-channels 1,2 --aux-to send --vocal natural
.\target\release\mixlive-audio.exe run --input "<MIC INPUT ID>" --output "<HEADPHONE OUTPUT ID>" --send "<CALL OUTPUT ID>" --aux-input "<MR CAPTURE ID>" --aux-channels 1,2 --aux-to send --vocal natural --seconds 30
```

보컬 EQ/컴프레서/디에서는 마이크에만 적용된다. AUX 좌우와 float headroom은 source trim까지
보존하고, 두 소리를 합친 뒤 출력 음량과 최종 sample clipping guard를 적용한다.
`--send-gain-db`는 **그 믹스에 포함된 마이크와 AUX 모두**에 적용한다.

같은 INPUT ID에서 마이크 `1`과 AUX `3,4`를 선택하면 한 입력 스트림에서 추출한다.
이 경우 실제 endpoint가 필요한 채널을 노출해야 하며 마이크/AUX 채널이 겹치면 거절한다.
별도 INPUT ID를 사용하면 AUX용 bounded queue와 기존 sinc resampler로 마이크 클럭에
맞춘다. **AUX에는 입력 queue·리샘플러 지연이 추가**되며 이것은 두 소스의 녹음 시각이나
외부에서 직접 듣는 MR과의 위상/시간 정렬을 보장하지 않는다. 마이크 경로는 지연 보상하지 않는다.

AUX가 아직 priming 중이거나 일시적으로 부족하면 AUX만 무음이고 마이크는 계속된다.
AUX 장치 오류·잘못된 sample·queue overflow는 전체 경로를 중단한다. 다른 입력으로 바꾸지 않는다.
`aux_peak`는 AUX trim 이전 sample peak, 별도 장치의 `aux_queue`/`aux_correction`/`aux_underruns`는
그 입력 clock boundary의 통계다. 입력 오류·실장비 동작·청취·왕복 지연 검증은 STATUS를 따른다.

## 마이크 경로를 Scene으로 저장·재실행

현재 마이크 입력, 채널, 헤드폰/송출 endpoint, 각 음량, 내장 보컬 프리셋·threshold·강도,
선택적 AUX 장치·채널·목적지·음량과 queue 목표를 하나의 로컬 JSON으로 저장한다. `scene save`는 파일만 만들고, `scene show`는
파일 형식과 값을 검사해 출력한다. 이 두 명령은 장치를 조회하거나 마이크를 시작하지 않는다.

```powershell
.\target\release\mixlive-audio.exe scene save singing.mixlive-scene.json --input "<INPUT ID>" --output "<OUTPUT ID>" --vocal natural --output-gain-db -18
.\target\release\mixlive-audio.exe scene show singing.mixlive-scene.json
.\target\release\mixlive-audio.exe scene check singing.mixlive-scene.json
.\target\release\mixlive-audio.exe scene run singing.mixlive-scene.json --seconds 30
```

`save`에는 기존 `check/run`과 동일한 route 옵션을 사용한다. 두 출력도 저장하려면
`--send "<별도 OUTPUT ID>" --send-channels 1,2 --send-gain-db -6`을 함께 지정한다.
같은 endpoint의 `1/2`·`3/4` 구성도 동일한 Scene 형식으로 저장·검사·재실행한다.
기존 파일은 덮어쓰지 않으며 다른 이름으로 저장한다. **AUX 필드를 포함한 `schema: 2`만 읽는다.**
이전 개발 Scene은 현재 옵션으로 새로 저장해야 한다. 구식 포맷 변환이나
누락된 필수 값의 기본값 복원은 하지 않는다. 오타·중복 채널·허용 범위 밖 음량은 거절한다.

`scene check`는 **현재** Windows 장치와 채널·포맷을 다시 확인하되 스트림은 시작하지 않는다.
`scene run`도 같은 확인을 거쳐야 시작한다. 저장한 장치가 없어졌다고 기본 마이크로 바꾸지 않는다.
샘플레이트/포맷은 저장 당시 값으로 강제하지 않고 선택한 endpoint의 현재 기본 구성을 사용한다.
실행 시간은 Scene에 저장하지 않으며 매번 기본 30초, `--seconds` 범위는 1..3600이다.

이것은 현재 구현된 **마이크 하나·선택적 AUX 하나·출력 1~2개의 장치/보컬 Scene**이다. 앱별 소스,
외부 VST3 상태, 임의 라우팅 그래프 또는 실행 중 Scene 전환을 구현했다고 의미하지 않는다.
음성은 저장하지 않는다. Scene에는 개인 endpoint ID가 있으므로 공유하지 않는 것이 좋고,
문서의 `*.mixlive-scene.json` 확장자는 `.gitignore`로 제외한다.

## 사용자 VST3 검사·편집

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

원본 편집창을 열어 설정한 뒤 새 상태 파일로 저장할 수 있다.

```powershell
.\target\release\mixlive-plugins.exe edit "C:\Plugins\Example.vst3" --state-out voice.mixlive-vst3.json
.\target\release\mixlive-plugins.exe render "C:\Plugins\Example.vst3" --input dry.wav --output wet.wav --state-in voice.mixlive-vst3.json
```

`edit`는 사용자 플러그인의 실제 창을 열지만 소리 미리듣기나 실시간 마이크 연결은 하지 않는다.
`--state-out`을 지정하면 창을 정상적으로 닫은 뒤 저장하고, 생략하면 편집 결과를 버린다.
기존 파일은 덮어쓰지 않는다. GPUI 메인 화면과는 별개의 네이티브 플러그인 창이다.

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
