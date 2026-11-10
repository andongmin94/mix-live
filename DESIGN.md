# MixLive 설계 v0.1

작성일: 2026-11-10 · 제품 요구사항 확정안 / 구현 단계와 검증 사실은 STATUS.md

## 1. 제품의 중심

**좋은 소리로, 원하는 사람에게, 지금 들려준다.**

MixLive는 작업장에 라이브 환경을 억지로 꾸리는 방식 대신, 실시간 청취·송출 환경
자체를 제품으로 만든다. 단순 믹서, 경량 DAW, OBS 전용 필터가 아니다.
주 사용자는 보컬 스트리머이며 게임·통화·연습, 기존 DAW의 앞단·뒷단에도 연결한다.

필수 가치 두 가지:

1. **Signature Sound:** 유료 VST 없이도 듣기 좋은 기본 보정. 사용자 VST3를 섞어
   자기만의 체인을 만들 수 있다. 이펙트 개수가 아니라 청취 결과로 평가한다.
2. **Audio Flow:** 목소리·MR·앱 소리를 목적지별로 다르게 보낸다. 사용자가 ASIO,
   loopback 3/4, 가상 케이블의 입력/출력 반대 이름을 학습하지 않아도 된다.

사용자의 MiniFuse는 모델 번호가 확인되지 않았다. 1/2/4 중 어느 모델인지 추정하거나
특정 채널 번호를 고정하지 않는다. 실제 장치 descriptor와 모델 확인 뒤 프로필을 검증한다.

## 2. 첫 수용 시나리오

Windows 11 x64, MiniFuse 또는 일반 USB 마이크, 헤드폰, Chrome의 MR, Discord.

- 마이크에는 내장 보컬 보정 또는 사용자 VST3를 적용한다.
- Discord 상대에게 목소리와 MR을 함께 보낸다.
- 본인은 원하는 목소리·MR·상대방 음성을 듣는다.
- 상대방 음성은 Discord 송신으로 되돌리지 않는다.
- 방송 믹스에는 통화 수신음·게임을 추가할 수 있다.
- 소스 보정은 해당 소스에만, 믹스 보정은 합쳐진 결과에만 적용한다.
- 앱 재시작 후 명시적으로 저장한 Scene과 플러그인 상태를 복원한다.

첫 실제 제품 완료는 **마이크 + 한 앱 소리 + 통화 믹스 + 모니터 믹스 + VST3 +
기본 보정 + 저장/재열기**가 실제 장비에서 끝까지 동작할 때다. UI 시안이나 합성
라우팅 데모만으로 이 기준을 충족하지 않는다.

## 3. 오디오 모델

한 경로의 순서:

`Source → input trim → source effect chain → per-mix send → Mix → mix effect chain → output gain/mute → Destination`

모니터도 별도 Mix다. 별도의 '목적지 이펙트'라는 중복 계층은 만들지 않는다.
같은 처리 결과가 필요한 목적지는 한 Mix를 공유하고, 다르게 처리하려면 Mix를 나눈다.

| 개념 | 데이터/동작 |
|---|---|
| Source | 물리 마이크/라인, 특정 앱의 소리, 시스템 출력, 선택적 DAW 반환 |
| EffectSlot | 내장 DSP 또는 VST3, 순서·bypass·상태·신고 지연 |
| Send | source→mix 연결, 목적지별 gain, on/off |
| Mix | 통화/방송/모니터 등 결합된 stereo 신호, 최종 체인 |
| Destination | 물리 출력 채널 또는 OS 가상 캡처 endpoint |
| Scene | 소스 바인딩, sends, 체인, 믹스, 목적지를 묶은 사용자 설정 |

소스는 한 번 처리하고 여러 Mix로 보낸다. 다른 목적지 때문에 같은 마이크 이펙트를
중복 실행하지 않는다. 첫 구조는 source→mix의 단방향 행렬이다. 임의 버스 재귀,
노드 스크립팅, 멀티호스트 분산 그래프를 먼저 만들지 않는다.

