# MixLive · 믹스라이브

**내 소리를 만들고, 누구에게 들려줄지 정한다.**

Windows용 실시간 마이크·MR 라우팅/믹싱/보정 앱을 만드는 프로젝트다. DAW 타임라인이나
OBS 전용 플러그인이 아니다. 현재는 **명령줄 개발 빌드**이며 GPUI 메인 화면·자체 가상 마이크·
ASIO·앱별 직접 캡처는 아직 없다. 실제 검증 결과와 미검증 범위는 [STATUS.md](STATUS.md)를 따른다.

## 현재 처리 경로

```text
마이크 → 입력 trim → 내장 보컬 보정 → 순차 stereo VST3 체인 → 목적지별 보내기 ─┐
AUX/MR → 필요 시 입력 clock bridge → AUX trim → 목적지별 보내기 ─────────┴→ 믹스
믹스 → 출력 clock bridge → master/send 음량·mute → sample clipping guard → 출력 장치
```

보컬은 한 번만 처리하고 AUX에는 보컬 DSP/VST3를 걸지 않는다. 출력 믹스는 최대 두 개다.
동일 endpoint의 분리 채널은 하나의 스트림/클럭을 공유하며 별도 장치는 bounded queue와
Rubato sinc resampler로 연결한다. 오류 시 임의 장치 선택이나 원음 마이크 우회는 하지 않는다.

VST3를 선택하면 장치 callback은 mic/AUX 프레임을 함께 bounded queue로 보낸다. 체인 전체에
worker/입력 queue는 각각 하나다. 로딩·editor·state·종료는 main/제어 스레드에서 수행한다.
호스트 내부 및 editor/control mutex는 남아 있으므로 **hard-real-time/무중단 보장은 없다.**
VST3 미선택 시 기존 직접 처리 경로다. 제품 요구사항은 [DESIGN.md](DESIGN.md)를 따른다.

## Windows에서 시작

Rust stable로 빌드하거나 해당 코드 커밋의 Actions artifact를 사용한다. 미서명 개발 바이너리이며
드라이버 설치나 시스템 기본 장치 변경을 하지 않는다.

```powershell
cargo build --release -p mixlive-audio-windows -p mixlive-vst3 --bins --locked
.\target\release\mixlive-audio.exe devices
.\target\release\mixlive-audio.exe check --input "<INPUT ID>" --output "<OUTPUT ID>" --vocal natural
.\target\release\mixlive-audio.exe run --input "<INPUT ID>" --output "<OUTPUT ID>" --vocal natural --output-gain-db -18 --seconds 30
```

`devices`는 목록, `check`는 장치·채널·보컬 설정만 검사한다. 실제 시작은 `run`이다.
채널은 선택한 WASAPI endpoint 내부 **1-based** 번호이며 MiniFuse ASIO/루프백/물리 단자와
같다고 가정하지 않는다. 기본 실행은 30초, `--seconds`는 1..3600이다. 자동 녹음은 없다.
**헤드폰 하드웨어 음량을 낮추고** DAW·Windows·인터페이스의 중복 모니터링을 끈다.
sample clipping guard는 전문 리미터·true-peak·음향 피드백 방지·청력 보호 기능이 아니다.

### 내장 보컬

`--vocal off|natural|warm|bright` (기본 off). HPF/EQ→RMS soft-knee compression→split-band
디에서→원음/보정 blend다. 내장 reverb/delay/pitch correction은 아직 없다. 프리셋 이름은
시작 설정이며 실제 청취 수용을 마친 고급 사운드를 의미하지 않는다.

| 옵션 | 적용 위치/범위 |
|---|---|
| `--input-gain-db 0` | 보컬 검출기 앞 trim -36..18 dB. ADC clipping 복원 불가 |
| `--vocal-strength 1` | 원음/보정 blend 0..1. 음량 일치 기능 아님 |
| `--vocal-threshold-db -24` | 컴프레서 threshold -60..0 dBFS |
| `--deess-threshold-db -30` | 디에서 고역 threshold -60..0 dBFS |
| `--output-gain-db -18` | 두 믹스의 초기 최종 감쇠 -96..0 dB |
| `--send-gain-db 0` | 두 번째 믹스의 초기 추가 감쇠 -96..0 dB |

