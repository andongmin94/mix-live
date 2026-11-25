# MixLive · 믹스라이브

**내 소리를 만들고, 누구에게 들려줄지 정한다.**

Windows용 실시간 마이크·MR 라우팅/믹싱/보정 앱을 만드는 프로젝트다. DAW 타임라인이나
OBS 전용 플러그인이 아니다. 현재는 **GPUI Scene 실행 화면과 명령줄 도구를 제공하는 개발 빌드**다.
자체 가상 마이크·ASIO는 아직 없다. 물리 AUX와 최대 8개의 앱 프로세스 트리를 독립 소스로 직접 캡처할 수 있다. 실제 검증 결과와 미검증 범위는
[STATUS.md](STATUS.md)를 따른다.

## 현재 처리 경로

```text
마이크 → 선택적 음정 보정 → 입력 trim → 내장 EQ/다이내믹스 → stereo 리버브 → stereo 에코 → 순차 stereo VST3 체인 → 목적지별 보내기 ─┐
AUX/MR → 필요 시 입력 clock bridge → AUX trim → 목적지별 보내기 ─────────┤
앱 1..8 → 각각의 입력 clock bridge → 앱별 trim → 목적지별 보내기 ───────────┴→ 믹스
믹스 → 출력 clock bridge → master/믹스별 음량·mute → 믹스별 stereo sample limiter → 출력 장치
```

보컬은 한 번만 처리하고 AUX와 앱 소스에는 보컬 DSP/VST3를 걸지 않는다. 출력 믹스는 모니터·통화·방송의 최대 세 개다.
동일 endpoint의 분리 채널은 하나의 스트림/클럭을 공유하며 별도 장치는 bounded queue와
Rubato sinc resampler로 연결한다. 오류 시 임의 장치 선택이나 원음 마이크 우회는 하지 않는다.

장치 callback은 mic/AUX/각 앱 프레임을 함께 bounded queue로 보낸다. VST3가 없는 경우도 같은
worker/입력 queue 하나를 사용해 실행 중 첫 슬롯 추가와 마지막 슬롯 삭제를 처리한다.
로딩·editor·state·종료는 main/제어 스레드에서 수행한다.
호스트 내부 및 editor/control mutex는 남아 있으므로 **hard-real-time/무중단 보장은 없다.**
제품 요구사항은 [DESIGN.md](DESIGN.md)를 따른다.

## Windows에서 시작

Rust stable로 빌드하거나 해당 코드 커밋의 Actions artifact를 사용한다. 미서명 개발 바이너리이며
드라이버 설치나 시스템 기본 장치 변경을 하지 않는다.

```powershell
cargo build --release -p mixlive-app -p mixlive-audio-windows -p mixlive-vst3 --bins --locked
.\target\release\mixlive.exe
```

### 네이티브 Scene 화면

`mixlive.exe`에서 **새 Scene → 장치·보컬·VST3 선택 → 새 파일로 저장 → 오디오 시작** 순서로 시작할 수 있다.
마이크와 주 출력은 직접 선택하며, AUX/MR 녹음 장치, 최대 8개의 실행 중인 앱, 통화(추가 송출)·방송 송출 재생 장치는 선택 사항이다. 앱 소스는 물리 AUX와 함께 사용할 수 있다.
목록은 실제 WASAPI endpoint 이름·ID·기본 채널/샘플레이트/형식을 표시한다. 같은 이름이어도
ID로 구분하며, 사용할 수 없는 설정은 이유를 표시한다. 장치를 연결한 뒤에는 새 Scene이나
**장치·출력 설정**을 다시 열어 목록을 조회한다. 조회는 스트림을 만들지 않고 시스템 기본 장치를 바꾸지 않는다.

채널 입력은 마이크 한 개, 나머지는 mono 한 개 또는 좌/우 두 개(`1` 또는 `1,2`)다.
선택한 장치가 mono이면 `1`, 그 외 출력/AUX에는 `1,2`를 제안한다. 실제 endpoint 범위를 넘거나
같은 endpoint의 mic/AUX 또는 모니터·통화·방송 채널이 겹치면 저장 전에 거절한다.
AUX를 통화 또는 방송으로 보내려면 해당 출력 장치도 선택해야 한다.

내장 보정 `off/natural/warm/bright`를 고를 수 있으며, 새 Scene의 초기값은 출력 −18 dB,
AUX −12 dB, 입력 trim 0 dB, 보정 비율 100%, queue 목표 30 ms다. 전체/모니터/통화/방송/AUX 시작 음량과
queue 목표, 조성·스케일·음정 보정 강도/속도, 입력 trim, 보정 비율, EQ 및 컴프레서/디에서 세부값, 리버브 비율/잔향/pre-delay, 에코 비율/시간/피드백과 VST3 순서를 저장 전에 바꿀 수 있다.
저장은 기존 schema 13의 **새 파일만** 만들며 기존 파일을 덮어쓰지 않는다. 취소/저장 실패 시
이전 Scene 선택을 유지한다. 장치와 현재 설정은 실제 시작 때 다시 검사한다.

기존 체인은 **Scene 열기 → 저장된 소스·보컬 체인·목적지 확인 → 오디오 시작**으로 실행한다.
파일 경로를 `mixlive.exe singing.mixlive-scene.json`으로 전달해도 읽기만 하며 자동으로 시작하지 않는다.
시작할 때 기존 장치/채널 검증과 VST3 로딩을 수행한다. Scene의 실제 DLL과 장치가 필요하며,
누락되면 오류를 표시하고 시작하지 않는다. 다른 장치나 원음 경로로 바꾸지 않는다.

정지한 Scene의 **보컬 설정** 버튼에서도 같은 설정 화면을 연다. 기존 장치·채널·AUX 목적지·
출력 음량·queue 설정을 그대로 유지하면서 보컬 설정과 VST3 체인을 편집해 새 Scene으로 저장한다.
편집 취소는 현재 Scene과 파일을 바꾸지 않는다. 저장한 설정은 다음 명시적 시작부터 적용된다.

