# MixLive · 믹스라이브

**내 소리를 만들고, 누구에게 들려줄지 정한다.**

Windows용 실시간 마이크·MR 라우팅/믹싱/보정 앱을 만드는 프로젝트다. DAW 타임라인이나
OBS 전용 플러그인이 아니다. 현재는 **명령줄 개발 빌드**이며 GPUI 메인 화면·자체 가상 마이크·
ASIO·앱별 직접 캡처는 아직 없다. 실제 검증 결과와 미검증 범위는 [STATUS.md](STATUS.md)를 따른다.

## 현재 처리 경로

```text
마이크 → 입력 trim → 내장 보컬 보정 → 순서대로 연결한 stereo VST3 체인 → 목적지별 보내기 ─┐
AUX/MR → 필요 시 입력 clock bridge → AUX trim → 목적지별 보내기 ─────────────┴→ 믹스
믹스 → 출력 clock bridge → master/send 음량·mute → sample clipping guard → 출력 장치
```

보컬은 한 번만 처리하고, AUX에는 보컬 DSP/VST3를 걸지 않는다. 출력 믹스는 최대 두 개다.
동일 endpoint의 분리 채널은 하나의 스트림/클럭을 공유하며 별도 장치는 bounded queue와
Rubato sinc resampler로 연결한다. 오류 시 임의 장치 선택이나 원음 마이크 우회는 하지 않는다.

VST3를 선택하면 장치 callback은 mic/AUX 프레임을 함께 bounded queue로 보낸다. 체인 전체의 전용 worker 하나가
내장 보컬→VST3 체인→믹싱을 수행한다. 플러그인마다 스레드나 오디오 queue를 추가하지 않는다. 로딩·editor·state·종료는 main/제어 스레드에서 한다.
호스트 내부 및 editor/control과의 mutex 공유는 남아 있으므로 **hard-real-time/무중단 보장은 없다.**
VST3 미선택 시 기존 직접 처리 경로를 사용한다. 처리 순서·품질 목표는 [DESIGN.md](DESIGN.md)에 있다.

## Windows에서 시작

Rust stable로 빌드하거나 해당 코드 커밋의 Actions 개발 artifact를 사용한다. 바이너리는 미서명이며
드라이버 설치나 시스템 기본 장치 변경을 하지 않는다.

```powershell
cargo build --release -p mixlive-audio-windows -p mixlive-vst3 --bins --locked
.\target\release\mixlive-audio.exe devices
.\target\release\mixlive-audio.exe check --input "<INPUT ID>" --output "<OUTPUT ID>" --vocal natural
.\target\release\mixlive-audio.exe run --input "<INPUT ID>" --output "<OUTPUT ID>" --vocal natural --output-gain-db -18 --seconds 30
```

`devices`는 목록만 보여주며 `check`는 장치·채널·보컬 설정만 검사한다. 실제 시작은 `run`이다.
명시한 endpoint ID를 사용하고, 채널 번호는 선택한 WASAPI endpoint 안의 **1-based** 번호다.
MiniFuse의 ASIO/루프백/물리 단자와 같다고 가정하지 않는다. `--help`로 전체 옵션을 확인한다.

**헤드폰 하드웨어 음량을 낮추고** DAW·Windows·인터페이스의 중복 모니터링을 끈다.
sample clipping guard는 전문 리미터, true-peak 제한, 음향 피드백 방지 또는 청력 보호 기능이 아니다.
기본 실행은 30초, `--seconds` 범위는 1..3600이다. 자동 음성 녹음은 하지 않는다.

### 내장 보컬

`--vocal off|natural|warm|bright`를 지원한다. 기본은 off다. 보정은 HPF/EQ→RMS soft-knee
컴프레서→split-band 디에서→원음/보정 blend이며 reverb/delay/pitch correction은 아직 없다.
이름은 시작 설정이지 실제 청취 수용을 마친 고급 프리셋이라는 뜻이 아니다.

| 옵션 | 적용 위치/범위 |
|---|---|
| `--input-gain-db 0` | 보컬 검출기 앞 trim, -36..18 dB. ADC clipping은 복원 못 함 |
| `--vocal-strength 1` | 원음/보정 blend 0..1. 음량 일치 기능 아님 |
| `--vocal-threshold-db -24` | 컴프레서 threshold -60..0 dBFS |
| `--deess-threshold-db -30` | 디에서 고역 threshold -60..0 dBFS |
| `--output-gain-db -18` | 두 믹스의 초기 최종 감쇠 -96..0 dB |
| `--send-gain-db 0` | 두 번째 믹스의 초기 추가 감쇠 -96..0 dB |