자동 makeup gain은 없다. A/B 청취의 원본/보정 음량은 따로 맞춘다. 프리셋은 32..192 kHz,
off는 8..192 kHz를 받는다. 입력 trim/보컬 파라미터는 실행 중 고정이다.

### 출력과 MR/AUX

두 번째 믹스는 `--send "<OUTPUT ID>" --send-channels 1,2`다. 기존 가상 케이블의 재생
endpoint로 보내고 Discord/OBS에서 대응 녹음 endpoint를 선택한다. 자동 연결/설치는 없다.
`output`/`send`는 지정한 믹스이지 헤드폰/Discord를 자동 인식하는 이름이 아니다.

동일 endpoint가 실제로 4채널 이상을 노출하면 `--output-channels 1,2`와
`--send "<같은 OUTPUT ID>" --send-channels 3,4`를 함께 사용할 수 있다. `check`는
`playback streams: 1`과 두 매핑을 표시한다. 공유 queue/다채널 리샘플러를 사용한다.
겹치는 채널은 거절하며 mono(`1`)·역순 stereo(`2,1`)를 지원하고 미지정 채널은 무음이다.
Windows가 stereo만 노출하면 ASIO 3/4를 만들어 주지 않는다.

```powershell
.\target\release\mixlive-audio.exe run --input "<MIC INPUT ID>" --output "<HEADPHONE OUTPUT ID>" --send "<CALL OUTPUT ID>" --aux-input "<MR CAPTURE ID>" --aux-channels 1,2 --aux-gain-db -12 --aux-to send --vocal natural --seconds 30
```

`--aux-input`은 **INPUT 목록의 캡처 endpoint**다. 분리된 루프백/라인 입력 또는 기존 가상
케이블의 녹음 쪽을 받으며 Chrome/PID/탭 직접 캡처가 아니다. 의도한 MR만 선택하고 MixLive 출력·
통화 수신음이 포함된 전체 루프백을 다시 넣지 않는다. 외부 피드백 자동 검출은 없다.
`--aux-to send|output|both`의 기본 send-only는 이미 듣는 MR의 이중 모니터를 피한다.
send/both에는 `--send`가 필요하다. AUX gain은 -96..0 dB, mono 복제·stereo·역순을 지원한다.
같은 INPUT ID의 mic 1/AUX 3,4는 한 스트림으로 추출하고 겹치는 채널은 거절한다.

별도 AUX 장치는 먼저 마이크 클럭으로 변환하므로 입력 queue/리샘플러 지연이 추가된다.
녹음 시각이나 직접 듣는 MR과의 시간/위상 정렬 보장은 없다. AUX 부족은 AUX만 무음,
장치/비유한 sample/overflow 오류는 전체 경로 중단이다. `stream0`/`stream1` 통계는
물리 출력 스트림 단위이며 `queue-ms`는 queue 목표이지 하드웨어 버퍼/실측 왕복 지연이 아니다.

## 장치·보컬·VST3를 Scene 하나로 재실행

**Scene schema 3**는 mic/AUX/출력/내장 보컬/queue 설정과 VST3의 순서·경로·class·state를
한 파일에 담는다. 기존 체인을 넣으면 상태 바이트를 복사하므로 원래 체인 JSON은 재실행에
필요하지 않다. 플러그인 DLL과 외부 샘플/라이선스 등은 여전히 설치되어 있어야 한다.

```powershell
.\target\release\mixlive-audio.exe scene save singing.mixlive-scene.json --input "<INPUT ID>" --output "<OUTPUT ID>" --vocal natural --vst3-chain voice.mixlive-chain.json
.\target\release\mixlive-audio.exe scene show singing.mixlive-scene.json
.\target\release\mixlive-audio.exe scene check singing.mixlive-scene.json
.\target\release\mixlive-audio.exe scene run singing.mixlive-scene.json --vst3-editor 2 --interactive --seconds 3600
```