앱 내부 버스와 OS 가상 장치는 다른 객체다. `Call`이라는 Mix를 만들었다고 Windows
마이크 목록에 새 장치가 생기지 않는다. endpoint의 설치·연결까지 확인해야 한다.[R3]

### 예시: 목적지별 선택

| Source | 통화 송신 | 방송 송신 | 내 헤드폰 |
|---|---:|---:|---:|
| 보정된 마이크 | 켬 | 켬 | 사용자 선택 |
| Chrome MR | 켬 | 켬 | 한 경로로만 |
| 게임 | 끔 | 켬 | 한 경로로만 |
| Discord 수신 | **제외** | 선택 | 켬 |

Scene의 mix-minus 정책은 '특정 source를 이 Mix에 연결하지 않는다'는 규칙이다.
원하지 않는 음성을 자동 파형 분석으로 제거한다는 뜻이 아니다. 앱/장치 경계를 넘어
생기는 모든 피드백을 core가 알아낼 수 있다는 약속도 하지 않는다.

전체 데스크톱 캡처에 Chrome·Discord·MixLive 모니터가 이미 들어갈 수 있다.
안전한 기본 Scene은 앱별 캡처를 사용한다. 전체 캡처는 고급 선택으로 제공하며 같은
소리의 중복 가능성을 안내한다. 파형 빼기로 '나머지 데스크톱'을 만드는 기능은 금지한다.

## 4. 오디오 장치와 가상 출력

### 4.1 Windows 기준 경로

- 일반 장치: WASAPI. 장치 이름이 아닌 안정적인 endpoint 식별자를 저장한다.
- 오디오 인터페이스: 제조사 ASIO가 있을 때, 선택한 한 드라이버의 입력과 모니터 출력을
  같은 clock으로 처리하는 경로를 우선한다. 사용하지 못하면 이유를 보여주며 조용히 다른
  마이크/드라이버로 전환하지 않는다. ASIO 지원은 필수 목표지만 배포권 검토 전 번들하지 않는다.[R5]
- 앱 캡처: Windows Process Loopback. API는 특정 프로세스와 하위 프로세스를 대상으로
  하며 build 20348+ 조건이 있다. 일반 Windows 10 전체 지원이라고 광고하지 않고 제품
  기준을 Windows 11 x64로 잡는다.[R1]
- Chrome **전체 앱**과 특정 **탭**은 다른 요구사항이다. 첫 제품은 앱 단위다. 탭 단위
  캡처는 별도 브라우저 통합이 필요할 수 있어 첫 범위에서 제외한다.
- PID는 실행마다 달라질 수 있으므로 Scene에는 PID만 저장하지 않는다. 앱 식별정보로
  실행 중 후보를 다시 찾고, 여러 후보를 자동으로 섞지 않는다.

### 4.2 캡처와 모니터 중복

앱 캡처는 재생 경로를 대체하지 않는다.[R1] Chrome이 이미 헤드폰으로 소리를 보내는데
그 캡처를 MixLive 모니터에 더하면 원본과 지연된 소리를 동시에 듣는다.

두 명시적 구성만 제공한다.

- **원래 경로로 듣기:** 앱은 기존 장치로 재생한다. MixLive는 송출용으로만 캡처하며
  모니터 Mix에 그 소스를 기본으로 추가하지 않는다. 해당 소스의 MixLive 보정은 본인의
  원래 재생음에는 적용되지 않는다고 표시한다.
- **MixLive에서 듣기:** 앱 출력을 MixLive의 private render endpoint로 보낸다.
  이 경로는 자체적으로 스피커를 울리지 않는다. MixLive가 가공한 소리만 헤드폰으로 보낸다.
  endpoint가 준비되지 않았으면 이 모드를 사용 가능하다고 표시하지 않는다.

