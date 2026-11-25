# 현재 상태

기준일: 2026-11-25
단계: **실행 중 프리셋·공간계 시간 변경 검증 완료 · main 병합**

## 현재 구현

- `mixlive.exe`: 새 Scene/기존 파일 열기 → 장치·앱·보컬·VST3 설정 → 새 파일 저장 →
  명시적 시작·정지. GPUI Deck 고정 revision `78c0898ccb6feba03e3a3243ac7d4729fd80aee6`,
  GPUI family `=0.3.12`. 웹뷰 없음.
- WASAPI mic + 선택적 물리 AUX + 최대 8개 앱 프로세스 트리 → 모니터·통화·방송의 최대 세 독립 출력 mix. 통화/방송은 각각 선택 사항.
  같은 endpoint의 최대 세 분리 믹스는 6채널까지 한 스트림/clock bridge를 공유한다. 각 앱의 별도 MTA 캡처 클라이언트와
  bounded/Rubato clock bridge는 기존 wasapi 0.27.0 및 sysinfo 0.31.4를 사용한다.
  모든 앱을 준비한 뒤 시작하며, 보컬은 한 번만 처리하고 AUX/앱은 보컬 DSP/VST3를 우회한다.
- GPUI 앱 목록에 추가/삭제, 앱별 채널·trim·mute·모니터/통화/방송 연결 및 경로별 gain을
  제공한다. 새 앱은 모든 경로 off. 물리 AUX도 함께 사용할 수 있다. 중복/겹치는 프로세스 트리,
  모호한 루트, MixLive를 포함한 트리는 거절한다. 앱 이름이나 PID를 추정해 자동 선택하지 않는다.
- 실행 중 mic/AUX/앱별 mute 및 각 목적지로의 send·gain, master/믹스별 감쇠·출력 mute를 조절한다.
  AUX/앱 공통 trim은 기존 Scene 필드에서 시작하고 GUI/CLI에서 실시간 조절한다.
  입력 clock에서 5 ms ramp를 한 번 적용하고 PDC 이후 mute/send gain을 유지한다.
  실제 입력 peak·보컬 체인 전후·gain reduction·출력별 peak를 약 100ms마다 읽는다.
  앱별 peak와 clock 진단도 독립적이다. callback/worker 버퍼는 준비 시 고정 용량으로 만든다.
  출력의 기존 개별 sample clamp를 믹스별 stereo-linked limiter로 교체했다. ceiling 0.98,
  즉시 감쇠/최대 80 ms 선형 회복, lookahead 없음. 각 믹스의 pre-limit peak와 GR 및
  endpoint의 감쇠 frame 수를 표시한다. true peak·청력 보호·피드백 방지는 아니다.
  마이크 raw window를 기존 bounded SPSC로 control thread에 전달한다. RustFFT 자기상관과
  정확한 중첩 에너지 정규화로 pitch(70..1200 Hz, A440 note/cents/clarity)를 추정한다.
  Scene에 chromatic/major/natural minor 및 기준 조성을 저장하고 가장 가까운 허용음과 편차를 표시한다.
  표시용 관측은 오디오를 변경하지 않는다. 선택적 자동 보정은 별도로 0..1 강도와 5..200 ms
  retune smoothing을 설정한다. 같은 Estimator와 RustFFT 기반 sqrt-Hann 4-overlap phase vocoder를
  사용하고 1:1 샘플 수를 유지한다. 강도 0이면 추가 지연 없이 비활성, 활성 시 분석창만큼 고정 지연
  (48 kHz에서 2048 samples)을 추가하며 무성음은 정렬된 dry다. 실제 음질과 CPU는 미검증이다.
