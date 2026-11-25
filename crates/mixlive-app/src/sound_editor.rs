//! Startup vocal settings and an ordered VST3 chain. This view never loads native plugins.
use std::path::PathBuf;

use gpui::{
    App, AppContext as _, Context, Div, Entity, InteractiveElement, IntoElement, ParentElement,
    PathPromptOptions, Render, Role, StatefulInteractiveElement, Styled, Subscription, Window, div,
    px, rgb,
};
use gpui_deck::{InputState, SelectItem, SelectState, StyledExt as _};

use crate::{
    form::{number_input, select_control, selected, text_input},
    sound::SoundDraft,
    ui::{ACCENT, BORDER, DANGER, MUTED, button, panel, section_title},
};

#[derive(Clone, Copy)]
enum Pick {
    Plugins,
    Bundle,
    Chain,
}

struct InstalledPlugins {
    paths: Vec<PathBuf>,
    picker: Entity<SelectState>,
    _subscription: Subscription,
}

pub struct SoundEditor {
    draft: SoundDraft,
    preset: Entity<SelectState>,
    input_gain: Entity<InputState>,
    strength: Entity<InputState>,
    compressor: Entity<InputState>,
    deesser: Entity<InputState>,
    eq_hpf_hz: Entity<InputState>,
    eq_body_db: Entity<InputState>,
    eq_presence_db: Entity<InputState>,
    eq_air_db: Entity<InputState>,
    compressor_ratio: Entity<InputState>,
    compressor_attack_ms: Entity<InputState>,
    compressor_release_ms: Entity<InputState>,
    deess_max_db: Entity<InputState>,
    reverb_mix: Entity<InputState>,
    reverb_decay: Entity<InputState>,
    reverb_predelay: Entity<InputState>,
    echo_mix: Entity<InputState>,
    echo_time: Entity<InputState>,
    echo_feedback: Entity<InputState>,
    busy: bool,
    picking: bool,
    scanning: bool,
    installed: Option<InstalledPlugins>,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl SoundEditor {
    pub fn new(draft: SoundDraft, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let preset = cx.new(|cx| {
            SelectState::new(
                [
                    SelectItem::new("off", "Off · EQ/다이내믹스 끄기"),
                    SelectItem::new("natural", "Natural · 자연스럽게"),
                    SelectItem::new("warm", "Warm · 따뜻하게"),
                    SelectItem::new("bright", "Bright · 밝게"),
                ],
                cx,
            )
            .default_value(&draft.preset)
        });
        let input_gain = text_input("마이크 입력 trim", &draft.input_gain_db, window, cx);
        let strength = text_input("보컬 보정 비율", &draft.strength, window, cx);
        let compressor = text_input(
            "컴프레서 threshold",
            &draft.compressor_threshold_db,
            window,
            cx,
        );
        let deesser = text_input("디에서 threshold", &draft.deess_threshold_db, window, cx);
        let eq_hpf_hz = text_input("하이패스 주파수", &draft.eq_hpf_hz, window, cx);
        let eq_body_db = text_input("저역 EQ 보정", &draft.eq_body_db, window, cx);
        let eq_presence_db = text_input("존재감 EQ 보정", &draft.eq_presence_db, window, cx);
        let eq_air_db = text_input("고역 EQ 보정", &draft.eq_air_db, window, cx);
        let compressor_ratio = text_input("컴프레서 ratio", &draft.compressor_ratio, window, cx);
        let compressor_attack_ms = text_input("컴프레서 attack", &draft.compressor_attack_ms, window, cx);
        let compressor_release_ms = text_input("컴프레서 release", &draft.compressor_release_ms, window, cx);
        let deess_max_db = text_input("디에서 최대 감쇠", &draft.deess_max_db, window, cx);
        let reverb_mix = text_input("리버브 비율", &draft.reverb_mix, window, cx);
        let reverb_decay = text_input("잔향 시간", &draft.reverb_decay_secs, window, cx);
        let reverb_predelay = text_input("리버브 pre-delay", &draft.reverb_predelay_ms, window, cx);
        let echo_mix = text_input("에코 비율", &draft.echo_mix, window, cx);
        let echo_time = text_input("에코 시간", &draft.echo_time_ms, window, cx);
        let echo_feedback = text_input("에코 피드백", &draft.echo_feedback, window, cx);
        let mut subscriptions = vec![cx.observe(&preset, |_, _, cx| cx.notify())];
        for input in [&input_gain, &strength, &compressor, &deesser, &eq_hpf_hz, &eq_body_db, &eq_presence_db, &eq_air_db, &compressor_ratio, &compressor_attack_ms, &compressor_release_ms, &deess_max_db, &reverb_mix, &reverb_decay, &reverb_predelay, &echo_mix, &echo_time, &echo_feedback] {
            subscriptions.push(cx.observe(input, |_, _, cx| cx.notify()));
        }
        Self {
            draft,
            preset,
            input_gain,
            strength,
            compressor,
            deesser,
            eq_hpf_hz,
            eq_body_db,
            eq_presence_db,
            eq_air_db,
            compressor_ratio,
            compressor_attack_ms,
            compressor_release_ms,
            deess_max_db,
            reverb_mix,
            reverb_decay,
            reverb_predelay,
            echo_mix,
            echo_time,
            echo_feedback,
            busy: false,
            picking: false,
            scanning: false,
            installed: None,
            error: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn draft(&self, cx: &App) -> SoundDraft {
        SoundDraft {
            preset: selected(&self.preset, cx).unwrap_or_default(),
            input_gain_db: self.input_gain.read(cx).value().to_string(),
            strength: self.strength.read(cx).value().to_string(),
            compressor_threshold_db: self.compressor.read(cx).value().to_string(),
            deess_threshold_db: self.deesser.read(cx).value().to_string(),
            eq_hpf_hz: self.eq_hpf_hz.read(cx).value().to_string(),
            eq_body_db: self.eq_body_db.read(cx).value().to_string(),
            eq_presence_db: self.eq_presence_db.read(cx).value().to_string(),
            eq_air_db: self.eq_air_db.read(cx).value().to_string(),
            compressor_ratio: self.compressor_ratio.read(cx).value().to_string(),
            compressor_attack_ms: self.compressor_attack_ms.read(cx).value().to_string(),
            compressor_release_ms: self.compressor_release_ms.read(cx).value().to_string(),
            deess_max_db: self.deess_max_db.read(cx).value().to_string(),
            reverb_mix: self.reverb_mix.read(cx).value().to_string(),
            reverb_decay_secs: self.reverb_decay.read(cx).value().to_string(),
            reverb_predelay_ms: self.reverb_predelay.read(cx).value().to_string(),
            echo_mix: self.echo_mix.read(cx).value().to_string(),
            echo_time_ms: self.echo_time.read(cx).value().to_string(),
            echo_feedback: self.echo_feedback.read(cx).value().to_string(),
            effects: self.draft.effects.clone(),
        }
    }

    pub fn is_busy(&self) -> bool {
        self.busy || self.picking || self.scanning
    }

    pub fn set_busy(&mut self, busy: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.busy = busy;
        self.update_disabled(window, cx);
    }

    fn update_disabled(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let disabled = self.is_busy();
        self.preset
            .update(cx, |state, cx| state.set_disabled(disabled, window, cx));
        if let Some(installed) = &self.installed {
            installed.picker.update(cx, |state, cx| state.set_disabled(disabled, window, cx));
        }
        for input in [
            &self.input_gain,
            &self.strength,
            &self.compressor,
            &self.deesser,
            &self.eq_hpf_hz,
            &self.eq_body_db,
            &self.eq_presence_db,
            &self.eq_air_db,
            &self.compressor_ratio,
            &self.compressor_attack_ms,
            &self.compressor_release_ms,
            &self.deess_max_db,
            &self.reverb_mix,
            &self.reverb_decay,
            &self.reverb_predelay,
            &self.echo_mix,
            &self.echo_time,
            &self.echo_feedback,
        ] {
            input.update(cx, |state, cx| state.set_disabled(disabled, cx));
        }
        cx.notify();
    }

    fn pick(&mut self, kind: Pick, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        self.picking = true;
        self.error = None;
        self.update_disabled(window, cx);
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: !matches!(kind, Pick::Bundle),
            directories: matches!(kind, Pick::Bundle),
            multiple: matches!(kind, Pick::Plugins),
            prompt: Some(
                match kind {
                    Pick::Plugins => "VST3 파일 추가",
                    Pick::Bundle => "VST3 폴더 추가",
                    Pick::Chain => "MixLive 체인 파일 추가",
                }
                .into(),
            ),
        });
        cx.spawn_in(window, async move |owner, cx| {
            let result = paths.await;
            let _ = cx.update(move |window, cx| {
                let _ = owner.update(cx, |this, cx| {
                    this.picking = false;
                    this.error = match result {
                        Ok(Ok(Some(paths))) => match kind {
                            Pick::Plugins | Pick::Bundle => this.draft.append_plugins(paths).err(),
                            Pick::Chain => paths
                                .first()
                                .and_then(|path| this.draft.append_chain(path).err()),
                        },
                        Ok(Ok(None)) => None,
                        Ok(Err(error)) => Some(error.to_string()),
                        Err(error) => Some(error.to_string()),
                    };
                    this.update_disabled(window, cx);
                });
            });
        })
        .detach();
    }

    /// Search standard VST3 installation paths using the same filesystem-only list as the CLI.
    /// No native DLL is loaded, even if a discovered candidate is not a valid plugin.
    fn scan_installed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_busy() { return; }
        self.scanning = true;
        self.installed = None;
        self.error = None;
        self.update_disabled(window, cx);
        let scan = cx.background_executor().spawn(async {
            mixlive_vst3::host::list(&[]).map_err(|error| error.to_string())
        });
        cx.spawn_in(window, async move |owner, cx| {
            let result = scan.await;
            let _ = cx.update(move |window, cx| {
                let _ = owner.update(cx, |this, cx| {
                    this.scanning = false;
                    match result {
                        Ok(paths) => {
                            let items = std::iter::once(SelectItem::new("", "추가할 VST3 선택"))
                                .chain(paths.iter().enumerate().map(|(index, path)| {
                                    // Keep the original OS path, not the lossy display string, as identity.
                                    SelectItem::new(index.to_string(), path.to_string_lossy().into_owned())
                                }))
                                .collect::<Vec<_>>();
                            let picker = cx.new(|cx| SelectState::new(items, cx).default_value(""));
                            let subscription = cx.observe(&picker, |_, _, cx| cx.notify());
                            this.installed = Some(InstalledPlugins { paths, picker, _subscription: subscription });
                        }
                        Err(error) => this.error = Some(error),
                    }
                    this.update_disabled(window, cx);
                });
            });
        }).detach();
    }

