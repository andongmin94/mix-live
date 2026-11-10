# VST3 preflight

Windows용 **비실시간 검사 도구**다. 플러그인을 마이크에 연결하거나 소리를 재생하지 않는다.
설치된 번들 경로를 찾고, 하나를 선택해 별도 프로세스에서 로딩·파라미터 변경·합성 버퍼
처리·상태 저장을 수행한다. 원본 편집창, 라이브 체인, Scene UI는 아직 없다.

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

상태 파일은 schema 1이며 **새 파일로만** 원자적으로 저장한다. 기존 파일을 덮어쓰지 않는다.
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

일반 설정·파일 검사는 `cargo test -p mixlive-vst3`다. `tests/native.rs` 두 검사는 native fixture가
필요해 기본 실행에서는 ignored이고, 기존 Windows CI job이 소스에서 fixture를 빌드한 뒤
`cargo test -p mixlive-vst3 --test native --locked -- --ignored`로 실제 실행한다.

- `coupler-rs/vst3-rs` commit `b2a45f2d40e602eff5a224b41613735388868590`의 gain example:
  실제 DLL 로딩과 stereo PCM에 0.25 gain이 적용되는지 확인한다.
- `HelgeSverre/rust-vst3-host` commit `6c64cdffc6740cce0b4c5b6916724697b0ef9ff8`의 test-plugin:
  parameter를 바꾼 state가 **새 프로세스**에서 복원되는지 확인한다.

fixture는 CI 임시 경로에만 두며 설치하지 않고 제품 artifact에도 포함하지 않는다.