- GPUI의 실행 중 음정 설정 패널에서 조성·스케일·강도·retune을 하나의 명시적 적용으로
  변경한다. 활성 상태끼리의 변경은 같은 FFT·위상·overlap·delay 이력을 유지한다.
  켜기·끄기는 제어 스레드에서 준비한 보정기/PDC를 기존 단일 요청·반환 큐로 교체하고
  이전 저장소를 제어 스레드에서 해제한다. 잘못된 설정·합산 2초 지연 초과는 기존 설정을 유지한다.
  원신호 observer는 매 조회 때 승인된 조성/스케일을 사용하며 이전 target 계측을 비운다.
  이후 VST3 편집도 현재 pitch 지연을 사용한다. 지연 설명과 명시적 Scene 저장에는 현재
  실행 설정을 반영한다. 켜기 초기 무음·끄기 지연 변화는 있으며 무중단 교차 전환은 아니다.
- 정지 상태의 장치·출력 설정은 기존 장치/앱·채널·보컬·플러그인 설정과 모든 앱의 mixer 값을
  보존한다. 누락/지원 불가 장치와 앱을 명시하며 저장/시작을 거절하고 임의로 대체하지 않는다.
  보컬 설정은 장치 조회 없이 preset·trim·strength·threshold·VST3 순서와 상태를 편집한다.
  기존 CLI의 filesystem-only VST3 표준 경로 검색을 GPUI에서 재사용해 발견 후보를 명시적으로
  추가한다. native DLL 로딩·class/호환성 검증은 시작 시점이며 경로 검색 자체는 이를 보장하지 않는다.
  검색 성공 이후 명시적 다시 검색 버튼을 표시해 완료 상태를 접근성 트리에서 구분한다.
- 내장 pitch correction의 고정 latency와 최종 활성 VST3 체인의 신고 latency 합으로 통화·방송의
  AUX/앱 send를 정수 샘플 지연한다. VST3 없는 경로에도 pitch 지연 보상을 적용한다.
  모니터는 추가 보상하지 않는다. 지연 뒤에 소스 mute/route gain을 적용하며, 실제 보상이 필요할 때
  2초 상한을 넘으면 시작/추가를 거절한다. 체인 편집 시 새 저장소를 제어 스레드에서 준비하고
  체인과 같은 블록 경계에서 교체한다. 총 지연이 같으면 이력을 유지하고, 마지막 VST3 삭제 후에도
  활성 pitch correction 지연은 남는다. 플러그인 자체의 지연 변경 restart는 기존대로 경로를 중단한다.
  장치 timestamp/clock bridge/OS 지연을 실측·보상하는 것은 아니다.
- 내장 EQ/다이내믹스 뒤에 선택적 8-line orthogonal FDN stereo 리버브를 적용한다.
  기존 biquad 고역 감쇠를 재사용한다. wet 비율(0은 off), 명목 잔향 시간, wet-only pre-delay를
  GPUI/CLI에서 설정하고 저장한다. dry는 추가 지연하지 않으며 stereo VST3 입력을 유지한다.
  실행 중 trim·strength·compressor/deesser threshold·reverb mix/decay를 독립 atomic 목표값과
  10 ms sample ramp로 조절한다. zero strength/mix에서도 준비된 DSP 상태를 갱신한다.
  하이패스(40..200 Hz)와 180/2800/7500 Hz EQ gain 오프셋(±6 dB)을 GUI/CLI에서 실시간 편집한다.
  biquad Direct Form I의 이력을 유지하며 변경한 밴드의 정규화 계수를 10 ms 동안 전환한다.
  추가 delay buffer 없이 같은 마이크 처리 결과를 모든 연결된 믹스에 보낸다.
  compressor ratio(1..8), attack(1..50 ms), release(20..500 ms), deesser 최대 감쇠(0..12 dB)를
  기존 atomic target과 10 ms ramp로 실시간 변경한다. 새 기본값은 명시적 2.5:1, 8/140 ms, 최대 4 dB다.
  off 프리셋의 EQ/다이내믹스 파라미터는 비활성이다.
  리버브 뒤에 독립 stereo feedback echo를 추가했다. 최대 1초 delay buffer를 시작 전에 만들고
  mix(0..1), feedback(0..0.85)을 10 ms ramp로 실시간 조절한다.
  AUX/앱은 영향을 받지 않는다. 보컬은 한 번만 처리하고 mute는 리버브와 반복음을 포함해 적용한다.
