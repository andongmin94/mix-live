//! Startup vocal settings and an ordered VST3 chain. This view never loads native plugins.

use gpui::{
    App, AppContext as _, Context, Div, Entity, InteractiveElement, IntoElement, ParentElement,
    PathPromptOptions, Render, Role, StatefulInteractiveElement, Styled, Subscription, Window, div,
    px, rgb,
};
use gpui_design::{InputState, SelectItem, SelectState, StyledExt as _};

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

pub struct SoundEditor {
    draft: SoundDraft,
    preset: Entity<SelectState>,
    input_gain: Entity<InputState>,
    strength: Entity<InputState>,
    compressor: Entity<InputState>,
    deesser: Entity<InputState>,
    busy: bool,
    picking: bool,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl SoundEditor {
    pub fn new(draft: SoundDraft, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let preset = cx.new(|cx| {
            SelectState::new(
                [
                    SelectItem::new("off", "Off · 내장 보정 끄기"),
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
        let mut subscriptions = vec![cx.observe(&preset, |_, _, cx| cx.notify())];
        for input in [&input_gain, &strength, &compressor, &deesser] {
            subscriptions.push(cx.observe(input, |_, _, cx| cx.notify()));
        }
        Self {
            draft,
            preset,
            input_gain,
            strength,
            compressor,
            deesser,
            busy: false,
            picking: false,
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
            effects: self.draft.effects.clone(),
        }
    }

    pub fn is_busy(&self) -> bool {
        self.busy || self.picking
    }

    pub fn set_busy(&mut self, busy: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.busy = busy;
        self.update_disabled(window, cx);
    }

    fn update_disabled(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let disabled = self.is_busy();
        self.preset
            .update(cx, |state, cx| state.set_disabled(disabled, window, cx));
        for input in [
            &self.input_gain,
            &self.strength,
            &self.compressor,
            &self.deesser,
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

    fn chain(&self, cx: &Context<Self>) -> Div {
        let disabled = self.is_busy();
        let mut chain = panel().gap_4()
            .child(div().flex().items_center().justify_between().gap_3()
                .child(section_title("VST3", "보컬 플러그인 순서"))
                .child(div().text_xs().text_color(rgb(MUTED))
                    .child(format!("{}개", self.draft.effects.len()))))
            .child(div().flex().flex_wrap().gap_2()
                .child(button("add-vst3-files", "VST3 파일 추가", false).disabled(disabled)
                    .on_click(cx.listener(|this, _, window, cx| this.pick(Pick::Plugins, window, cx))))
                .child(button("add-vst3-bundle", "VST3 폴더 추가", false).disabled(disabled)
                    .on_click(cx.listener(|this, _, window, cx| this.pick(Pick::Bundle, window, cx))))
                .child(button("add-chain-file", "체인 파일 추가", false).disabled(disabled)
                    .on_click(cx.listener(|this, _, window, cx| this.pick(Pick::Chain, window, cx)))))
            .child(div().text_xs().text_color(rgb(MUTED))
                .child("위에서 아래로 처리합니다. 체인 파일은 현재 목록 뒤에 추가되며, 같은 플러그인도 별도 슬롯입니다."));
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
                            .child(format!("{class} · {state}")),
                    ),
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
                    .child(field("sound-strength", "보정 비율", "0~1 · 원음/보정 blend", &self.strength, window, cx))
                    .child(field("sound-compressor", "컴프레서 threshold", "−60~0 dBFS", &self.compressor, window, cx))
                    .child(field("sound-deesser", "디에서 threshold", "−60~0 dBFS", &self.deesser, window, cx)))
                .child(div().text_xs().text_color(rgb(MUTED))
                    .child("저장한 값은 다음 오디오 시작부터 적용됩니다. 보정 비율은 음량 일치 기능이 아니며, 자동 makeup gain은 없습니다.")))
            .children(self.error.as_ref().map(|error| panel().id("sound-editor-error")
                .role(Role::Alert).aria_label(error.clone()).gap_2().border_color(rgb(DANGER))
                .child(div().font_semibold().text_color(rgb(DANGER)).child("플러그인 목록을 변경하지 못했습니다"))
                .child(div().text_sm().child(error.clone()))))
            .child(self.chain(cx))
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
