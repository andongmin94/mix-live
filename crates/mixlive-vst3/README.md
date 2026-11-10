# VST3 검사·원본 편집창·WAV 처리

Windows용 **장치 독립 검사·설정·파일 처리 도구**다. 플러그인을 마이크에 연결하거나 소리를 재생하지 않는다.
설치된 번들 경로를 찾고, 하나를 선택해 별도 프로세스에서 로딩·파라미터 변경·합성 버퍼
처리·상태 저장과 원본 편집창을 제공한다. 라이브 체인과 Scene UI는 아직 없다.

```powershell
mixlive-plugins.exe list
mixlive-plugins.exe list --directory "D:\My Plugins"
mixlive-plugins.exe inspect "C:\Program Files\Common Files\VST3\Example.vst3"
mixlive-plugins.exe inspect "C:\Program Files\Common Files\VST3\Example.vst3" --set 0=0.25 --state-out voice.mixlive-vst3.json
mixlive-plugins.exe inspect "C:\Program Files\Common Files\VST3\Example.vst3" --state-in voice.mixlive-vst3.json
```

`list`는 파일 경로만 나열하며 플러그인을 로드하지 않는다. `inspect`는 선택한 번들을
자식 프로세스에서 연다. 여러 audio class가 들어 있는 번들은 `--class-id <32자리 hex>`로
고른다. ID 없이 열면 라이브러리의 첫 audio class가 선택되므로 결과의 `plugin.uid`를 확인한다.
상태 파일을 읽을 때에는 그 파일의 ID를 사용한다. 다른 class로의 대체/이행은 허용하지 않는다.

`--set ID=value`의 값은 **0..1 normalized 값**이지 dB나 Hz가 아니다. 반환된 parameter 목록에서
ID, 이름, 단위와 현재 값을 확인한다. 읽기 전용·없는 parameter·NaN·범위 밖 값은 거절한다.

## 실제로 확인하는 것

- 선택한 native VST3의 로딩과 audio class ID.
- 입출력 bus와 channel 배치, parameter 정보, plugin 신고 latency.
- 48 kHz 기본(`--sample-rate`로 변경), 256 frames × 16개의 합성 buffer 처리.
- 출력 peak/RMS와 유한값 여부. 효과에 따라 무음이 나오는 것 자체를 오류로 단정하지 않는다.
- 지정 시 plugin이 제공한 component/controller state 저장 및 다음 실행에서 복원.

상태 파일은 schema 1이며 완성한 데이터를 **새 파일로만** 저장한다. 기존 파일을 덮어쓰지 않는다.
파일 내용은 plugin 고유 데이터이므로 사용자 프리셋처럼 취급하고 저장소에 올리지 않는다.
지원하지 않는 schema·다른 class ID·지나치게 큰 파일은 명확한 오류다. 구버전 마이그레이션은 없다.

검사는 기본 30초 제한(`--timeout-seconds 1..120`)으로 한 번만 실행한다. native crash/abort는
자식 프로세스 실패로 표시하고 재시도하지 않는다. 프로세스 분리는 **권한 격리 sandbox가 아니며**
플러그인은 사용자 권한으로 동작한다. 신뢰하고 적법하게 설치한 플러그인만 선택한다.

## 왜 마이크에는 아직 연결하지 않았나

`vst3-host 0.11.1`의 공개 API·상태 포맷·오류 반환을 재사용한다. 기본 features는 꺼서
CPAL/egui/별도 runtime helper를 추가하지 않는다. upstream `RealtimePluginRunner`에도 내부
mutex가 남아 있다고 문서화되어 있으므로 이 경로를 MixLive callback에 넣지 않는다.
검사 성공은 저지연, 라이브 안전성, native editor, 음질 또는 모든 상용 VST3의 호환성 인증이 아니다.
검사 코드는 향후 GUI의 plugin picker와 상태 로딩에서도 쓰는 제어 경계다.

## 검사 실행

일반 설정·파일 검사는 `cargo test -p mixlive-vst3 --locked`다. `tests/native.rs`와
`tests/wav_render.rs`, `tests/editor.rs`의 검사는 native fixture가 필요해 기본 실행에서는 ignored이고,
기존 Windows CI job이 소스에서 fixture를 빌드한 뒤
`cargo test -p mixlive-vst3 --tests --locked -- --ignored`로 실제 실행한다.

- `coupler-rs/vst3-rs` commit `b2a45f2d40e602eff5a224b41613735388868590`의 gain example:
  실제 DLL 로딩과 stereo PCM에 0.25 gain이 적용되는지 확인한다.
- `HelgeSverre/rust-vst3-host` commit `6c64cdffc6740cce0b4c5b6916724697b0ef9ff8`의 test-plugin:
  parameter를 바꾼 state가 **새 프로세스**에서 복원되는지 확인한다.

fixture는 CI 임시 경로에만 두며 설치하지 않고 제품 artifact에도 포함하지 않는다.

## WAV에 실제 이펙트 적용