- 실행 중 보컬 설정 패널에서 preset·reverb pre-delay(0..80 ms)·echo time(10..1000 ms)을
  함께 적용한다. 기존 16개 live target은 별도로 유지하며 현재 preset에 맞춰 fader를 갱신한다.
  활성 프리셋끼리는 EQ 계수를 10 ms 전환하고 EQ·다이내믹스 이력을 유지한다. off↔활성은
  제어 스레드에서 준비한 처리부만 기존 worker 요청·반환 큐로 교체하고 이전 상태를 반환해 해제한다.
  시간 변경은 최대 길이 ring의 읽기 위치를 10 ms 보간하며 reverb/echo 이력과 ramp를 유지한다.
  음정 보정·VST3 인스턴스·송출 PDC는 바꾸지 않는다. 승인된 세 값만 제어/UI에 게시하고
  Scene 저장에 반영하며, worker는 자신에게 적용된 preset을 읽어 설정 mutex를 사용하지 않는다.
  잘못된 값과 입력 sample rate에 맞지 않는 preset은 무변경 거절한다. 시간 변경의 효과음 변조와
  off 전환의 순간 소리 변화는 가능하며 실제 청취 품질은 미검증이다.
- VST3 슬롯마다 명시적 host bypass: 신고 지연만큼 정렬한 dry를 5 ms ramp로 교차 전환한다.
  같은 효과 인스턴스를 계속 처리하며 다른 슬롯과 PDC를 유지한다. 총 신고 지연 상한은 2초다.
  GPUI 실행 중/정지 설정 및 CLI `bypass N on|off`에서 조절하고 Scene/체인에 저장한다.
  음량 일치·CPU 절감·오류 시 dry fallback은 아니다.
- 실행 중 VST3 원본 창은 슬롯별 열기/닫기/재열기. 같은 실제 인스턴스를 유지하며, GPUI가
  메시지 루프를 소유한다. 창 조작과 native poll은 GPUI 변경 참조 밖에서 수행한다.
  실행 슬롯의 공통 파라미터 패널도 같은 인스턴스를 사용한다. native 표시 문자열/단계 수를 읽고
  선택한 값 하나를 명시적 적용으로 전달한다. 숨김은 제외하고 읽기 전용은 조작하지 않는다.
  조회한 값은 새로고침으로 갱신하며, native editor/processor 피드백을 DSP에 재전송하지 않는다.
  조회·포맷·변경은 foreground에서 수행하고 기존 최종 state flush/Scene 저장을 재사용한다.
- GPUI에서 실행 중 VST3 파일/폴더·설치 경로 후보 추가, 슬롯 삭제·위/아래 이동을 제공한다.
  빈 체인도 하나의 worker를 사용하며 첫 슬롯 추가·전체 삭제를 지원한다. 유지 슬롯은 같은
  native 인스턴스·파라미터·editor·dry 이력을 옮기고 새 플러그인만 제어 스레드에서 준비한다.
  capacity-1 요청/반환 큐로 블록 경계에서 체인/PDC를 교체한 뒤 제거 슬롯과 이전 버퍼를
  제어 스레드에서 해제한다. 준비 실패는 기존 체인에 영향을 주지 않는다.
  실행 목록은 저장된 Scene과 분리하며, 일반 정지는 변경을 버리고 정지 후 저장은 현재 순서와
  최종 상태를 저장한다. 공통 파라미터 패널은 체인 편집 때 닫아 이전 슬롯 index를 버린다.
  지연 변경 시 새 보상 이력의 시작 무음과 효과 꼬리 단절이 가능하며 무중단 교차 전환은 아니다.