`scene save`는 `--vst3` 목록을 직접 받을 수도 있다. 이때 플러그인을 열지 않으므로 상태는
아직 없고 실행 시 플러그인의 초기 설정을 사용한다. 옵션을 생략하면 `effects: []`인
의도적인 무플러그인 Scene이다. `save/show`는 장치·native DLL을 열지 않고 파일 계약만 검사한다.
`scene check`는 현재 장치를 확인하지만 플러그인 설치·호환성·음질 검증은 하지 않는다.
시작 시 저장된 chain을 한 번 해석해 직접 실행과 같은 native 처리 경로로 전달한다.

`scene run`은 **Scene 내부 체인만** 사용한다. `--vst3`/`--vst3-chain` 덮어쓰기 옵션은 거절한다.
슬롯별 `--vst3-editor`와 정상 종료의 `--vst3-chain-out <새 파일>`은 실행 시 지정할 수 있다.
Scene과 체인 파일은 자동으로 덮어쓰지 않는다. 실행 중 바꾼 경로·음량·mute까지 저장하는
실행 세션 스냅샷은 아직 없다. 편집한 VST3 상태는 새 체인 파일로 내보낸 뒤 다음 `scene save`에 넣는다.

상대 경로는 들어 있던 chain/scene 파일 기준으로 해석한다. CLI에서 지정한 경로는 현재 작업
폴더 기준으로 절대화한 뒤 Scene에 저장한다. 이전 schema 1/2는 읽지 않으며 현재 옵션으로
다시 저장한다. 마이그레이션/누락값 복구는 없다. Scene에는 개인 장치 ID와 플러그인 상태가
포함되므로 공개하지 않는다. `*.mixlive-scene.json`은 Git에서 제외한다.

## 실행 중 라우팅·음량

`--interactive`에서만 터미널 명령을 받는다. EOF도 종료 요청이다.

| 명령 | 동작 |
|---|---|
| `mute/unmute mic|aux|output|send|all` | 소스·출력·전체 음소거 |
| `gain master -18`, `gain send -6` | 공통 출력/두 번째 믹스 추가 감쇠 |
| `route mic output off`, `route mic output on` | 내 목소리의 첫 믹스 포함 여부 |
| `route aux send off` | 두 번째 믹스의 MR만 제외 |
| `level aux send -6`, `level mic output -9` | 지정 경로만 추가 감쇠 -96..0 dB |
| `status`, `stop` | 요청된 제어 상태 / 장치 해제 후 종료 |

일반형은 `route mic|aux output|send on|off`, `level mic|aux output|send <dB>`다.
꺼둔 경로의 level 변경은 켜짐이 아니고 전체 unmute도 개별 mute를 풀지 않는다.
source/route 제어는 합산 전이어서 출력 queue 지연이 남는다. 급한 송출 중단은 queue 뒤의
`mute send`/`mute all`을 사용한다. mute 중에도 queue를 소비해 오래된 소리를 재생하지 않는다.
이미 OS에 보낸 소리/Direct Monitor는 제어하지 않는다. gain/route 전환은 약 5 ms sample ramp이며
전체 응답시간 보장이 아니다. mic mute는 장치의 마이크 접근 자체를 종료하지 않는다.

## VST3 체인 만들기·편집·내보내기

직접 `run`에서는 `--vst3`를 처리 순서대로 반복하거나 `--vst3-chain`으로 복원한다.
각 슬롯은 **단일 active stereo 입출력 버스**의 effect다. 중간 좌우를 유지하고 같은 DLL도
별도 인스턴스/상태다. sidechain/multibus는 조용히 생략하지 않는다.