```powershell
mixlive-plugins.exe render "C:\Plugins\Example.vst3" --input dry.wav --output wet.wav --set 0=0.25
mixlive-plugins.exe render "C:\Plugins\Example.vst3" --input dry.wav --output wet-tail.wav --state-in voice.mixlive-vst3.json --tail-ms 1000
```

`0=0.25`는 예시다. 실제 ID와 의미는 inspect에서 확인한다. `--class-id`로 audio class를
선택하며 상태 파일이 있으면 같은 class와 component/controller state를 복원한다.
중복 parameter ID는 실수로 서로 덮어쓰지 않고 거절한다.

- 입력: mono/stereo PCM 8/16/24/32-bit 또는 float32 WAV, 8..192 kHz. sample rate는 파일을 따른다.
- 출력: 같은 sample rate의 stereo float32. mono는 양쪽에 복제하며 stereo는 좌우를 유지한다.
- 한 active stereo 입력/출력 bus를 가진 효과만 허용한다. 악기·추가 bus는 누락하지 않고 거절한다.
- 파일은 256-frame 이하 블록으로 읽고 쓴다. 전체 녹음을 RAM에 읽지 않는다.
- `--tail-ms 0..30000`: 입력 뒤 무음을 명시한 길이만 추가한다. 전체 잔향 자동 탐지가 아니다.
- 출력은 normalize/limit하지 않으며 float headroom을 유지한다. sample peak와 plugin 신고 지연을
  보고하지만 LUFS·true peak·latency 보상·실시간 호환성 인증은 아니다.
- 플러그인 실행과 종료가 성공한 뒤 새 파일로만 게시한다. 기존 WAV/원본은 덮어쓰지 않는다.
  잘린 입력·비유한 sample·처리 중 bus/latency 변경·native 실패는 출력 게시 없이 종료한다.
- 기본 30초, `--timeout-seconds 1..120`의 작업 제한이다. 파일 길이 제한이나 재생 시간이 아니다.
  실패한 작업을 자동 재시도하지 않는다. native code는 사용자 권한이며 보안 sandbox가 아니다.

`tests/wav_render.rs`는 기존 Gain fixture로 613-frame 부분 블록, stereo 분리, float headroom,
44.1 kHz mono/24-bit 변환, 명시적 tail, 기존 출력 보존과 실패 시 미게시를 확인한다.
CI는 같은 두 fixture를 재사용하고 `--tests -- --ignored`로 native 검사를 실행한다.

## 원본 편집창에서 설정하기

```powershell
mixlive-plugins.exe edit "C:\Plugins\Example.vst3" --state-out voice.mixlive-vst3.json
mixlive-plugins.exe edit "C:\Plugins\Example.vst3" --state-in voice.mixlive-vst3.json --state-out voice-v2.mixlive-vst3.json
mixlive-plugins.exe render "C:\Plugins\Example.vst3" --input dry.wav --output wet.wav --state-in voice-v2.mixlive-vst3.json
```

- 제조사가 제공하는 VST3 원본 창이다. 새 egui/WebView UI를 만들거나 GPUI 메인 화면을 대신하지 않는다.
- 플러그인 로딩·창·메시지 처리·상태 접근·종료는 별도 editor 프로세스의 메인 스레드에 있다.
- `--class-id`, `--state-in`, `--set`, `--sample-rate`는 기존 검사와 같은 의미다.
- **창을 정상적으로 닫으면 완료**한다. `--state-out`을 명시한 경우에만 새 파일로 저장한다.
  생략하면 저장하지 않는다. 기존 파일 덮어쓰기·자동 저장·마이크 녹음은 하지 않는다.
- 대화형 편집에는 검사 도구의 30초 제한을 적용하지 않는다. 중단하려면 콘솔에서 Ctrl+C로
  종료한다. 비정상 종료 시 최종 상태 파일을 게시하지 않는다. 플러그인은 사용자 권한이며 sandbox가 아니다.
- Win32 메시지 처리 중에는 플러그인 mutex를 잡지 않는다. 창 크기/DPI/닫힘 처리는 기존
  `PluginWindow`를 사용한다. 플러그인 뷰를 분리한 뒤 부모 창을 파괴한다.
- 오디오 장치는 열지 않는다. GUI 변경은 **0-sample parameter flush**로 processor에도 전달하고
  component/controller 상태를 함께 저장한다. 오디오 미리듣기·보컬 실시간 처리 기능은 아니다.
- native editor가 없는 플러그인은 오류로 안내한다. 자동으로 다른 플러그인이나 원음으로 대체하지 않는다.
- CI는 설정 인자, 파일 보존, native editor 미지원, zero-sample 처리와 새 프로세스 상태 복원을
  기존 fixture로 검사한다. 실제 상용 플러그인 창의 그리기·조작·DPI는 별도 수동 검증 대상이다.

새 의존성 버전은 추가하지 않았다. 이미 lock에 있던 winapi 0.3.9를 메시지 처리의 직접
의존성으로 선언했다. unsafe 예외는 `editor::native::messages`에만 있으며 오디오 crate의
`forbid(unsafe_code)`는 바꾸지 않았다.