- 정지 후 새 Scene 저장은 장치 중지 → 앱 capture thread join → VST3 worker join → 모든
  상태 수집 → 새 파일 게시를 사용한다. 실행 중 모든 소스/출력 mute·route·gain, 보컬 파라미터와 VST3 상태를
  다음 시작 전에 복원한다. 일반 정지/닫기/오류는 저장하지 않으며 파일 덮어쓰기 없음.
  schema 13은 음정 기준·보정 설정 및 EQ·다이내믹스·에코 설정과 슬롯별 `bypassed`와 `route.apps` 배열(실행 파일 경로와 슬롯별 설정), 방송 장치·채널·감쇠와 각 믹스의 상태 및 리버브 설정을 저장하고 PID나 잔향 버퍼는 저장하지 않는다.
  CLI의 직접 `run`/`scene run --scene-out`도 같은 전체 snapshot을 정상 정지 후 새 파일에 저장한다.
  종료 시 console 명령을 닫아 늦은 입력이 snapshot을 바꾸지 못하게 한다. 기존 파일·chain-out 동시 요청은
  시작 전에 거절한다. VST3 chain schema는 2이며 bypassed는 필수다.
  Scene schema 1~12 및 chain schema 1, 기존 단일 `aux_app` 필드와 `--aux-app` 옵션은 제거했다. CLI는 반복 `--app` 및
  `appN` 제어 명령을 사용한다.

- `drivers/virtual-audio/`에 고정 Call/Broadcast/Return PnP 하드웨어 ID와 독립 render/capture
  WaveRT miniport를 추가했다. Cableloom의 MIT 고지를 보존한 C++ WDK 경계이며 별도 관리자·IOCTL은 없다.
  48 kHz stereo PCM32, 96-frame 고정 FIFO, QPC 기반 clock, RUN 경계 초기화와 부재 시 무음/폐기를
  제공한다. 드라이버는 Rust 오디오/DSP/VST3/Scene 형식과 독립이다. 설치된 render ID만 기존 WASAPI에서
  선택할 수 있으며 실제 endpoint 노출은 아직 미검증이다.
  Return은 외부 앱/DAW의 render→capture→기존 AUX를 위한 별도 장치이며 실제 앱에서 선택·수신한 결과는 미검증이다.
  Return과 같은 앱의 process loopback 중복 캡처, Return으로의 자기 송출은 사용하지 않는다.
  개발용 manage-devices.ps1은 SignTool kernel-policy 및 catalog의 INF/SYS 검증 후에만
  명시적 관리자 승인으로 WDK DevGen/PnPUtil 등록을 수행한다. 고정 ID만 조회·제거하고
  중복 장치·자동 재시작·보안 우회를 허용하지 않는다. 제품 설치기는 별도 미구현이다.

## 검증