제조사의 Direct Monitor도 원음 모니터 경로다. DAW/앱 wet 모니터와 동시에 켜는 경우
이중 모니터링이 생길 수 있으므로 원음/보정음 중 무엇을 듣는지 선택하게 한다.

### 4.3 제품용 가상 장치

출시 목표는 설치 과정에 통합된 안정적인 가상 endpoint다. 내부 이름 예시:

| Windows에서의 방향 | 이름 예시 | 역할 |
|---|---|---|
| 녹음/캡처 장치 | MixLive Call | Discord 등에서 마이크로 선택 |
| 녹음/캡처 장치 | MixLive Broadcast | OBS 등에서 최종 믹스로 선택 |
| 재생/render 장치 | MixLive Return | 외부 앱/DAW가 소리를 보내는 private 입력 |

이는 **예정 이름**이며 현재 설치되어 있지 않다. 초기 endpoint 수는 고정한다.
버스를 만들 때마다 드라이버를 다시 설치하거나 OS endpoint를 무한히 생성하지 않는다.

제품 드라이버는 DSP를 하지 않고 버퍼 전달·시간 정보·명시적 endpoint만 담당한다.
VST와 사용자 플러그인은 커널에 절대 넣지 않는다. Windows 드라이버는 WDK/C++ 경계가
필요하면 허용한다. '100% Rust' 홍보를 위해 드라이버를 무리하게 재작성하지 않는다.

SysVAD는 참고 샘플이지 가상 오디오 케이블의 완성품이 아니다. 공식 설명에도 실제
믹싱 대신 톤을 생성하는 제한이 있다. 전송 프로토콜, clock, 실제 capture endpoint,
서명·설치·제거·절전·재연결 검증은 별도 개발·배포 과제다.[R3][R4]

개발 첫 경로는 사용자의 검증된 하드웨어 loopback 또는 기존의 적법하게 설치된 가상
장치를 목적지로 연결해 확인한다. 이것은 완성 UX가 아니라 **실장비 검증 경로**다.
드라이버 통합 여부와 무관하게 같은 destination 계약을 쓰므로 오디오 core를 버리지 않는다.
기존 드라이버의 무단 재배포, 자동 설치 또는 라이선스 동의를 대신 하지 않는다.

### 4.4 DAW와 연결

일반 가상 endpoint는 OBS/Discord와의 기본 연결이다. DAW의 ASIO 선택과 별도의
WASAPI endpoint가 저절로 결합된다고 가정하지 않는다. DAW 반환/다중 client 지원은
실제 드라이버의 동시 사용 결과로 판단한다. 필요성이 확인된 뒤 DAW bridge 플러그인을
추가하며, 처음부터 가상 ASIO 드라이버와 DAW용 VST3 둘 다 만들지 않는다.

## 5. 실시간 처리 구조

선택한 모니터 출력 장치 clock을 기준으로 한 audio processing loop를 둔다.
각 비동기 캡처와 별도 가상 출력은 경계의 bounded buffer를 거친다.
프로세스 캡처·물리 장치·가상 장치가 모두 48 kHz라고 해도 같은 clock은 아니다.

- 내부 신호는 planar f32, 시작 기본 sample rate는 48 kHz. 실제 지원 값을 먼저 조회한다.
- 처리 블록은 협상된 크기와 가변 길이를 지원한다. 512 samples를 엔진 상수로 강제하지 않는다.
- source timestamp를 engine timeline에 매핑한다. 장치 간 drift에는 경계의 지속적인
  비동기 리샘플링을 사용한다. 단순히 주기적으로 sample을 버리거나 반복하지 않는다.
- 리샘플러는 검증된 라이브러리로 도입하며, 선행 문서·타입·라이선스를 확인한다.
- audio callback에서 할당/해제, mutex 대기, 파일 I/O, 로그, JSON, 플러그인 생성·스캔·상태
  저장을 하지 않는다. UI의 telemetry 갱신 때문에 오디오 callback을 기다리게 하지 않는다.[R2]