장치나 목적지를 바꾸려면 정지한 Scene의 **장치·출력 설정**을 연다. 현재 WASAPI 목록을 조회하고
저장된 mic/AUX/모니터/통화/방송 ID·채널·AUX 목적지·시작 음량·queue 값을 불러온다.
보컬과 VST3 체인도 유지하며 같은 화면에서 편집할 수 있다. 사용하지 않는 AUX/통화/방송의
저장 채널과 음량도 폼을 열었다는 이유로 초기화하지 않는다.

저장된 장치가 연결되지 않았거나 지원되지 않으면 해당 ID와 채널을 유지하고 이유를 표시한다.
다른 장치나 “사용 안 함”으로 자동 전환하지 않는다. 사용 가능한 장치를 직접 선택해야 하며,
optional AUX/통화/방송은 “사용 안 함”으로 명시적으로 제거할 수 있다. 장치를 바꿀 때만 현재
채널 수에 맞는 mono/stereo 시작값을 제안하고, 저장 전 채널 범위·겹침·AUX 목적지를 검사한다.
누락/지원 불가 장치가 남으면 저장을 거절한다. 저장·취소는 보컬 편집과 같은 새 파일 계약을 따른다.

전체 시작 음량은 모든 출력에 공통 적용되고 모니터/통화/방송 시작 음량은 각 믹스의 추가 감쇠다.
AUX 입력 음량은 합산 전 trim이다. 음량 값은 −96~0 dB, 장치별 queue 목표는 10~100 ms이며,
queue 목표는 하드웨어 버퍼나 실측 왕복 지연이 아니다. 이 화면은 **파일의 시작 설정**을 편집하며
저장된 mixer mute/경로별 gain·mic 연결값을 유지한다. AUX 목적지는 모든 경로 끄기도 선택할 수 있다. 장치 목록은 시작 때 다시 검사한다.

VST3는 **설치된 VST3 검색**에서 표준 설치 경로의 후보를 골라 추가하거나,
**파일 추가** 또는 Windows bundle용 **폴더 추가**로 직접 선택한다. 검색은 기존 CLI의 경로 조회만
백그라운드에서 실행하며 DLL을 열지 않고, 후보를 자동으로 체인에 추가하지 않는다. 이름이 같은
플러그인은 전체 경로로 구분한다. 실제 class·stereo 호환성은 시작 때 확인한다. 슬롯별 위로/아래로/삭제로
순서를 정한다. **체인 파일 추가**는 기존 체인 JSON을 현재 목록 뒤에 붙이며 class와 저장 상태도
가져온다. 같은 플러그인을 여러 번 추가할 수 있고, 순서를 바꿔도 각 슬롯의 class/state가 함께
이동한다. 기존 상대 경로는 가져온 파일 기준으로 해석한다. 슬롯에는 경로·class·저장 상태 크기를
표시하며 파일 선택/편집/저장 과정에서는 DLL을 열지 않는다. 새로 추가한 파일은 저장 상태 없이
시작하고, native 호환성·설치 여부는 오디오 시작 때 확인한다.

실행 중에는 다음을 화면에서 조절한다.

- 고정 상단의 전체 출력 음소거, 소스별 mic/AUX/앱 음소거, 출력별 음소거.
- 모든 출력에 공통으로 적용되는 master와 모니터/통화/방송의 독립 감쇠.
- 각 소스를 어느 출력에 보낼지 선택하는 스위치와 경로별 음량.
- AUX/MR·앱별 공통 입력 trim(-96..0 dB). 입력 peak는 trim 전이며, 소스 trim은
  약 5 ms sample ramp로 적용하고 모든 연결된 목적지에 동일하게 반영한다. 장치·VST3는 다시 열지 않는다.
- 라이브 보컬 trim·보정 비율·컴프레서/디에서 threshold·ratio·attack/release·최대 감쇠·리버브 비율·잔향 시간·에코 비율·피드백 슬라이더.
  **실시간 EQ**에서 HPF(40..200 Hz)와 180/2800/7500 Hz 밴드의 프리셋 대비 gain(±6 dB)도 조절한다.
  EQ/다이내믹스 프리셋이 off이면 trim과 리버브·에코만 표시한다.
- **실행 중 보컬 설정**에서 프리셋·리버브 pre-delay·에코 시간을 함께 적용한다.
  현재 슬라이더 값은 유지하며 프리셋을 켜거나 끄면 사용할 수 있는 EQ/다이내믹스 조절 항목도 갱신한다.
- 실제 마이크 입력·내장 보정 후·전체 보컬 체인 후·AUX 입력·앱별 입력과 모니터/통화/방송 sample peak.
  약 100ms 간격의 최대값을 dBFS로 표시한다. 소스 peak는 mute 전, AUX와 앱은 입력 trim 전이며
  출력 peak는 감쇠·mute·stereo sample limiter 후 장치 채널 매핑 전이다. 공유 endpoint의 각 믹스도 독립 계측한다.
  리미터 전 peak와 해당 구간 최대 gain reduction(dB)을 각 출력에 함께 표시한다.
  내장 보정 gain reduction과 초당 갱신되는 진단 수치는 같은 계측값을 사용한다. 내장 보정 후 peak는 리버브·에코를 포함한 stereo 최대값이다. 가짜 level 애니메이션은 없다.

**정지** 또는 창 닫기는 기존 장치 중지 → worker 종료 → native plugin 해제 경로를 사용한다.
장치 시작·종료와 native plugin 수명은 GPUI 메인 스레드에 유지하고, 네이티브 호출 중에는
GPUI 객체의 변경 참조를 유지하지 않는다. 플러그인 자체의 crash/hang은 여전히 격리되지 않는다.
명시적으로 정지하거나 오류가 나기 전까지 실행하며 CLI의 실행 시간 제한은 적용하지 않는다.