- [Audio #56](https://github.com/andongmin94/mix-live/actions/runs/45636823189) `a4df88b` 전체 성공:
  Windows workspace 검사 145개와 실제 native VST3 fixture 검사 13개, 총 **158개 성공**.
  Clippy `--workspace --all-targets --locked -- -D warnings`, locked Windows release 빌드,
  CLI 및 실제 GPUI Scene 생성·편집·취소·저장·재열기 검사 모두 통과했다.
  PR #6의 병합 tree가 검증한 source tree와 동일함을 확인했다.
- 프리셋·공간계 시간 변경은 준비 이후 도착한 EQ target 재조정, 기존 16개 target/ramp와
  활성 EQ·다이내믹스·FDN 이력 유지, pre-delay 0/80 ms·echo 10/1000 ms 및 중간 retarget,
  반복 plan과 block 1/17/256 독립성을 확인했다. 지원하지 않는 sample rate·잘못된 값은 거절한다.
  실제 mic impulse로 진행 중 pitch 이력과 별도 AUX/app PDC 이력 보존을 확인했고,
  idle worker 적용·승인 후 게시·Scene 저장과 native VST3 인스턴스/파라미터 보존도 검사했다.
- 음정 설정 변경의 목표 주파수·retune, 분석 이력 유지, block 1/17/137/256 독립성,
  켜기·끄기와 합성 VST 지연 0/71 samples에서 마이크·AUX/앱 송출 정렬을 확인했다.
  모니터 보상 제외, 이전 보정기 반환, 잘못된 설정·합산 지연 초과의 무변경 거절도 통과했다.
  입력 없는 worker의 설정 승인, 현재 observer 기준, UI 입력 파싱과 후속 VST3 지연 재준비도 검사했다.
- VST3 체인은 같은 지연의 PDC 이력 유지, 마지막 지연 제거, 잘못된 준비의 무변경 거절,
  bypass 목록 재배치를 검사했다. native fixture에서는 입력 없는 첫 슬롯 추가, 서로 다른
  파라미터의 인스턴스 동일성·순서 유지, 잘못된 경로/레이아웃/인덱스 거절, 전체 삭제 후
  오디오 처리·AUX/앱 격리·빈 체인 snapshot 및 편집한 순서/bypass의 snapshot을 확인했다.
  기존 state fixture의 파라미터 저장/복원과 라이브 EQ·limiter·Scene 13 검사도 통과했다.
- [Windows 개발 빌드](https://github.com/andongmin94/mix-live/actions/runs/45636823189/artifacts/15035221280):
  `mixlive.exe`, `mixlive-audio.exe`, `mixlive-plugins.exe`와 의존성/라이선스 고지.
  실장비의 실행 중 UI 조작·상용 VST3 호환성·실제 보컬 청취·지연/CPU는 검증하지 않았다.
- [Virtual Audio #4](https://github.com/andongmin94/mix-live/actions/runs/45569677959) `08d0007` 성공:
  세 고정 INF/PnP ID, transport 격리, WDK/INF/unsigned catalog 및 미서명 거절·부재 검사 완료.
  드라이버 서명·실제 설치/로드/제거, WASAPI 장치 노출 및 실제 Call/Broadcast/Return 오디오는 미검증이다.

## 남은 작업과 제한

- 실제 MiniFuse/가상 케이블/복수 앱 process loopback/상용 VST3 및 장치+UI 전체 실행은
  미검증이다. queue·plugin 신고값은 실측 왕복 지연이 아니며 sample peak는 LUFS/true peak가 아니다.
  다중 앱 캡처·음질·장시간 안정성을 검증 완료라고 부르지 않는다.
- 탭 선택·ASIO·실행 중
  소스 삽입/삭제·고급 formant 보존과 음정 보정 품질 수용은 미완료다(실험적 자동 보정은 구현).
  CLI 전체 Scene snapshot의 실제 장치 실행은 미검증이다. 실제 로딩·장치 등록/제거·전원 상태·HVCI·HLK·서명·Discord/OBS 및 DAW Return 수신은 미검증이다.
  음정 설정/프리셋/리버브/에코/라이브 파라미터의 청취·CPU·실제 상용 VST3 경로는 미검증이다.
- 모든 VST3 restart 요청은 경로 중단 사유다. 각 VST3는 단일 active stereo 입출력만 지원한다.
  in-process crash/hang과 control/worker mutex 위험이 남으며 hard-real-time 보장은 없다.
- 원래 앱 재생 경로·장치 기본값은 변경하지 않는다. 자동 mix-minus 추정/외부 피드백 검출/
  전체 데스크톱 폴백/자동 재연결/녹음 없음. MiniFuse 모델이나 채널 번호를 추정하지 않는다.
  개인 장치 ID/앱 경로/녹음/플러그인 상태는 공개하지 않는다. 드라이버 설치·서명·공식 릴리스 없음.

현재 상태는 이 파일, 이력은 Git, 실행 근거는 코드 commit별 Actions를 따른다.
