# 출처 및 라이선스 상태

이 초기 저장소는 MixLive를 위한 신규 설계와 라우팅 기초 코드다.
SoundCraft, FilmCraft, Splitwave, Steinberg SDK, Microsoft driver sample의 소스를
직접 복사하거나 포함하지 않았다. S0 코어는 외부 의존성이 없고, S1 장치 경로는
아래 Cargo 의존성을 사용한다.

제품 전체의 공개 라이선스는 아직 결정하지 않았다. 따라서 임의의 MIT/Apache
라이선스를 부여하지 않았고, Cargo 패키지는 `publish = false`로 둔다.
이는 외부 의존성에 적용되는 라이선스를 변경하지 않는다.

향후 재사용 시 원본 저장소, 고정 commit/tag, 파일, 원래 라이선스, 필요한 고지를
이 파일에 기록한다. 제품명·로고·폰트·샘플·상용 플러그인의 권리는 코드 라이선스와
별도로 확인한다. 사용자가 설치한 VST3를 실행하는 것과 그 바이너리를 배포하는 것은
별개이며, 유료 플러그인을 번들에 포함하지 않는다.

기술 조사 출처와 아직 결정하지 않은 의존성은 DESIGN.md의 참고자료를 따른다.

## S1 직접 의존성

소스 복사나 SoundCraft 포크 없이 Cargo 의존성을 사용한다. 정확한 전이 의존성은 Cargo.lock을 따른다.
- CPAL 0.18.2 (Apache-2.0): https://github.com/RustAudio/cpal/tree/v0.18.2
- Rubato 5.0.1 (MIT OR Apache-2.0): https://github.com/HEnquist/rubato
- rtrb 0.4.0 (MIT OR Apache-2.0): https://github.com/mgeier/rtrb
- clap 4.6 (MIT OR Apache-2.0): https://github.com/clap-rs/clap

ASIO 기능은 켜지 않으며 SDK를 다운로드·동봉하지 않는다. 사용자 플러그인·드라이버는 포함하지 않는다.
이 목록은 전체 제품의 라이선스 선택을 대신하지 않는다.