자동 makeup gain은 없다. A/B 청취 시 원본/보정본 음량은 따로 맞춰야 한다. 보컬 프리셋은
32..192 kHz, off는 8..192 kHz를 받는다. 입력 trim/보컬 파라미터는 실행 중 고정이다.

### 출력 장치와 채널

두 번째 믹스는 `--send "<OUTPUT ID>" --send-channels 1,2`로 지정한다. 이미 설치된 가상 케이블은
재생 endpoint로 보내고 Discord/OBS에서 대응 녹음 endpoint를 선택한다. MixLive가 케이블을
설치하거나 자동 연결하지 않는다. `output`/`send`는 앱 이름이 아니라 사용자가 지정한 믹스다.

같은 endpoint가 실제로 4채널 이상을 노출하면 다음 구성이 가능하다.

```powershell
.\target\release\mixlive-audio.exe check --input "<INPUT ID>" --output "<OUTPUT ID>" --output-channels 1,2 --send "<같은 OUTPUT ID>" --send-channels 3,4
```

`check`에 `playback streams: 1`과 두 매핑이 표시된다. 공유 queue·다채널 리샘플러를 사용해
두 경로를 함께 소비한다. 다른 endpoint는 각각의 스트림을 쓴다. 겹치는 채널은 거절하며
mono(`1`)·역순 stereo(`2,1`)를 지원하고 미지정 채널은 무음이다. stereo로만 노출된 장치에
ASIO 3/4를 만들어 주지 않는다. 통계의 `stream0`/`stream1`은 물리 출력 스트림 단위다.

### MR/AUX 입력

`--aux-input`은 **INPUT 목록의 캡처 endpoint**다. 인터페이스의 분리된 루프백/라인 입력이나
기존 가상 케이블의 녹음 쪽을 지정한다. Chrome 프로세스/PID나 특정 탭의 직접 캡처가 아니다.
의도한 MR만 선택하고 MixLive 출력·통화 수신음이 포함된 전체 루프백을 다시 넣지 않는다.
외부 피드백은 자동 검출하지 않는다.

```powershell
.\target\release\mixlive-audio.exe run --input "<MIC INPUT ID>" --output "<HEADPHONE OUTPUT ID>" --send "<CALL OUTPUT ID>" --aux-input "<MR CAPTURE ID>" --aux-channels 1,2 --aux-gain-db -12 --aux-to send --vocal natural --seconds 30
```

`--aux-to send|output|both`로 초기 목적지를 정한다. 기본 send-only는 이미 다른 경로로 듣는
MR의 이중 모니터를 피하기 위한 값이다. send/both는 `--send`가 필요하다. AUX gain은 -96..0 dB,
채널은 mono 복제·stereo·역순을 지원한다. 같은 INPUT ID의 mic 1/AUX 3,4는 한 캡처 스트림에서
추출하며 채널이 겹치면 거절한다. 실제 endpoint가 필요한 채널을 제공해야 한다.

별도 AUX 장치는 먼저 마이크 클럭으로 변환하므로 추가 queue/리샘플러 지연이 생긴다.
이는 두 소스의 녹음 시각이나 직접 듣는 MR과의 시간/위상 정렬을 보장하지 않는다.
AUX 부족은 AUX만 무음, 장치/비유한 sample/overflow 오류는 전체 경로 중단이다.

## Scene과 실행 중 조작

```powershell
.\target\release\mixlive-audio.exe scene save singing.mixlive-scene.json --input "<INPUT ID>" --output "<OUTPUT ID>" --vocal natural
.\target\release\mixlive-audio.exe scene show singing.mixlive-scene.json
.\target\release\mixlive-audio.exe scene check singing.mixlive-scene.json
.\target\release\mixlive-audio.exe scene run singing.mixlive-scene.json --interactive --seconds 3600
```

`save`는 run과 같은 route 옵션으로 **새 파일**을 만들며 `show`는 검사·출력만 한다. 둘 다
장치를 열지 않는다. check/run은 현재 장치와 채널을 다시 확인하고 기본 장치로 대체하지 않는다.
Scene schema 2는 mic/AUX/출력/보컬/queue 설정만 저장한다. 이전 형식은 새로 저장해야 하며
마이그레이션은 없다. VST3 launch 옵션·실행 시간·실시간 변경은 Scene에 자동 저장하지 않는다.
개인 endpoint ID가 포함된 `*.mixlive-scene.json`은 Git에서 제외한다.