- 이펙트·장치·Scene 교체는 제어 스레드에서 준비한다. 런타임에 필요한 값은 bounded
  전달 경로로 넘기고, 옛 플랜의 해제도 제어 쪽에서 한다. 구현 필요 시점에만 구체화한다.
- 실시간 parameter는 짧은 ramp로 바꾸고, topology/latency가 바뀌는 Scene은 소리와
  지연 차이를 검사한다. 모든 플러그인 preset을 무음/무지연 전환할 수 있다고 약속하지 않는다.

### 지연은 세 항목으로 구분

1. 장치/API가 보고한 buffer·stream 지연.
2. 플러그인 `getLatencySamples`가 신고한 지연.[R2]
3. 실제 loopback으로 측정한 end-to-end 지연.

`buffer / sample_rate`는 한 블록의 시간이지 왕복 지연이 아니다. 측정 전에는 UI에
'예상/신고값'이라고 표시한다. Live 모니터는 짧게, 송출용 믹스는 필요할 때 경로를
지연 정렬한다. PDC는 느린 경로를 빠르게 만들지 않는다. 큰 lookahead/linear phase
플러그인이 있으면 숨기지 않고 알린다. 인위적으로 -14 LUFS를 모든 통화의 목표로 강제하지 않는다.

## 6. 내장 보정과 사운드 품질

첫 제품의 기본 체인은 다음을 제공한다. 명칭과 순서는 프리셋별로 검증한다.

`Input trim → HPF/EQ → gentle dynamics → de-esser → reverb/delay → output protection`

음정 보정은 사용자 VST3로 먼저 제공하고, 자체 알고리즘은 독립 품질 검증 뒤 추가한다.
Gate는 배경잡음 제거와 다르다. 노래에 speech-only noise suppression을 자동 적용하지 않는다.
사운드가 끊기거나 원음이 파괴되는 강한 설정을 쉬운 UI 뒤에 숨기지 않는다.

초기 사운드 목표는 세 가지: 자연스러운 대화, 가까운 라이브 보컬, 넓은 노래 공간감.
새 프리셋 개수보다 이 세 가지의 좋은 실제 결과가 우선이다.

품질 수용:

- 같은 원본으로 bypass/보정본 음량을 맞춰 블라인드 청취한다. gain 상승을 개선으로 세지 않는다.
- 작은/큰 발성, 낮은/높은 음, 강한 치찰음, 비브라토, 말→노래 전환, 다른 마이크로 비교한다.
- 마이크 원본 보정과 보컬+MR 최종 믹스를 따로 듣는다. 리버브 꼬리·저역 펌핑·발음 손실을 점검한다.
- 프리셋별 불리한 조건을 기록한다. 사용자 원본은 명시 동의 없이는 저장·업로드하지 않는다.
- peak/RMS/필요 시 loudness·true peak 측정은 청취를 보조한다. 현재 core의 sample peak를
  true peak·LUFS 또는 mastering 품질의 증거로 부르지 않는다.
- 자동 개인화는 후속이다. 짧은 샘플에서 EQ·threshold 시작값을 제안할 수 있지만 모든 목소리에
  완벽한 보정을 보장하거나 정체성/성별 같은 불필요한 분류를 하지 않는다.

SoundCraft DSP는 후보이지 품질 보증이 아니다. 우선 실제 입출력/효과 단위를 검증하고,
필요한 모듈만 원래 라이선스와 출처를 보존해 도입한다. 모든 DAW 의존성을 가져오지 않는다.[R6]

## 7. VST3 호스팅

첫 제품은 VST3 audio effect를 지원한다. 가상악기·MIDI 작곡·CLAP/AU는 후속 범위다.