    fn add_installed(&mut self, cx: &mut Context<Self>) {
        if self.is_busy() { return; }
        let Some(installed) = &self.installed else { return; };
        let Some(index) = selected(&installed.picker, cx).and_then(|value| value.parse::<usize>().ok()) else { return; };
        let Some(path) = installed.paths.get(index) else { return; };
        self.error = self.draft.append_plugins(vec![path.clone()]).err();
        cx.notify();
    }

    fn move_slot(&mut self, index: usize, target: usize, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        self.error = self.draft.move_effect(index, target).err();
        cx.notify();
    }

    fn remove_slot(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        self.error = self.draft.remove_effect(index).err();
        cx.notify();
    }

    fn chain(&self, window: &Window, cx: &Context<Self>) -> Div {
        let disabled = self.is_busy();
        let scan_label = if self.installed.is_some() { "설치된 VST3 다시 검색" } else { "설치된 VST3 검색" };
        let mut chain = panel().gap_4()
            .child(div().flex().items_center().justify_between().gap_3()
                .child(section_title("VST3", "보컬 플러그인 순서"))
                .child(div().text_xs().text_color(rgb(MUTED))
                    .child(format!("{}개", self.draft.effects.len()))))
            .child(div().flex().flex_wrap().gap_2()
                .child(button("scan-vst3", scan_label, false).disabled(disabled)
                    .on_click(cx.listener(|this, _, window, cx| this.scan_installed(window, cx))))
                .child(button("add-vst3-files", "VST3 파일 추가", false).disabled(disabled)
                    .on_click(cx.listener(|this, _, window, cx| this.pick(Pick::Plugins, window, cx))))
                .child(button("add-vst3-bundle", "VST3 폴더 추가", false).disabled(disabled)
                    .on_click(cx.listener(|this, _, window, cx| this.pick(Pick::Bundle, window, cx))))
                .child(button("add-chain-file", "체인 파일 추가", false).disabled(disabled)
                    .on_click(cx.listener(|this, _, window, cx| this.pick(Pick::Chain, window, cx)))))
            .child(div().text_xs().text_color(rgb(MUTED))
                .child("위에서 아래로 처리합니다. 체인 파일은 현재 목록 뒤에 추가되며, 같은 플러그인도 별도 슬롯입니다."));
        if self.scanning {
            chain = chain.child(div().text_sm().text_color(rgb(ACCENT)).child("VST3 설치 경로 조회 중"));
        }
        if let Some(installed) = &self.installed {
            let count = installed.paths.len();
            let mut results = div().flex().flex_col().gap_2()
                .child(div().font_semibold().child("VST3 검색 결과"))
                .child(div().text_xs().text_color(rgb(MUTED)).child(format!("표준 설치 경로에서 발견한 후보 {count}개 · 실제 호환성은 시작할 때 확인")));
            if count == 0 {
                results = results.child(div().text_sm().child("발견된 후보가 없습니다. VST3 파일 또는 폴더를 직접 추가할 수 있습니다."));
            } else {
                results = results.child(div().flex().items_center().gap_2()
                    .child(div().flex_1().min_w_0().child(select_control("installed-vst3", "설치된 VST3 선택", &installed.picker, None, window, cx)))
                    .child(button("add-installed-vst3", "선택한 VST3 추가", false)
                        .disabled(disabled || selected(&installed.picker, cx).is_none())
                        .on_click(cx.listener(|this, _, _, cx| this.add_installed(cx)))));
            }
            chain = chain.child(results);
        }
        if self.picking {
            chain = chain.child(
                div()
                    .text_xs()
                    .text_color(rgb(ACCENT))
                    .child("파일 선택을 기다리고 있습니다."),
            );
        }
        if self.draft.effects.is_empty() {
            chain =
                chain.child(div().py_3().text_sm().text_color(rgb(MUTED)).child(
                    "선택한 VST3가 없습니다. 내장 보정만 사용하거나 플러그인을 추가하세요.",
                ));
        }
        for (index, effect) in self.draft.effects.iter().enumerate() {
            let slot = index + 1;
            let bypass_label = if effect.bypassed { "우회 끄기" } else { "우회 켜기" };
            let name = effect
                .plugin
                .file_name()
                .unwrap_or(effect.plugin.as_os_str())
                .to_string_lossy()
                .into_owned();
            let class = effect
                .class_id
                .as_deref()
                .map(|id| format!("class {id}"))
                .unwrap_or_else(|| "class 미지정 · 시작 시 확인".into());
            let state = effect
                .state
                .as_ref()
                .map(|state| format!("저장 상태 {} bytes", state.len()))
                .unwrap_or_else(|| "저장 상태 없음".into());
            chain = chain.child(
                div()
                    .id(("sound-slot", index))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .pt_3()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .font_semibold()
                                    .child(format!("{slot:02}  {name}")),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(
                                        button("slot-up", "위로", false)
                                            .accessibility_label(format!("VST3 {slot} 위로"))
                                            .disabled(disabled || index == 0)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.move_slot(index, index.saturating_sub(1), cx)
                                            })),
                                    )
                                    .child(
                                        button("slot-down", "아래로", false)
                                            .accessibility_label(format!("VST3 {slot} 아래로"))
                                            .disabled(disabled || slot == self.draft.effects.len())
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.move_slot(index, index + 1, cx)
                                            })),
                                    )
                                    .child(
                                        button("slot-remove", "삭제", false)
                                            .accessibility_label(format!("VST3 {slot} 삭제"))
                                            .disabled(disabled)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.remove_slot(index, cx)
                                            })),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .whitespace_normal()
                            .child(effect.plugin.to_string_lossy().into_owned()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .whitespace_normal()
                            .child(format!("{class} · {state} · {}", if effect.bypassed { "우회" } else { "효과 적용" })),
                    )
                    .child(button(("slot-bypass", index), bypass_label, false)
                        .accessibility_label(format!("VST3 {slot} {bypass_label}"))
                        .disabled(disabled)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.is_busy() { return; }
                            if let Some(effect) = this.draft.effects.get_mut(index) {
                                effect.bypassed = !effect.bypassed;
                                cx.notify();
                            }
                        }))),
            );
        }
        chain.child(div().text_xs().text_color(rgb(MUTED))
            .child("추가·저장할 때 플러그인을 열지 않습니다. 실제 로딩과 호환성 확인은 오디오 시작 때 수행합니다. AUX / MR은 이 체인을 거치지 않습니다."))
    }
}