실행 중 **정지 후 새 Scene 저장**을 선택하면 파일명 선택을 취소할 때는 오디오를 계속 실행하고,
확정하면 장치 중지 → worker join → 모든 VST3 슬롯 상태 수집 → 새 Scene 저장을 수행한다.
저장된 전체/믹스별 음량, AUX/앱 입력 trim, 소스·출력·전체 mute, 모든 경로의 연결·gain, 라이브 보컬 파라미터와 VST3 우회·상태는 다음 명시적 시작부터
적용된다. 초기 mute는 스트림 시작 전에 복원한다. 오류·상태 수집 실패·기존 파일 충돌 시 이전 Scene 선택을
유지하고 새 파일을 게시하지 않는다. 일반 정지/창 닫기는 저장하지 않는다.
장치·출력 편집은 정지 상태에서만 제공한다.
실행 중에는 **설치된 VST3 검색**, **VST3 파일 추가**, **VST3 폴더 추가**로 슬롯을 추가하고,
각 슬롯의 **위로/아래로/삭제**로 현재 체인을 편집한다. 처음 비어 있던 체인과 전체 삭제도 지원한다.
유지한 슬롯의 실제 인스턴스·파라미터·원본 창·우회 이력은 그대로 옮긴다. 공통 파라미터 패널은
체인 변경 때 닫히며 새 순서에서 다시 열 수 있다. 일반 정지는 저장된 시작 설정으로 돌아가고,
**정지 후 새 Scene 저장**은 변경한 순서와 남은 슬롯의 최종 상태를 저장한다.
새 플러그인과 버퍼는 제어 스레드에서 준비하고, 처리 블록 경계에서 체인과 송출 지연 보상을 함께
바꾼다. 제거한 인스턴스와 이전 버퍼도 제어 스레드에서 해제한다. 추가 준비 실패 시 기존 체인을 유지한다.
총 지연이 같은 변경은 보상 이력을 유지한다. 지연이 달라지면 새 보상 이력은 무음에서 시작하며,
효과 추가·삭제 때의 소리 변화나 꼬리 단절을 없애는 교차 전환은 제공하지 않는다.
실행 중 보컬 체인의 각 VST3 슬롯에서 **우회 켜기/끄기** 및 **원본 창 열기/닫기**를 선택할 수 있다.
우회는 해당 슬롯의 신고 지연만큼 정렬한 dry 신호로 약 5 ms 교차 전환한다. 다른 슬롯과 PDC는
유지하며 모든 native 효과는 계속 처리한다(CPU 절감이나 음량 일치 A/B가 아니다).
음정 보정을 포함한 총 지연이 2초를 넘는 체인은 시작 또는 추가를 거절한다. 정지한 보컬 설정에서도 우회 시작값을 변경할 수 있다. 기존 라이브
인스턴스의 네이티브 편집창이며 별도 검사 인스턴스가 아니다. 닫았다 다시 열어도 파라미터와
체인 순서를 유지한다. 원본 창의 닫기 버튼도 다음 상태 확인에서 반영한다. 편집창이 없는
플러그인은 오류를 표시하며 해당 요청만 거절한다. GPUI가 메시지 루프를 소유하고 native
창 조작은 UI 변경 참조 밖의 foreground 스레드에서 실행한다. 일반 정지/저장 후 정지는 모든
원본 창을 닫으며, 저장을 선택한 경우 마지막 상태도 보존한다.
원본 창 유무와 관계없이 슬롯의 **파라미터 편집** 버튼으로 같은 실행 인스턴스의 값을 조절할 수 있다.
목록에서 파라미터를 고르고 선택 위치를 움직인 뒤 **적용**을 누른다. 슬라이더를 움직이는 동안에는
미적용 값만 바뀌며, 적용할 때 선택한 값 하나를 전달한다. 표시 문자열과 단위는 플러그인에서 읽고,
선택 위치의 비율을 임의의 Hz/dB 값으로 환산하지 않는다. 이산 파라미터는 플러그인의 단계 수에 맞춘다.
읽기 전용 값은 표시만 하고 숨김 파라미터는 목록에서 제외한다. **새로고침**은 원본 창/processor의
피드백을 포함한 값을 다시 조회하며, 다른 파라미터 선택과 함께 미적용 값을 버린다. 패널 닫기는 오디오를
정지하지 않는다. 변경값은 기존 **정지 후 새 Scene 저장** 경로로 보존하며 Scene 형식은 13 그대로다.
조회·포맷·편집은 foreground에서 수행한다. 기존 native host의 공유 mutex와 파라미터 큐 제약은 남으므로
연속 자동화·hard-real-time 전달 보장을 추가한 기능은 아니다.
표시된 endpoint 채널은 WASAPI 채널이고, 신고된 queue/plugin 지연은 실측 왕복 지연이 아니다.