`--interactive`를 지정했을 때만 터미널에 다음 명령을 입력한다.

| 명령 | 동작 |
|---|---|
| `mute/unmute mic|aux|output|send|all` | 소스·출력·전체 음소거 |
| `gain master -18`, `gain send -6` | 공통 출력/두 번째 믹스 추가 감쇠 |
| `route mic output off`, `route mic output on` | 내 목소리의 첫 믹스 포함 여부 |
| `route aux send off` | 두 번째 믹스의 MR만 제외 |
| `level aux send -6`, `level mic output -9` | 지정 경로만 추가 감쇠, -96..0 dB |
| `status`, `stop` | 요청된 제어 상태 / 장치 해제 후 종료 |

일반형은 `route mic|aux output|send on|off`, `level mic|aux output|send <dB>`다.
꺼둔 경로의 level 변경은 켜짐을 뜻하지 않고, 전체 unmute도 개별 mute를 풀지 않는다.
보컬은 한 번 처리하며 목적지별 gain은 합산 전에 적용한다. 출력 mute/master/send gain은
queue 뒤에 적용하고 mute 중에도 queue를 소비한다. 이미 OS에 보낸 소리나 Direct Monitor에는
작용하지 않는다. source/route mute에는 출력 queue 지연이 남으므로 급한 송출 중단은
`mute send`/`mute all`을 사용한다. 음소거는 장치의 마이크 접근 자체를 종료하지 않는다.

gain/route는 약 5 ms sample ramp를 사용한다. 전체 응답시간 보장이 아니며 설정은 현재 실행만
바꾼다. EOF도 종료 요청이다. `--interactive` 없이 실행하면 지정한 시간까지 동작한다.

## 여러 VST3를 순서대로 연결하고 조절하기

`--vst3`를 처리 순서대로 반복한다. 각 슬롯은 **단일 active stereo 입출력 버스**의 effect다.
마이크에 내장 보정→1번→2번→…을 한 번만 적용하고 그 결과를 각 믹스에 보낸다. AUX는 체인 전체를
우회한다. 같은 DLL을 두 번 지정해도 서로 다른 인스턴스/상태를 만든다. 중간 결과의 좌우를
다음 슬롯에 그대로 넘기며 mono로 다시 합치지 않는다. sidechain/multibus는 조용히 생략하지 않는다.

```powershell
.\target\release\mixlive-audio.exe scene run singing.mixlive-scene.json `
  --vst3 "C:\Plugins\EQ.vst3" --vst3 "C:\Plugins\Compressor.vst3" --vst3 "C:\Plugins\Reverb.vst3" `
  --vst3-editor 1 --vst3-editor 3 --vst3-chain-out voice.mixlive-chain.json `
  --interactive --seconds 3600
```

`--vst3-editor 1`은 **1번 슬롯의 실제 라이브 인스턴스** 원본 창을 연다. 원하는 슬롯마다
반복한다. 창 하나를 닫아도 체인과 다른 창은 유지된다. 창 다시 열기는 아직 없다. 없는 번호,
중복 번호, 편집창이 없는 슬롯 또는 열기 실패는 오디오 스트림 시작 전에 거절한다.

`--vst3-chain-out`을 지정한 정상 종료에서만 **모든 슬롯의 순서·절대 경로·class ID·opaque state**를
하나의 새 파일에 저장한다. 입력/출력 중지→worker join→모든 editor 제거→마지막 편집 flush→각
plugin processing stop→state 취득→모든 native 해제→파일 게시 순서다. 어느 슬롯이든 실패하면
부분 체인 파일을 만들지 않는다. 기존 파일은 보존하고 오류·restart·암묵적 Drop에서는 저장하지 않는다.
상태 취득 중 하나가 실패해도 나머지 슬롯의 종료를 수행한다. 생략하면 변경은 현재 실행에만 남는다.

다음 실행은 긴 플러그인 목록 대신 저장한 체인을 선택한다.

```powershell
.\target\release\mixlive-audio.exe scene run singing.mixlive-scene.json `
  --vst3-chain voice.mixlive-chain.json --vst3-editor 2 --interactive --seconds 3600
```