필수 기능: 설치 경로 스캔, class ID 식별, mono/stereo 배치 협상, 원본 editor 창,
parameter 변경, bypass, latency 보고, plugin state 저장/재열기, missing plugin 표시.
효과 순서가 Scene의 일부이며 내장/외부 효과의 배치 방식은 같다.

- 새 plugin 스캔은 별도 scanner process에서 수행한다. 실패한 바이너리를 자동으로 계속
  재실행하지 않는다. 이는 네이티브 외부 코드를 로드하는 경계의 격리다.
- 최초 runtime은 in-process를 기준으로 한다. native crash까지 Rust panic으로 격리할 수
  있다고 쓰지 않는다. scan 격리와 runtime 격리는 다르다. 런타임 sandbox는 실제 요구와
  비용을 확인한 뒤 별도 변경으로 결정한다.
- 플러그인 instantiate/destroy와 state 접근은 요구된 스레드에서 한다. 비오디오 작업을
  callback에 넣지 않는다. controller state와 processor state를 구분해 보존한다.[R2]
- latency/channel configuration 변경 알림을 처리한다. `maxSamplesPerBlock` 변경 시 필요한
  비활성화/재설정 순서를 지킨다. 호출마다 numSamples가 바뀌는 것은 허용한다.[R2]
- 누락 플러그인 때문에 사용자 허락 없이 원본 마이크를 송출하지 않는다. 실패한 Scene의
  송출을 시작하지 않고 누락 위치를 표시한다. 전체 runtime 복구 관리자를 미리 만들지 않는다.
- 사용자 VST의 라이선스·인증 상태는 사용자가 관리한다. 바이너리를 재배포하거나 인증을 우회하지 않는다.

VST SDK 3.8+의 공식 라이선스는 MIT이며, 옛 VST2/옛 SDK 조건을 그대로 적용하지 않는다.[R7]
ASIO는 별도 라이선스 결정이다. 저장소 공개 여부와 SDK·드라이버 배포권은 별개다.[R5]

## 8. GPUI 화면 구성

메인 화면은 하나다. Simple/Advanced라는 두 앱이나 두 audio engine을 만들지 않는다.
기본 화면은 쉬운 표현, 고급 세부값은 같은 채널의 상세 패널에서 펼친다.

- **상단:** 현재 Scene, 저장 상태, 전체 송출 mute, 실제 장치/지연 상태.
- **왼쪽 Sources:** 내 목소리, Chrome MR, 통화 수신 등 현재 소리. level·mute·source chain.
- **가운데 Sound:** 선택 채널의 보정 카드, 원음/보정 비교, VST 원본 창 열기.
- **오른쪽 Destinations:** 통화에 보내기, 방송에 보내기, 내가 듣기. 포함 source와 각각의 레벨.
- **연결 상세:** 행=Sources, 열=Mixes 행렬. 일반 사용자는 목적지 카드에서 source를 고른다.
- **장치 상세:** 실제 endpoint/ASIO channel, sample rate, buffer, monitor 방식.

UX의 진단은 관측 가능한 사실만 표시한다. input meter 정상→FX 뒤 무음→mix mute처럼
앱 내부 단계는 판별할 수 있다. OBS를 연동하지 않으므로 OBS가 선택한 입력이나 시청자가
실제로 받은 소리까지 자동 확인했다고 표시하지 않는다.

최초 연결은 '마이크 선택 → 말해서 level 확인 → 헤드폰 선택 → 송출 장치 확인'으로 한다.
다른 앱/OS의 기본 장치를 허락 없이 바꾸지 않는다. 장치가 빠지면 노트북 마이크로
몰래 전환하지 않고 해당 경로의 중단을 보여준다.

`andongmin94/gpui-design`의 실제 manifest에서 `gpui-pre = =0.3.11` 계열과 Apache-2.0을
확인했다. 첫 UI 작업 때 저장소 HEAD와 API를 다시 읽고 commit을 고정한다.
현재 core에는 GPUI 의존성을 추가하지 않는다. 오디오 계산은 UI toolkit과 무관하다.[R8]

