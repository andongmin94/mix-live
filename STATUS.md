# 현재 상태

기준일: 2026-11-11
단계: **장치/보컬/VST3 체인을 한 Scene으로 저장·재실행 구현, Windows 통합 검사 대기**

## 이번 변경

- 기준 main `449d8919ac1c03a03ce46f15842ea6aab8cfe20b`.
- AudioScene schema 3: 기존 route와 ordered effects(path/class/state)를 한 JSON에 저장한다.
  `scene save --vst3-chain`은 체인 파일 참조가 아니라 상태 자체를 포함한다.
  반복 `--vst3`도 저장 가능하되 native loading 없이 초기 상태의 선택만 저장한다.
- save/show는 장치나 플러그인을 열지 않는다. scene run은 저장된 체인만 사용하고 외부 선택
  덮어쓰기를 거절한다. 실행별 editor 슬롯/chain-out 선택은 가능하다.
- CLI 파일 선택과 native 실행을 ResolvedPlugins로 분리했다. 파일은 한 번만 해석한다.
  source 체인 삭제 후에도 Scene의 설정/state는 남는다. DLL/외부 자산은 별도 설치가 필요하다.
- 상대 경로는 원래 chain/scene 기준이며 CLI 경로는 저장 전에 절대화한다. schema 1/2 이행 없음.
- 기존 오디오 경로·DSP·native processing/종료 순서는 유지했다. dependency/lockfile/CI 변경 없음.
- Scene은 시작 설정이다. 실행 중 routing/mute/gain과 편집한 VST3 상태를 전체 Scene으로
  다시 저장하는 세션 snapshot은 아직 없다. 편집 결과는 명시적 chain-out으로 내보낸다.

## 검증

기존 scene/native fixture 검사에 embedding·경로·중복 상태·덮어쓰기 거절 검사를 추가했다.
새 컴파일/테스트/Clippy 결과는 아직 확인 전이다. 현재 로컬 Rust 도구체인/외부 DNS는 없으며
기존 Windows Actions를 사용한다. 이전 성공을 이번 변경의 성공으로 취급하지 않는다.

## 다음 개발

- GPUI + 기존 gpui-design으로 Scene 열기/명시적 시작·정지/라이브 라우팅을 잇는 작은 실제 화면.
  정적 데모로 대체하지 말고 기존 실행 경로를 재사용한다. UI 의존성의 최신 문서·타입을 먼저 확인.
- 앱별 직접 캡처, 전체 실행상태 저장, native editor 재열기, 플러그인 지연 보상.
- 실제 MiniFuse/상용 VST3 GUI·청취·왕복 지연·장시간 안정성은 별도 실장비 검증 항목.
- 자체 가상 장치·ASIO·내장 공간계/음정 보정은 미구현. 모든 native restart는 중단 사유이며
  in-process crash/hang과 control/worker mutex 위험은 남아 있다.

현재 상태는 이 파일, 이력은 Git, 실행 근거는 commit별 Actions를 따른다.