`--vst3` 목록과 `--vst3-chain`은 함께 지정하지 않는다. 수정 결과를 보존하려면 새 이름의
`--vst3-chain-out voice-edited.mixlive-chain.json`을 추가한다. 체인은 route Scene과 별도 파일이며
마이크 장치·MR·실시간 경로 변경까지 저장하는 전체 Scene은 아니다. 이 파일에 음성이나 DLL은
넣지 않는다. 플러그인 경로·상태가 들어가므로 `*.mixlive-chain.json`은 Git에서 제외한다.

특정 audio class를 선택하거나 수동으로 시작 구성을 만들 때는 아래 JSON을 사용할 수 있다.
`class_id`와 `state`는 생략 가능하지만 state가 있으면 해당 class ID도 반드시 있어야 한다.
플러그인 상대 경로는 현재 작업 폴더가 아니라 **체인 파일이 있는 폴더**를 기준으로 해석한다.
정상 실행 후 저장한 파일에는 절대 경로와 확인된 class ID가 들어간다.

```json
{
  "schema": 1,
  "effects": [
    { "plugin": "plugins/EQ.vst3" },
    { "plugin": "plugins/Reverb.vst3" }
  ]
}
```

이전 단일 플러그인용 `--vst3-state`, `--vst3-class`, `--vst3-state-out` 경로는 제거했다.
`--vst3-editor`는 이제 슬롯 번호가 필요하다. 별도 `mixlive-plugins` 도구의 단일 상태 파일은
그 도구에서 계속 사용하지만, 새 라이브 체인 파일로 자동 변환하지 않는다.

**모든 plugin restart 요청은 중단 사유다.** oversampling·bus·latency뿐 아니라 일부 preset/parameter
갱신도 중단될 수 있다. 자동 재구성·dry 우회·실행 중 체인 삽입/삭제/순서 변경은 없다.
메시지는 plugin mutex를 잡지 않은 main 스레드에서 처리한다. 나머지 제어/창 API는 worker와
mutex를 공유하므로 GUI 조작 중 dropout 없는 hard-real-time 동작을 보장하지 않는다.

체인은 in-process native 코드다. crash/hang은 격리되지 않으며 플러그인이 반환하지 않으면
종료도 기다릴 수 있다. 신뢰하고 적법하게 설치한 효과만 사용한다. 각 슬롯과 전체 체인의
**신고 지연 합계**를 표시하지만 실제 왕복 지연 측정이나 지연 보상은 아니다. 입력 queue 용량,
worker scheduling, 출력 queue, OS/장치 지연은 별도다. 실제 MiniFuse·상용 GUI·청취 수용은 남아 있다.

## 별도 VST3 검사·파일 도구

```powershell
.\target\release\mixlive-plugins.exe list
.\target\release\mixlive-plugins.exe inspect "C:\Plugins\Example.vst3"
.\target\release\mixlive-plugins.exe edit "C:\Plugins\Example.vst3" --state-out voice.mixlive-vst3.json
.\target\release\mixlive-plugins.exe render "C:\Plugins\Example.vst3" --input dry.wav --output wet.wav --state-in voice.mixlive-vst3.json
```

이 별도 도구의 `edit`는 오디오 미리듣기가 없다. 검사·편집·WAV 처리는 자식 프로세스에서
수행하며 native crash 분리이지 권한 sandbox가 아니다. 상태·파라미터·WAV 형식·잔향 옵션은
[VST3 안내](crates/mixlive-vst3/README.md)를 따른다. 성공한 inspect는 라이브 호환성 승인이 아니다.

## 검증·개발 계약

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo run -p mixlive-core --example discord_singing --locked
```

CI는 수치/CLI/상태/실제 native fixture와 Windows 빌드를 검사하며 실제 오디오 장치를 열지 않는다.
추가 native 검사는 기존 CI 명령을 따른다. sample peak/GR은 LUFS/true peak가 아니고
`queue-ms`/driver buffer/플러그인 신고 지연은 실측 왕복 지연이 아니다. 실제 장비·청취 수용과
자동 검사 결과를 구별한다. 내장 reverb/delay/pitch correction·기본 프리셋의 음질 수용도 남아 있다.

[DESIGN.md](DESIGN.md) · [STATUS.md](STATUS.md) · [AGENTS.md](AGENTS.md) · [NOTICE.md](NOTICE.md)

공개 저장소이며 제품 라이선스는 미정이다. 자동 녹음·클라우드 전송·드라이버 설치·공식 릴리스는 없다.