## 9. 저장 구조·모듈 경계

### 현재 생성한 것

`crates/mixlive-core`: source/send/mix 설정 검증 + memory render. 장치·UI·VST·serde 의존성 없음.
8개 테스트와 `discord_singing` 합성 예제가 있다. Rust 실행 여부는 STATUS.md에서 구분한다.

현재 runtime index는 한 snapshot 안의 위치일 뿐이다. persistence ID나 장치 ID로 사용하지 않는다.
core는 모든 입력이 같은 시간축/sample rate로 들어온다는 계약이다. 이를 실제 장치에서
만들어주는 adapter가 아직 없다는 점을 테스트/README에 유지한다.

### 해당 기능을 구현할 때 추가할 모듈

| 모듈 | 역할 | 지금 빈 crate로 만들지 않는 이유 |
|---|---|---|
| mixlive-audio-windows | WASAPI/ASIO, process capture, clock 경계 | 장치 spike 이후 실제 API에 맞춰 생성 |
| mixlive-effects | 내장 DSP와 effect lifecycle | 음질 후보·의존성 검증 후 생성 |
| mixlive-vst3 | plugin host, scanner | 실제 한 plugin의 end-to-end로 시작 |
| app | GPUI 화면, Scene 저장, 제어 스레드 | core와 실제 장치 path를 먼저 확보 |
| virtual-device | endpoint transport/WDK driver 경계 | 서명·재배포 전략과 실제 transport를 결정 후 생성 |

프로젝트에 웹 서버, 로그인, 클라우드 DB, AI 에이전트 서비스, 원격 제어 TCP를 기본 추가하지 않는다.

저장 목표: 로컬 Scene manifest(JSON)와 slot별 binary plugin state. 명시적인 버전,
stable source/slot/mix ID, device binding을 분리한다. 저장은 control thread에서 atomic replace.
구식 포맷 이행 체계는 만들지 않는다. 개발 중 변경은 지원하지 않는 버전임을 명확히 알린다.
개인 장치명·플러그인 라이선스 데이터·사용자 음성은 저장소 fixtures에 넣지 않는다.

## 10. 단계와 실제 완료 조건

| 단계 | 최소 결과 | 완료 증거 |
|---|---|---|
| S0 설계/기초 | 현재 모델 + core + 최소 테스트 + Git | 컴파일·8개 테스트·예제 실제 실행 및 source diff |
| S1 장치 경로 | 선택한 mic→내장 gain→headphone/기존 loopback | MiniFuse 모델/OS/driver/rate/buffer 기록, 실측 왕복 지연, 클리핑 확인 |
| S2 보컬 랙 | 내장 핵심 체인 + VST3 1개 이상 + 상태 저장 | 원본 창, 음성 처리, 저장/재열기 동일 설정, 음량 맞춘 청취 비교 |
| S3 소스/믹스 | mic+Chrome→Call, Monitor, Broadcast | Discord return 제외, 원음 중복 없음, clock drift/연속 재생 확인 |
| S4 제품 연결 | 통합 가상 endpoint + Scene + 설치/제거 | 실제 Windows 장치로 노출, Discord/OBS 수신, 보안설정 해제 없는 서명 설치 |
| S5 음질 수용 | 대표 3개 Signature preset + 안정적 전환 | 여러 목소리/마이크 청취 수용, latency·dropout 기록, 설명과 실제 결과 일치 |

S1~S5의 설계는 현재 구현 완료와 다르다. 일정/시간/완성률은 임의로 산정하지 않는다.
S2와 S3를 병행할 수 있지만 통합 검증은 한 묶음의 완성 경로에서 한다.

시험 범위는 portable core 단위 검사 → 변경된 Windows adapter → 배포 묶음의 실제
장비 시나리오 순서다. 단일 CI job에서 core만 확인한다. GUI/Windows 코드를 넣는 시점에
관련 플랫폼 검사를 추가하고, 필요 없는 OS 매트릭스와 테스트 수량 목표는 두지 않는다.