```powershell
.\target\release\mixlive-audio.exe run --input "<INPUT ID>" --output "<OUTPUT ID>" `
  --vst3 "C:\Plugins\EQ.vst3" --vst3 "C:\Plugins\Compressor.vst3" --vst3 "C:\Plugins\Reverb.vst3" `
  --vst3-editor 1 --vst3-editor 3 --vst3-chain-out voice.mixlive-chain.json `
  --interactive --seconds 3600
```

`--vst3-editor 1`은 1번 슬롯의 실제 라이브 원본 창이다. 창 하나를 닫아도 체인은 유지한다.
닫은 창 재열기는 아직 없다. 잘못된/중복 슬롯과 편집창 없는 플러그인은 시작 전에 거절한다.
`--vst3-chain-out` 정상 종료만 모든 슬롯 상태를 한 새 파일로 저장한다. 장치 중지→worker join→
editor 제거→마지막 편집 flush/processing stop→state 취득→native 해제→파일 게시 순서다.
어느 슬롯이든 실패하면 부분 파일을 게시하지 않는다. 오류·restart·암묵적 Drop에서는 저장하지 않는다.

체인 JSON은 아래처럼 작성할 수도 있다. `class_id`/`state`는 생략 가능하나 state가 있으면
32자리 hexadecimal class ID가 필요하다. 저장 snapshot에는 절대 경로와 확인된 class가 들어간다.

```json
{"schema": 1, "effects": [{"plugin": "plugins/EQ.vst3"}, {"plugin": "plugins/Reverb.vst3"}]}
```

이전 단일 플러그인 launch 옵션 `--vst3-state`, `--vst3-class`, `--vst3-state-out`은 제거했다.
별도 파일 도구의 단일 상태 형식은 그 도구에서만 사용하며 자동 변환하지 않는다.
**모든 plugin restart 요청은 경로 중단 사유**다. 일부 프리셋/oversampling/파라미터 변경도
중단될 수 있다. 실행 중 체인 삽입/삭제/순서 변경·dry 우회·지연 보상은 없다.

in-process native crash/hang은 격리되지 않는다. 신뢰하고 적법하게 설치한 플러그인만 사용한다.
GUI/control API와 worker의 mutex 공유 때문에 편집 중 무중단을 보장하지 않는다. 플러그인이
반환하지 않으면 종료도 기다릴 수 있다. 신고 지연 합계는 실제 왕복 지연 측정이 아니다.
실제 MiniFuse·상용 플러그인 GUI·청취·장시간 검증은 남아 있다.

## 별도 VST3 파일 도구

```powershell
.\target\release\mixlive-plugins.exe list
.\target\release\mixlive-plugins.exe inspect "C:\Plugins\Example.vst3"
.\target\release\mixlive-plugins.exe edit "C:\Plugins\Example.vst3" --state-out voice.mixlive-vst3.json
.\target\release\mixlive-plugins.exe render "C:\Plugins\Example.vst3" --input dry.wav --output wet.wav --state-in voice.mixlive-vst3.json
```

별도 `edit`에는 오디오 미리듣기가 없다. 검사·편집·WAV 처리는 자식 프로세스에서 수행하며
native crash 분리이지 권한 sandbox가 아니다. 상세는 [VST3 안내](crates/mixlive-vst3/README.md).
성공한 inspect는 라이브 호환성 승인이 아니다.

## 개발·검증

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo run -p mixlive-core --example discord_singing --locked
```

Windows CI의 기존 native fixture 검사도 따른다. 자동 검사는 실제 오디오 장치를 열지 않는다.
통과한 검사와 실제 장비·GUI·청취 수용을 구분한다. sample peak/GR은 LUFS/true peak가 아니다.

[DESIGN.md](DESIGN.md) · [STATUS.md](STATUS.md) · [AGENTS.md](AGENTS.md) · [NOTICE.md](NOTICE.md)

공개 저장소이며 제품 라이선스는 미정이다. 자동 녹음·클라우드 전송·드라이버 설치·공식 릴리스는 없다.
