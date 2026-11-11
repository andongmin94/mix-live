# 출처 및 라이선스 상태

MixLive의 라우팅·보컬 체인·검사 앱 코드는 신규 구현이다. SoundCraft/FilmCraft/Splitwave,
Steinberg SDK, Microsoft driver sample 소스를 직접 복사하거나 vendoring하지 않았다.
VST3 API는 아래 Rust 호스팅 라이브러리와 그 의존성을 통해 사용한다.

제품 전체 라이선스는 미정이다. 임의로 MIT/Apache 라이선스를 부여하지 않았으며
Cargo 패키지는 `publish = false`다. 외부 의존성의 라이선스는 별도로 유지한다.
사용자 음성, 유료 플러그인 바이너리·프리셋, SDK·드라이버는 포함하지 않는다.
ASIO feature는 꺼져 있다. 공식 설치 파일·드라이버 릴리스는 하지 않는다.

## 직접 의존성

정확한 전이 의존성은 Cargo.lock을 따른다. 전체 제품 라이선스 선택을 대신하지 않는다.

- GPUI Design (Apache-2.0), `andongmin94/gpui-design`:
  https://github.com/andongmin94/gpui-design/tree/385b0178fbbaa3d30de79fe04329aab62a257dc0
  이 revision의 Button/Switch/Slider/Root를 의존성으로 재사용한다. 앱의 색상과 레이아웃은
  MixLive가 정의하며 slider 표시의 배치는 upstream `two_styles` 예제의 계약을 따른다.
  원본은 GPUI Kit v0.9.0에서 분리되었고 Copyright 2024–2026 Longbridge를 포함한다.
  원본 라이선스 전문은 `licenses/gpui-design-APACHE-2.0.txt`에 보존한다. 별도 NOTICE는 없다.
- GPUI pre / platform 0.3.11 (Apache-2.0), Zed Industries:
  https://crates.io/crates/gpui-pre/0.3.11
  https://github.com/zed-industries/zed/tree/f816e1f9263e3a8ef63c4baaa02e1441ce1c6038
  GPUI Design과 같은 engine family를 정확히 고정하며 Windows 네이티브 backend를 사용한다.
  원본 Copyright 2022–2025 Zed Industries와 라이선스 전문은
  `licenses/gpui-APACHE-2.0.txt`에 보존한다. 바이너리 artifact에도 두 고지를 포함한다.

- CPAL 0.18.2 (Apache-2.0): https://github.com/RustAudio/cpal/tree/v0.18.2
- Rubato 5.0.1 (MIT OR Apache-2.0): https://github.com/HEnquist/rubato
- rtrb 0.4.0 (MIT OR Apache-2.0): https://github.com/mgeier/rtrb
- clap 4.6 (MIT OR Apache-2.0): https://github.com/clap-rs/clap
- biquad 0.6.0 (MIT OR Apache-2.0), Emil Fresk: https://github.com/korken89/biquad-rs
  Registry checksum: `8dee8f423f64504139bdc53d7fbe6a1cd8eff4f354be3652e7cf26385c9f1cb4`.
  신규 필터 구현을 복사하지 않고 라이브러리로 링크한다. coefficient와 DF2T API를 확인했다.
  biquad가 사용하는 num-traits 0.2.19의 libm feature 때문에 libm 0.2.16이 추가된다.
- vst3-host 0.11.1 (MIT), Helge Sverre:
  https://github.com/HelgeSverre/rust-vst3-host/tree/6c64cdffc6740cce0b4c5b6916724697b0ef9ff8
  기본 features를 끄고 Windows의 검사·편집·파일 처리 및 선택적 라이브 worker에서 사용한다.
  VST3 bindings (`vst3`), libloading 등의 전이 의존성을 포함하며 원본 저작권을 유지한다.
- serde / serde_json (MIT OR Apache-2.0), tempfile (MIT OR Apache-2.0),
  wait-timeout (MIT OR Apache-2.0): 설정 직렬화·새 파일 원자 저장·검사 자식 프로세스 제한.

필터 근거: Robert Bristow-Johnson의 Audio EQ Cookbook을 구현한 biquad를 사용한다.
https://www.w3.org/TR/audio-eq-cookbook/
컴프레서와 디에서는 MixLive의 소규모 streaming 구현이며 기존 제품의 모델링이나
청취 품질 동등성을 주장하지 않는다. 기술 조사 자료는 DESIGN.md를 함께 참조한다.

## 검사용 외부 소스

CI가 고정 commit에서 가져오는 다음 fixture는 저장소에 복사하거나 제품에 배포하지 않는다.
- `coupler-rs/vst3-rs` (`MIT OR Apache-2.0`), commit `b2a45f2d40e602eff5a224b41613735388868590`의 gain example.
- `HelgeSverre/rust-vst3-host` (`MIT`), commit `6c64cdffc6740cce0b4c5b6916724697b0ef9ff8`의 test-plugin.

## vst3-host MIT 고지

Copyright (c) 2026 Helge Sverre

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## biquad MIT 선택 고지

원문 LICENSE-MIT blob: `815d6c02a70f1aa7b99ab323ba14f048a0a2ec83`.

Copyright (c) 2018

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.

## WAV 처리 의존성

- hound 3.5.1, Ruud van Asseldonk (Apache-2.0): https://github.com/ruuda/hound
  WAV 파서·writer를 라이브러리로 재사용한다. 자체 WAV 파서를 만들지 않았다.
  crates.io registry checksum: `62adaabb884c94955b19907d60019f4e145d091c75345379e70d1ee696f7854f`.
  runtime 전이 의존성이 없으며 기존 lockfile의 다른 패키지 버전은 변경하지 않았다.

## 원본 편집창

- 기존 vst3-host 0.11.1의 PluginWindow를 재사용한다. egui/WebView 의존성은 켜지 않는다.
- winapi 0.3.9 (MIT OR Apache-2.0): https://docs.rs/crate/winapi/0.3.9
  기존 전이 의존성을 직접 선언하며 버전·checksum은 바꾸지 않았다. Win32 MSG와
  PeekMessageW/TranslateMessage/DispatchMessageW를 통한 메인 스레드 메시지 처리에만 쓴다.

## 장치 Scene 파일

- 기존 serde/serde_json과 tempfile 의존성을 장치 CLI에서도 직접 사용한다.
  JSON 직렬화와 완성된 새 파일 게시를 재사용하며 새 패키지·버전·checksum은 추가하지 않는다.
  DSP crate에는 직렬화 의존성을 넣지 않는다.

## 라이브 VST3 경로

`mixlive-audio-windows`도 기존 잠금 파일의 `vst3-host 0.11.1`을 직접 사용하고,
`mixlive-vst3`의 상태 파일 검증을 재사용한다. 새 외부 패키지 버전이나 원본 코드 복사는 없다.
플러그인은 in-process이며, 메인 스레드의 로딩/상태/종료와 별도 처리 스레드의 processing을
분리한다. 장치 callback 바깥에서도 호스트의 내부 mutex와 plugin 자체 동작의 제약은 남는다.
이 변경은 상용 플러그인 라이선스·인증을 제공하거나 바이너리를 재배포하지 않는다.