기초 core의 입력/출력 gain은 기능 시연용이며 보컬 보정 출시를 대체하지 않는다.
float headroom은 보존한다. limiter/출력 보호가 없는 현재 예제를 물리 출력에 직접
연결하지 않는다. 품질 평가는 반드시 실제 신호 처리 구현 뒤 수행한다.

## 11. 공개·권리·미결정 사항

- 저장소는 사용자가 생성한 공개 `andongmin94/mix-live`다. 기본 브랜치는 main이다.
- 전체 제품 라이선스는 미정. 공개 가시성과 별개로 코드 재사용 라이선스를 임의로 부여하지 않는다.
- 이름은 사용자 결정대로 MixLive로 유지한다. 상표 사용권 확보를 주장하지 않는다.
- MiniFuse 모델/물리 채널과 Windows ASIO 구성은 실장비 연결 때 확인한다.
- ASIO 배포 조건, virtual driver 서명·인증서, 상용 드라이버 재배포 계약은 별도 승인 대상.
- SoundCraft/다른 코드 재사용은 license·고정 revision·테스트 범위를 실제 확인한 것만 반영한다.
- DAW plugin, CLAP/AU, macOS/Linux, 탭 캡처, 자체 pitch correction, 장치 전체 자동 복구는
  첫 요구를 완성한 뒤의 항목이다. 지금 추상화나 placeholder 구현으로 넣지 않는다.

## 12. 참고자료 — 2026-11-10 확인

아래는 설계 판단의 외부 근거다. 링크 내용이 변경되면 실제 구현 시 다시 확인한다.
라이선스 문서는 도입/배포 결정을 위한 확인 대상이며 법률 검토를 대신하지 않는다.

- **[R1] Microsoft, Application loopback audio capture** — process tree 캡처, endpoint와 분리,
  지원 build. https://learn.microsoft.com/en-us/samples/microsoft/windows-classic-samples/applicationloopbackaudio-sample/
- **[R2] Steinberg, Processing / Persistence** — process buffer, latency, realtime 제약, state.
  https://steinbergmedia.github.io/vst3_dev_portal/pages/FAQ/Processing.html
  https://steinbergmedia.github.io/vst3_dev_portal/pages/FAQ/Persistence.html
- **[R3] Microsoft, SysVAD sample** — virtual endpoint 샘플의 범위, 톤 생성/믹싱 미구현 제한.
  https://learn.microsoft.com/en-us/samples/microsoft/windows-driver-samples/sysvad-virtual-audio-device-driver-sample/
- **[R4] Microsoft, Driver signing** — 제품용 드라이버 서명/배포 경계.
  https://learn.microsoft.com/en-us/windows-hardware/drivers/install/driver-signing
- **[R5] Steinberg, Third-party developers / ASIO SDK licensing FAQ** — VST/ASIO 조건 구분.
  https://www.steinberg.net/developers/
  https://forums.steinberg.net/t/asio-sdk-proprietary-licensing-faq/1044024
- **[R6] SoundCraft, crates/dsp/Cargo.toml** — 후보 DSP의 경계와 의존성 확인. 아직 재사용하지 않음.
  https://github.com/storytold/soundcraft/blob/main/crates/dsp/Cargo.toml
- **[R7] Steinberg, VST 3 License** — SDK 3.8+ MIT, VST2 제외, 상표 규칙 별도.
  https://steinbergmedia.github.io/vst3_dev_portal/pages/VST%2B3%2BLicensing/VST3%2BLicense.html
- **[R8] 사용자 GPUI Design manifest** — 조회 blob `82fdcb9be246624a9f366d200b8999d03edb5b37`.
  https://github.com/andongmin94/gpui-design/blob/main/Cargo.toml