GPUI Deck은 [고정 revision](https://github.com/andongmin94/gpui-deck/tree/78c0898ccb6feba03e3a3243ac7d4729fd80aee6)의
Button/Switch/Slider/SliderThumb/Select/Input/ScrollArea/Root를 재사용하고 GPUI engine family 0.3.12를 사용한다.

### 장치 확인과 Scene 생성용 CLI

```powershell
.\target\release\mixlive-audio.exe devices
.\target\release\mixlive-audio.exe check --input "<INPUT ID>" --output "<OUTPUT ID>" --vocal natural
.\target\release\mixlive-audio.exe run --input "<INPUT ID>" --output "<OUTPUT ID>" --vocal natural --output-gain-db -18 --seconds 30
```

`devices`는 목록, `check`는 장치·채널·보컬 설정만 검사한다. 실제 시작은 `run`이다.
채널은 선택한 WASAPI endpoint 내부 **1-based** 번호이며 MiniFuse ASIO/루프백/물리 단자와
같다고 가정하지 않는다. 기본 실행은 30초, `--seconds`는 1..3600이다. 자동 녹음은 없다.
**헤드폰 하드웨어 음량을 낮추고** DAW·Windows·인터페이스의 중복 모니터링을 끈다.
항상 적용되는 믹스별 스테레오 연동 sample limiter는 두 채널에 같은 감쇠를 적용하여
0.98 sample ceiling을 지킨다. 추가 lookahead 없이 즉시 감쇠하며 최대 80 ms의 선형 회복으로
unity에 복귀한다. 채널별 개별 클리핑 대신 gain을 낮추지만 완전한 무왜곡이나 true-peak,
음향 피드백 방지·청력 보호 기능은 아니다. 입력 ADC clipping은 복원하지 않는다.

### 내장 보컬

`--vocal off|natural|warm|bright` (기본 off). HPF/EQ→RMS soft-knee compression→split-band
디에서→원음/보정 blend→stereo 리버브→stereo 에코다. 선택적 실험 음정 보정은 이 체인 앞에 적용하며 기본 꺼짐이다. 프리셋 이름은
시작 설정이며 실제 청취 수용을 마친 고급 사운드를 의미하지 않는다.

| 옵션 | 적용 위치/범위 |
|---|---|
| `--pitch-correction 0` | 음정 편차를 목표 음으로 이동시키는 강도 0..1. 0은 지연 없이 끄기 |
| `--pitch-retune-ms 20` | 음정 보정 목표의 smoothing time 5..200 ms |
| `--input-gain-db 0` | 음정 보정 이후, EQ/다이내믹스 검출기 앞 trim -36..18 dB. ADC clipping 복원 불가 |
| `--vocal-strength 1` | 원음/보정 blend 0..1. 음량 일치 기능 아님 |
| `--vocal-threshold-db -24` | 컴프레서 threshold -60..0 dBFS |
| `--deess-threshold-db -30` | 디에서 고역 threshold -60..0 dBFS |
| `--eq-hpf-hz 75` | 하이패스 40..200 Hz |
| `--eq-body-db 0`, `--eq-presence-db 0`, `--eq-air-db 0` | 180 Hz shelf / 2.8 kHz peak / 7.5 kHz shelf의 프리셋 대비 ±6 dB |
| `--compressor-ratio 2.5` | 컴프레서 ratio 1..8 |
| `--compressor-attack-ms 8`, `--compressor-release-ms 140` | attack 1..50 ms / release 20..500 ms |
| `--deess-max-db 4` | 디에서 최대 감쇠 0..12 dB |
| `--reverb-mix 0` | stereo 리버브 wet 비율 0..1 (0은 끄기) |
| `--reverb-decay-secs 1.2` | 저역 기준 명목 잔향 시간 0.2..5초 |
| `--reverb-predelay-ms 15` | wet 신호에만 추가 pre-delay 0..80 ms |
| `--echo-mix 0` | 에코 wet 비율 0..1 |
| `--echo-time-ms 250` | 에코 반복 간격 10..1000 ms |
| `--echo-feedback 0.35` | 피드백 0..0.85, 0은 한 번 반복 |
| `--output-gain-db -18` | 두 믹스의 초기 최종 감쇠 -96..0 dB |
| `--monitor-gain-db 0` | 모니터 초기 추가 감쇠 -96..0 dB |
| `--send-gain-db 0` | 통화 초기 추가 감쇠 -96..0 dB |
| `--broadcast-gain-db 0` | 방송 초기 추가 감쇠 -96..0 dB |

자동 makeup gain은 없다. A/B 청취의 원본/보정 음량은 따로 맞춘다. 프리셋은 32..192 kHz,
off는 8..192 kHz를 받는다. 열여섯 보컬 파라미터는 실행 중에도 조절할 수 있다.
각 대상값은 블록 경계에서 읽고 처리 clock의 약 10 ms
sample ramp를 적용한다. trim은 선형 gain, 보정/리버브 비율은 blend, threshold는 dB,
ratio와 디에서 최대 감쇠는 요청값, attack/release는 envelope coefficient를 전환한다.
decay는 각 FDN feedback gain을 전환하며, 에코의 blend와 feedback도 독립적으로 전환한다. 콜백 안에서 버퍼를 만들거나 VST3를 다시 로드하지 않는다.
EQ는 기존 biquad의 Direct Form I 필터를 사용한다. 바뀐 밴드의 목표 계수만 계산하고 정규화된 계수를
10 ms 동안 전환하며 입력·출력 이력을 유지한다. 추가 delay buffer는 없고, 동일 목표값을 다시 읽어도
진행 중인 전환을 재시작하지 않는다. 저장 시에는 전환 중간값이 아닌 요청된 HPF/EQ 값을 보존한다.
보정 비율 0에서도 선택한 EQ/다이내믹스 상태를 유지하며, 리버브 0에서도 room 이력을 갱신한다.
따라서 처음 꺼 둔 리버브를 실행 중 켤 수 있고 다시 켤 때 정지된 과거 이력을 재생하지 않는다.
이 구조는 off 상태에서도 내부 DSP 비용이 들며, 실제 장비 CPU/청취 지연은 미측정이다.

프리셋은 저역·존재감·고역 EQ의 시작 형태를 선택한다. 하이패스는 명시적 절대 주파수이고
나머지 EQ gain은 프리셋에 더하는 오프셋이다. 다이내믹스 시작값은 모든 프리셋에서
2.5:1, 8/140 ms, 최대 디에서 4 dB로 통일하며 사용자가 직접 변경한다. Ratio 1은 새 압축을
발생시키지 않고 기존 envelope는 release로 복귀한다. 디에서 최대값은 현재 ramp 값을 넘지 않는다.
Off 프리셋에서는 EQ/다이내믹스가 적용되지 않는다. 새 설정값의 음질은 청취 검증이 필요하다.

**실시간 보컬 보정** 카드의 **실행 중 보컬 설정**에서 프리셋, 리버브 pre-delay(0..80 ms),
에코 시간(10..1000 ms)을 편집하고 **보컬 설정 적용**을 누른다. 세 값만 함께 적용하므로
패널을 연 뒤 조절한 trim·EQ·다이내믹스·효과 비율과 mixer 값은 덮어쓰지 않는다.
**현재 설정으로 되돌리기**는 미적용 입력을 버리고, **보컬 설정 닫기**는 패널만 닫는다.
유효하지 않은 값이나 현재 입력 sample rate에서 지원하지 않는 프리셋은 기존 설정을 유지한 채 거절한다.

natural/warm/bright 사이의 변경은 같은 EQ·다이내믹스 상태를 유지하면서 EQ 계수를 10 ms에
걸쳐 전환한다. off↔활성 전환은 필요한 EQ·다이내믹스 처리부를 제어 스레드에서 준비하고
기존 worker의 블록 경계에서 교체하며, 이전 처리부도 제어 스레드에서 해제한다.
이때 음정 보정·VST3 인스턴스·리버브·에코와 송출 PDC는 유지한다. off 전환의 클릭이나
음량 차이를 제거하는 교차 전환은 제공하지 않는다. 일반 정지는 변경을 버리고
**정지 후 새 Scene 저장**은 현재 프리셋과 시간, 기존 live target을 Scene 13에 함께 저장한다.

스테레오 리버브는 **보컬 설정**의 별도 ROOM 카드에서 조절한다. 기본값은 꺼짐이며,
예를 들어 `--reverb-mix 0.15 --reverb-decay-secs 1.2 --reverb-predelay-ms 15`로
짧은 공간감을 추가할 수 있다. 수치는 청취로 조정하는 시작점이다. EQ/다이내믹스 preset이
off여도 리버브 비율이 0보다 크면 리버브가 동작한다. 보정 비율과 별도로 linear wet/dry blend를
적용하고, dry 신호는 추가 지연하지 않는다. 8-line orthogonal FDN과 기존 biquad 고역 감쇠를
사용하며, 서로 다른 stereo wet 신호를 VST3에 전달한다. 잔향은 예술적 시간 응답으로 PDC의
고정 신고 지연에 합산하지 않는다. AUX/앱은 공간계를 거치지 않는다. 소스 mute는 잔향을
포함한 보컬을 차단하며, 내부 잔향 이력은 계속 갱신된다. 설정 편집 화면의 값은 다음 시작부터
적용하고 라이브 슬라이더는 실행 중 적용한다. 종료 시 잔향 샘플을 녹음/저장하지 않는다. 실제 청취·음질 수용은 미검증이다.

스테레오 에코는 별도 ECHO 카드에서 설정한다. 리버브 후·VST3 전의 stereo feedback delay이며,
기본 꺼짐이다. 예: `--echo-mix 0.2 --echo-time-ms 250 --echo-feedback 0.35`.
좌우를 독립 처리하며 dry에는 추가 지연을 넣지 않는다(100% wet은 원음 대신 반복음만 출력).
시작할 때 에코 최대 1초와 리버브 pre-delay 최대 80 ms의 이력을 확보한다. 실행 중 시간 변경은
기존 이력을 유지하고 읽기 위치를 10 ms 동안 이동하며 인접 샘플을 선형 보간한다.
리버브의 FDN 잔향과 에코 feedback 이력, 진행 중인 mix/decay/feedback ramp는 초기화하지 않는다.
시간을 바꾸는 동안 효과음의 높이나 질감이 달라질 수 있다. mix 0에서도 이력을 갱신하며,
AUX/앱에는 적용하지 않는다. 두 시간은 wet-only 효과로서 원음이나 PDC에 지연을 추가하지 않는다.
실제 노래 음질·장비 CPU 수용은 아직 미검증이다.

### 마이크 음정 표시 및 실험적 자동 보정

마이크 원신호(trim/EQ/VST3 전)의 40–80 ms 연속 window를 기존 SPSC 큐로 전달하고,
control thread에서 RustFFT 6.4.1의 자기상관과 정확한 중첩구간 에너지 정규화로 추정한다.
70–1200 Hz 범위의 단일 음높이·가까운 평균율 음이름(A4=440 Hz)·cent 편차·검출 명료도를 표시한다.
`--pitch-key 9 --pitch-scale minor`처럼 기준 조성(C=0, …, B=11)과 chromatic/major/minor를
선택하여 가장 가까운 허용음과 해당 음에 대한 편차를 GUI·CLI로 표시한다. Scene에 두 설정을 저장한다.
-50 dBFS 평균제곱 threshold와 0.80 명료도 기준을 사용한다. 불명확하거나 새 window가 없으면
음정을 표시하지 않는다. 느린 표시기는 관측 window만 잃으며 오디오를 멈추지 않는다.
표시용 분석 자체는 오디오를 변경하지 않고 지연도 넣지 않는다. 별도로 `--pitch-correction 0.75 --pitch-retune-ms 20`을
설정하면 마이크에 실험적 mono 자동 음정 보정이 활성화된다. 기존 RustFFT와 같은 음높이 검출식을 재사용하며
sqrt-Hann 4-overlap phase vocoder로 샘플 수를 유지하면서 주파수를 이동한다. 목표 보정 강도와
retune smoothing은 시작 설정 또는 실행 중 음정 설정 패널에서 선택하고 조성과 함께 저장한다. 새 유성 구간은 처음 검출한 목표로
시작하고, 이어지는 유성 프레임 사이의 목표 변화에 retune을 적용한다. 음정이 검출되지 않는 frame은
동일 지연의 dry 신호를 사용하고 앞 frame의 보정 차분은 overlap-add로 사라진다. 검출창 크기만큼
고정 지연이 생긴다(48 kHz에서 2048 samples, 약 42.67 ms). 강도 0은 처리기 자체를 준비하지 않으므로
추가 지연이 없다. 기존 통화·방송 보상에 이 지연을 합산하고 모니터에는 추가 AUX/app 보상을 하지 않는다.
보컬 검출기와 FFT의 작업 버퍼를 시작 또는 실행 중 켜기 요청의 제어 스레드에서 준비한다. 실제 노래 음질·formant 보존·비브라토·CPU 수용을
주장하지 않는다. VST3 없이도 동작하는 실험 기능이며 자동 녹음이나 파일 저장은 없다.

실행 중 소스별 보내기 카드의 **실행 중 음정 설정**을 열어 조성·스케일·강도·속도를 입력하고
**음정 설정 적용**을 누른다. 네 값을 한 번에 적용하며 입력 중에는 실행 설정을 바꾸지 않는다.
**현재 설정으로 되돌리기**는 미적용 입력을 버리고, **음정 설정 닫기**는 패널만 닫는다.
유효하지 않은 값이나 VST3와 합산한 2초 지연 상한 초과는 기존 설정을 유지한 채 거절한다.

강도 양수 사이의 변경은 같은 분석·위상·overlap 이력을 유지하고 기존 retune smoothing으로
새 목표를 따른다. 0↔양수 전환은 보정기와 통화·방송의 AUX/앱 지연 보상을 같은 처리 경계에서
교체한다. VST3 인스턴스와 내장 EQ·잔향 상태는 유지한다. 켤 때 분석창을 채우는 초기 무음과
끌 때 지연 변화가 있으므로 무중단 교차 전환을 보장하지 않는다.
현재 지연 설명과 원신호 음정의 스케일 목표는 승인된 설정을 사용하며, 이전 목표 계측은 다음
측정까지 비운다. 이후 VST3 추가·삭제도 현재 음정 보정 지연을 포함해 보상한다.
일반 정지는 변경을 버리고 **정지 후 새 Scene 저장**은 현재 음정 설정을 기존 Scene 13에 저장한다.

### 출력과 MR/AUX

모니터는 `--output "<OUTPUT ID>"`, 통화는 `--send "<OUTPUT ID>"`, 방송은
`--broadcast "<OUTPUT ID>"`로 선택한다. 통화/방송은 각각 선택 사항이므로 통화 없이
방송만 추가해도 된다. 기존에 설치된 가상 케이블의 재생 endpoint로 연결하고 Discord/OBS에서
대응 녹음 endpoint를 직접 선택한다. MixLive가 자체 가상 장치를 설치하거나 자동 연결하지 않는다.

같은 endpoint가 실제로 6채널 이상을 노출하면 모니터 1,2 / 통화 3,4 / 방송 5,6을
한 스트림의 서로 다른 채널로 사용할 수 있다. `--output-channels`, `--send-channels`,
`--broadcast-channels`를 지정하며 겹치는 채널을 거절한다. 하나의 shared clock/resampler를
유지하고 각 믹스의 gain/mute/peak는 독립적이다. mono·역순 stereo·미지정 채널 무음을 지원한다.
장치가 stereo만 노출하면 임의 ASIO 채널을 만들어 주지 않는다.

```powershell
.\target\release\mixlive-audio.exe run --input "<MIC INPUT ID>" --output "<HEADPHONE OUTPUT ID>" --send "<CALL OUTPUT ID>" --aux-input "<MR CAPTURE ID>" --aux-channels 1,2 --aux-gain-db -12 --aux-to send --vocal natural --seconds 30
```

`--aux-input`은 **INPUT 목록의 캡처 endpoint**다. 분리된 루프백/라인 입력 또는 기존 가상
케이블의 녹음 쪽을 받는다. 앱 직접 캡처는 별도의 `--app` 옵션을 사용한다. 의도한 MR만 선택하고 MixLive 출력·
통화 수신음이 포함된 전체 루프백을 다시 넣지 않는다. 외부 피드백 자동 검출은 없다.
`--aux-to off|send|output|both|broadcast|output-broadcast|send-broadcast|all`의 기본 send-only는 이미 듣는 MR의 이중 모니터를 피한다.
`both`는 모니터+통화, `all`은 세 출력이다. 통화/방송을 포함하려면 각각의 장치가 필요하다. AUX gain은 -96..0 dB, mono 복제·stereo·역순을 지원한다.
같은 INPUT ID의 mic 1/AUX 3,4는 한 스트림으로 추출하고 겹치는 채널은 거절한다.

별도 AUX 장치는 먼저 마이크 클럭으로 변환하므로 입력 queue/리샘플러 지연이 추가된다.
녹음 시각이나 직접 듣는 MR과의 시간/위상 정렬 보장은 없다. AUX 부족은 AUX만 무음,
장치/비유한 sample/overflow 오류는 전체 경로 중단이다. `stream0`/`stream1` 통계는
물리 출력 스트림 단위이며 `queue-ms`는 queue 목표이지 하드웨어 버퍼/실측 왕복 지연이 아니다.

### 다중 앱 직접 캡처 (Windows 11)

실행 중인 앱을 장치·출력 설정의 **앱 소스** 목록에서 하나씩 추가한다. 최대 8개이며
물리 AUX와 동시에 사용할 수 있다. 각 앱의 채널(1, 2 또는 stereo 순서), 입력 trim,
시작 mute, 모니터·통화·방송 연결 및 경로별 gain을 독립적으로 지정한다.
새 앱의 모든 경로는 꺼져 있다. 통화 수신음을 통화 송출로 되돌리지 않도록 직접 연결한다.
앱은 기존 장치로 계속 재생하므로 주 출력에 다시 보내면 이중 모니터링될 수 있다.

```powershell
.\target\release\mixlive-audio.exe apps
.\target\release\mixlive-audio.exe run --input "<MIC INPUT ID>" --output "<HEADPHONE OUTPUT ID>" --send "<CALL OUTPUT ID>" --app "C:\Program Files\Browser\browser.exe" --app "C:\Program Files\Call\call.exe" --interactive --seconds 3600
```

`--app`은 정확한 실행 파일 경로를 반복 지정한다. 이 예에서는 `route app1 send on`으로
첫 앱만 송출에 연결할 수 있다. `route app2 send off`로 두 번째 앱의 송출 제외를 확인한다.
번호는 현재 Scene의 목록 순서이며, 영구 ID는 실행 파일 경로다. Scene에는 PID를 저장하지 않는다.
동일 경로의 여러 독립 루트, 중복 경로, 서로 포함 관계인 프로세스 트리, MixLive를 포함한
트리는 거절한다. Windows Process Loopback의 include-tree 모드만 사용하며 전체 데스크톱
캡처나 다른 PID로 자동 전환하지 않는다.

각 앱은 별도 MTA 스레드, float32 stereo 48 kHz와 독립 bounded clock bridge를 사용한다.
모든 앱 클라이언트를 준비한 뒤 시작하고, 실제 믹싱 및 VST3 worker의 source 순서는 하나의
세션으로 유지된다. 앱별 실시간 peak와 mute, 목적지별 연결·gain을 표시하며 정지 후 Scene 저장에
함께 기록한다. 탭 선택, 자체 private render endpoint, 자동 재연결은 없다.
실제 앱의 동시 오디오 수신과 지연·장시간 안정성은 실장비에서 확인해야 한다.

## 장치·보컬·VST3를 Scene 하나로 재실행

**Scene schema 13**은 mic/AUX/앱 소스별/출력/내장 보컬/queue 설정과 VST3의 순서·경로·class·state·bypassed를
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
Scene과 체인 파일은 자동으로 덮어쓰지 않는다. 직접 `run` 또는 `scene run`에서
`--scene-out <새 파일>`을 지정하면 정상 정지(시간 제한, stop, EOF) 후 GUI와 같은
전체 Scene snapshot을 저장한다. 믹서·라이브 보컬 설정·모든 VST3 상태를 schema 13에 담으며,
장치나 플러그인이 오류로 중단되면 파일을 게시하지 않는다. 기존 파일 또는 존재하지 않는
상위 폴더는 시작 전에 거절하고, 종료 시점의 파일 충돌도 덮어쓰지 않는다.
전체 Scene과 체인만 내보내기를 동시에 요청할 수 없다. 옵션이 없으면 실행 중 변경을 저장하지 않는다.

```powershell
.\target\release\mixlive-audio.exe scene run singing.mixlive-scene.json --interactive --scene-out adjusted.mixlive-scene.json
```

상대 경로는 들어 있던 chain/scene 파일 기준으로 해석한다. CLI에서 지정한 경로는 현재 작업
폴더 기준으로 절대화한 뒤 Scene에 저장한다. 이전 schema 1~12는 읽지 않으며 현재 옵션으로
다시 저장한다. 마이그레이션/누락값 복구는 없다. Scene에는 개인 장치 ID와 플러그인 상태가
포함되므로 공개하지 않는다. `*.mixlive-scene.json`은 Git에서 제외한다.

## 실행 중 라우팅·음량

`--interactive`에서만 터미널 명령을 받는다. EOF도 종료 요청이다.

| 명령 | 동작 |
|---|---|
| `mute/unmute mic|aux|appN|output|send|broadcast|all` | 소스·출력·전체 음소거 |
| `gain master -18`, `gain monitor -3`, `gain send -6`, `gain broadcast -9` | 공통/각 믹스 추가 감쇠 |
| `route mic output off`, `route mic output on` | 내 목소리의 첫 믹스 포함 여부 |
| `route aux send off` | 두 번째 믹스의 MR만 제외 |
| `level aux send -6`, `level mic output -9` | 지정 경로만 추가 감쇠 -96..0 dB |
| `trim aux -12`, `trim app1 -6` | AUX/앱 전체 입력 trim -96..0 dB |
| `bypass 1 on`, `bypass 1 off` | 1번 VST3 슬롯의 지연 정렬 우회/효과 적용 |
| `vocal trim -3`, `vocal strength 0.75` | 입력 trim(dB) / EQ·다이내믹스 blend(0..1) |
| `vocal eq-hpf 95` | 하이패스 주파수(40..200 Hz) |
| `vocal eq-body -2`, `vocal eq-presence 1.5`, `vocal eq-air 3` | 180/2800/7500 Hz EQ의 프리셋 대비 gain(±6 dB) |
| `vocal compressor -28`, `vocal deesser -35` | threshold(-60..0 dBFS) |
| `vocal ratio 3.5`, `vocal attack 5`, `vocal release 220`, `vocal deess-max 7` | ratio / attack·release(ms) / 디에서 최대 감쇠(dB) |
| `vocal reverb 0.15`, `vocal decay 1.5` | room blend(0..1) / 명목 잔향 시간(0.2..5초) |
| `vocal echo 0.2`, `vocal echo-feedback 0.35` | 에코 blend(0..1) / feedback(0..0.85) |
| `status`, `stop` | 요청된 제어 상태 / 장치 해제 후 종료 |

일반형은 `route mic|aux|appN output|send|broadcast on|off`, `level mic|aux|appN output|send|broadcast <dB>`다. `appN`은 1부터 시작하는 현재 앱 슬롯이다.
`vocal` 명령의 값 범위는 시작 옵션과 같다. off 프리셋의 EQ/다이내믹스 관련 라이브 제어는 거절하며,
pre-delay, 에코 시간 및 프리셋 전환은 GPUI의 **실행 중 보컬 설정**에서 제공한다. 해당 CLI 명령은 없다.
꺼둔 경로의 level 변경은 켜짐이 아니고 전체 unmute도 개별 mute를 풀지 않는다.
AUX/앱 trim은 입력 timeline에서 PDC 전에 적용되며, mute/경로별 gain은 PDC 뒤에서 적용된다.
마이크는 `vocal trim`으로 조절한다. Scene 저장은 목표 trim을 기존 필드에 보존한다.
source/route 제어는 합산 전이어서 출력 queue 지연이 남는다. 급한 송출 중단은 queue 뒤의
`mute send`/`mute broadcast`/`mute all`을 사용한다. mute 중에도 queue를 소비해 오래된 소리를 재생하지 않는다.
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
CLI는 시작 시 지정한 창만 연다. 실행 중 재열기는 GPUI 앱에서 지원한다.
잘못된/중복 슬롯과 편집창 없는 플러그인은 CLI 시작 전에 거절한다.
`--vst3-chain-out` 정상 종료만 모든 슬롯 상태를 한 새 파일로 저장한다. 장치 중지→worker join→
editor 제거→마지막 편집 flush/processing stop→state 취득→native 해제→파일 게시 순서다.
어느 슬롯이든 실패하면 부분 파일을 게시하지 않는다. 오류·restart·암묵적 Drop에서는 저장하지 않는다.

체인 JSON schema 2는 아래처럼 작성할 수도 있다. `bypassed`는 필수이며 schema 1은 읽지 않는다.
`class_id`/`state`는 생략 가능하나 state가 있으면
32자리 hexadecimal class ID가 필요하다. 저장 snapshot에는 절대 경로와 확인된 class가 들어간다.

```json
{"schema": 2, "effects": [{"plugin": "plugins/EQ.vst3", "bypassed": false}, {"plugin": "plugins/Reverb.vst3", "bypassed": false}]}
```

이전 단일 플러그인 launch 옵션 `--vst3-state`, `--vst3-class`, `--vst3-state-out`은 제거했다.
별도 파일 도구의 단일 상태 형식은 그 도구에서만 사용하며 자동 변환하지 않는다.
**모든 plugin restart 요청은 경로 중단 사유**다. 일부 프리셋/oversampling/파라미터 변경도
중단될 수 있다. 명시적인 슬롯 추가·삭제·순서 변경과 지연 정렬 우회는 GPUI에서 지원한다.
CLI는 시작한 체인 순서를 유지하며 슬롯별 우회와 파라미터 상태 저장을 제공한다.

### 보컬 처리 지연 보상 (PDC)

최종 샘플레이트와 저장 상태로 체인을 초기화한 뒤 각 VST3의
`getLatencySamples` 값과 활성화된 내장 pitch correction의 고정 지연을 합한다.
VST3가 없는 경우에도 pitch correction 지연은 적용한다. 통화·방송 믹스의 물리 AUX 및 앱 신호에 그 샘플 수만큼
고정 지연을 적용한다. 모니터에는 이 추가 보상을 넣지 않으며 보컬 자체의 지연을 제거하지 않는다.
송출에 연결한 앱이나 AUX의 gain/mute/route 제어는 보상된 샘플에 적용하고, 연결이 꺼져 있어도
지연 이력은 계속 갱신된다. 출력 mute는 기존과 같이 출력 queue 뒤에서 적용한다.
보상 메모리는 시작 또는 명시적 체인 변경을 준비하는 제어 스레드에서 할당하며 전체 보컬 처리
지연은 최대 2초를 허용한다. 초과 시 시작/추가를 거절한다. 슬롯 편집에 따른 지연은 체인과 함께
교체하며, 마지막 VST3를 제거해도 활성 pitch correction의 지연은 남는다.
플러그인 자체의 지연 변경 알림을 포함한 restart 요청은 계속 경로 중단 사유다.
UI/CLI 진단은 신고 지연과 적용된 보상량을 구분한다.

보상 기준은 마이크 clock으로 변환된 샘플이다. 서로 다른 장치/프로세스의 캡처 timestamp,
입력 queue, 리샘플러 group delay, OS/hardware 지연 및 IIR 위상 차이까지 측정하거나
정렬하는 것은 아니다. 실제 청취·왕복 지연 검증과는 구분한다.

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

Windows CI의 기존 native fixture 검사도 따른다. 화면 검사는 UIAutomation과 Windows 키보드
입력으로 실제 창과 컨트롤을 조작한다. 임시 Scene의 장치 ID, 앱 실행 파일 및 플러그인 경로는 합성 값이며
오디오 시작을 누르지 않는다.
장치 편집은 실제 목록 조회 후 누락 ID·채널·시작값 유지, 저장 거절과 취소를 확인한다.
장치 교체 후 성공적인 파일 저장/재열기는 합성 catalog를 쓰는 모델 검사이며 실제 장치 검증과 구분한다.
실제 오디오 장치를 열거나 상용 VST3 GUI를 조작하는 검사가 아니다.
통과한 검사와 실제 장비·GUI·청취 수용을 구분한다. sample peak/GR은 LUFS/true peak가 아니다.

[DESIGN.md](DESIGN.md) · [STATUS.md](STATUS.md) · [AGENTS.md](AGENTS.md) · [NOTICE.md](NOTICE.md)

공개 저장소이며 제품 라이선스는 미정이다. 자동 녹음·클라우드 전송·드라이버 설치·공식 릴리스는 없다.

## 자체 가상 오디오 드라이버 개발

[drivers/virtual-audio](drivers/virtual-audio/README.md)에 Call/Broadcast/Return용 독립 PortCls/WaveRT
재생·캡처 쌍의 C++ 개발 소스와 INF를 추가했다. 각 쌍은 48 kHz stereo PCM32, 96-frame
FIFO로 연결하며 기존 Rust/GPUI 앱의 WASAPI 장치 경계를 그대로 사용한다.
Return은 외부 앱/DAW 재생을 MixLive AUX로 받기 위한 별도 케이블이다.
설치된 Return 재생 장치를 외부 앱에 명시적으로 선택하고, 대응 Return Capture를 MixLive AUX로
선택한다. 동일 앱을 process loopback으로 중복 캡처하거나 MixLive 출력을 Return으로 보내지 않는다.
기존 Scene 형식과 물리/가상 AUX 선택 경계는 그대로이며 이름 기반 자동 연결은 추가하지 않았다.
이 FIFO의 2 ms는 실제 왕복 지연이 아니다. 현재 미서명 소스·빌드 단계이며 장치 설치,
Windows에서의 실제 endpoint 노출 및 Discord/OBS 수신은 확인되지 않았다.
WDK와 INF 검사는 드라이버 변경에 한해 별도 경로 필터 CI에서 실행하며 Rust 앱 CI는 변경하지 않는다.
개발 소스에는 Inf2Cat 미서명 카탈로그 준비, SignTool 사전 검증 및 고정 장치 ID 상태 조회/명시적
개발 장치 등록·제거 스크립트가 포함된다. DevGen은 WDK 개발용 도구로 재배포하지 않으며
제품용 설치기는 아직 없다.