impl Render for SoundEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().flex().flex_col().flex_shrink_0().gap_5()
            .child(panel().gap_4()
                .child(section_title("FX", "내장 보컬 보정"))
                .child(div().max_w(px(440.)).child(select_control(
                    "sound-preset", "내장 보컬 보정", &self.preset, None, window, cx)))
                .child(div().flex().flex_wrap().gap_5()
                    .child(field("sound-trim", "입력 trim", "−36~18 dB", &self.input_gain, window, cx))
                    .child(field("sound-strength", "보정 비율", "0~1 · EQ/다이내믹스 blend", &self.strength, window, cx)))
                .child(div().text_xs().text_color(rgb(MUTED))
                    .child("저장한 값은 다음 오디오 시작부터 적용됩니다. 보정 비율은 음량 일치 기능이 아니며, 자동 makeup gain은 없습니다.")))
            .child(panel().gap_4()
                .child(section_title("EQ", "보컬 EQ 세부 설정"))
                .child(div().flex().flex_wrap().gap_5()
                    .child(field("eq-hpf-hz", "하이패스 주파수", "40~200 Hz · 정지 상태", &self.eq_hpf_hz, window, cx))
                    .child(field("eq-body-db", "저역 EQ 보정", "180 Hz shelf · −6~6 dB", &self.eq_body_db, window, cx))
                    .child(field("eq-presence-db", "존재감 EQ 보정", "2.8 kHz peak · −6~6 dB", &self.eq_presence_db, window, cx))
                    .child(field("eq-air-db", "고역 EQ 보정", "7.5 kHz shelf · −6~6 dB", &self.eq_air_db, window, cx)))
                .child(div().text_xs().text_color(rgb(MUTED))
                    .child("선택한 프리셋의 EQ에 저역·존재감·고역 gain을 더합니다. 하이패스는 절대 주파수이며 EQ 계수는 정지 상태에서 변경합니다. Off 프리셋에서는 적용하지 않습니다.")))
            .child(panel().gap_4()
                .child(section_title("DYN", "컴프레서 · 디에서"))
                .child(div().flex().flex_wrap().gap_5()
                    .child(field("sound-compressor", "컴프레서 threshold", "−60~0 dBFS", &self.compressor, window, cx))
                    .child(field("compressor-ratio", "컴프레서 ratio", "1~8 : 1", &self.compressor_ratio, window, cx))
                    .child(field("compressor-attack-ms", "컴프레서 attack", "1~50 ms", &self.compressor_attack_ms, window, cx))
                    .child(field("compressor-release-ms", "컴프레서 release", "20~500 ms", &self.compressor_release_ms, window, cx)))
                .child(div().flex().flex_wrap().gap_5()
                    .child(field("sound-deesser", "디에서 threshold", "−60~0 dBFS", &self.deesser, window, cx))
                    .child(field("deess-max-db", "디에서 최대 감쇠", "0~12 dB · 0은 감쇠 없음", &self.deess_max_db, window, cx)))
                .child(div().text_xs().text_color(rgb(MUTED))
                    .child("모든 다이내믹스 값은 재생 중에도 조절할 수 있습니다. 프리셋과 독립적인 명시적 값이며 자동 makeup gain은 없습니다.")))
            .child(panel().gap_4()
                .child(section_title("ROOM", "스테레오 리버브"))
                .child(div().flex().flex_wrap().gap_5()
                    .child(field("reverb-mix", "리버브 비율", "0~1 · 0은 끄기", &self.reverb_mix, window, cx))
                    .child(field("reverb-decay", "잔향 시간", "0.2~5초 · 저역 기준", &self.reverb_decay, window, cx))
                    .child(field("reverb-predelay", "추가 pre-delay", "0~80 ms · wet만", &self.reverb_predelay, window, cx)))
                .child(div().text_xs().text_color(rgb(MUTED))
                    .child("내장 EQ·다이내믹스 뒤, VST3 앞에 적용합니다. 보컬 프리셋과 별개이며 원음 경로는 지연하지 않습니다. 설정은 다음 시작부터 적용됩니다.")))
            .child(panel().gap_4()
                .child(section_title("ECHO", "스테레오 에코"))
                .child(div().flex().flex_wrap().gap_5()
                    .child(field("echo-mix", "에코 비율", "0~1 · 0은 끄기", &self.echo_mix, window, cx))
                    .child(field("echo-time", "에코 시간", "10~1000 ms · 정지 상태에서 변경", &self.echo_time, window, cx))
                    .child(field("echo-feedback", "에코 피드백", "0~0.85 · 0은 한 번 반복", &self.echo_feedback, window, cx)))
                .child(div().text_xs().text_color(rgb(MUTED))
                    .child("리버브 뒤, VST3 앞에 적용합니다. 원음 경로에 추가 지연을 넣지 않으며 에코 비율과 피드백은 실행 중에도 조절할 수 있습니다.")))
            .children(self.error.as_ref().map(|error| panel().id("sound-editor-error")
                .role(Role::Alert).aria_label(error.clone()).gap_2().border_color(rgb(DANGER))
                .child(div().font_semibold().text_color(rgb(DANGER)).child("플러그인 목록을 변경하지 못했습니다"))
                .child(div().text_sm().child(error.clone()))))
            .child(self.chain(window, cx))
    }
}

fn field(
    id: &'static str,
    label: &'static str,
    hint: &'static str,
    state: &Entity<InputState>,
    window: &Window,
    cx: &App,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(div().text_sm().font_semibold().child(label))
        .child(number_input(id, state, window, cx))
        .child(div().text_xs().text_color(rgb(MUTED)).child(hint))
}
