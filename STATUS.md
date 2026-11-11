# 현재 상태

기준일: 2026-11-11
단계: **순차 VST3 보컬 체인·슬롯별 편집창·일괄 상태 저장 구현, Windows 통합 검사 전**

- 기준 main: `0ecd808d50958094353e6d9ea4b407e5a7d55fcc`.
- `--vst3`를 반복해 체인 순서를 지정한다. 내장 보컬 뒤에서 한 번만 처리하고 AUX는 우회한다.
  같은 DLL의 중복 슬롯도 독립 인스턴스/상태다. 체인 전체에 worker와 input queue는 하나다.
- `--vst3-editor <1-based slot>`를 반복해 필요한 원본 창만 연다. 창 닫기는 해당 editor만 제거한다.
- 정상 종료의 `--vst3-chain-out`은 모든 슬롯의 경로/class/state를 한 파일에 저장하고,
  `--vst3-chain`으로 복원한다. 상대 plugin 경로는 체인 파일 기준이며 저장 snapshot은 절대 경로다.
  어느 슬롯이든 실패하면 부분 파일을 게시하지 않는다. 오류 중에도 모든 슬롯을 종료한다.
- 단일 plugin state/class launch 옵션은 제거했다. route Scene schema 2와 별도 단일 VST3 도구는 유지한다.
- config/file, native 처리, native tests를 분리했다. dependency/lockfile/CI 변경 없음.

## 검증 상태

- 신규 source 준비·Git 공백 검사 완료. 로컬 cargo/rustc와 네트워크 실행 환경이 없어 Rust 검사는
  기존 Windows CI에서 수행한다. 아직 새 코드 컴파일/테스트/Clippy/빌드를 통과했다고 보고하지 않는다.
- 관련 검사는 순서/중복 슬롯, 두 native 효과의 연속 적용·stereo/부분 블록·AUX 분리,
  마지막 편집의 다중 state 복원, 실패/Drop/기존 파일 보존, 잘못된 chain과 editor index를 다룬다.

## 남은 범위

실제 MiniFuse/상용 VST3 GUI·청취·왕복 지연·장시간 안정성, GPUI 메인 화면, 앱 직접 캡처,
자체 가상 장치, ASIO, plugin 지연 보상, 전체 Scene과 실행 중 체인 편집은 미완료다.
Native crash/hang 격리·hard-real-time 보장은 없고 자동 녹음/클라우드 전송/드라이버 설치는 하지 않는다.
현재 상태는 이 파일, 이력은 Git, 실행 증거는 commit별 Actions를 따른다.
