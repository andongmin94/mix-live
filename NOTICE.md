# 출처 및 라이선스 상태

MixLive는 신규 설계·라우팅·보컬 체인 코드다. SoundCraft, FilmCraft, Splitwave,
Steinberg SDK, Microsoft driver sample의 소스를 복사하거나 포함하지 않았다.
S0 코어는 외부 의존성이 없고 장치/DSP 경로는 아래 Cargo 의존성을 사용한다.

제품 전체 라이선스는 미정이다. 임의로 MIT/Apache 라이선스를 부여하지 않았으며
Cargo 패키지는 `publish = false`다. 외부 의존성의 라이선스는 별도로 유지한다.
사용자 음성, 유료 플러그인 바이너리·프리셋, SDK·드라이버는 포함하지 않는다.
ASIO feature는 꺼져 있다. 공식 설치 파일·드라이버 릴리스는 하지 않는다.

## 직접 의존성

정확한 전이 의존성은 Cargo.lock을 따른다. 전체 제품 라이선스 선택을 대신하지 않는다.

- CPAL 0.18.2 (Apache-2.0): https://github.com/RustAudio/cpal/tree/v0.18.2
- Rubato 5.0.1 (MIT OR Apache-2.0): https://github.com/HEnquist/rubato
- rtrb 0.4.0 (MIT OR Apache-2.0): https://github.com/mgeier/rtrb
- clap 4.6 (MIT OR Apache-2.0): https://github.com/clap-rs/clap
- biquad 0.6.0 (MIT OR Apache-2.0), Emil Fresk: https://github.com/korken89/biquad-rs
  Registry checksum: `8dee8f423f64504139bdc53d7fbe6a1cd8eff4f354be3652e7cf26385c9f1cb4`.
  新규 필터 구현을 복사하지 않고 라이브러리로 링크한다. coefficient와 DF2T API를 확인했다.
  biquad가 사용하는 num-traits 0.2.19의 libm feature 때문에 libm 0.2.16이 추가된다.

필터 근거: Robert Bristow-Johnson의 Audio EQ Cookbook을 구현한 biquad를 사용한다.
https://www.w3.org/TR/audio-eq-cookbook/
컴프레서와 디에서는 MixLive의 소규모 streaming 구현이며 기존 제품의 모델링이나
청취 품질 동등성을 주장하지 않는다. 기술 조사 자료는 DESIGN.md를 함께 참조한다.

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
