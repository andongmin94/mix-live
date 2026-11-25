//! A draft UI for one live VST3 instance. Native access belongs to Session operations.
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, InteractiveElement, IntoElement,
    ParentElement, Render, Styled, Subscription, Window, div, px, relative, rgb,
};
use gpui_deck::{SelectEvent, SelectItem, SelectState, Slider, SliderEvent, SliderState, SliderThumb, StyledExt as _};
use mixlive_audio_windows::plugin::LiveParameter;

use crate::{
    form::select_control,
    ui::{ACCENT, BORDER, MUTED, TEXT, button, panel},
};

pub enum ParameterEvent {
    Refresh,
    Apply { id: u32, normalized: f64 },
    Close,
}

pub struct ParameterEditor {
    pub index: usize,
    parameters: Vec<LiveParameter>,
    picker: Entity<SelectState>,
    slider: Option<Entity<SliderState>>,
    changed: bool,
    busy: bool,
    _picker_subscription: Subscription,
    slider_subscription: Option<Subscription>,
}

impl EventEmitter<ParameterEvent> for ParameterEditor {}

impl ParameterEditor {
    pub fn new(index: usize, parameters: Vec<LiveParameter>, selected_id: Option<u32>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let selected_id = selected_id.filter(|id| parameters.iter().any(|parameter| parameter.id == *id))
            .or_else(|| parameters.first().map(|parameter| parameter.id));
        let picker = cx.new(|cx| {
            let state = SelectState::new(parameters.iter().map(|parameter| {
                SelectItem::new(parameter.id.to_string(), parameter.name.clone())
            }), cx);
            if let Some(id) = selected_id { state.default_value(id.to_string()) } else { state }
        });
        let subscription = cx.subscribe_in(&picker, window, |this, _, event, window, cx| {
            let SelectEvent::Change(_) = event;
            this.install_slider(window, cx);
            cx.notify();
        });
        let mut editor = Self { index, parameters, picker, slider: None, changed: false, busy: false,
            _picker_subscription: subscription, slider_subscription: None };
        editor.install_slider(window, cx);
        editor
    }

    pub fn selected_id(&self, cx: &App) -> Option<u32> {
        self.picker.read(cx).selected_item().and_then(|item| item.value().parse().ok())
    }

    fn selected(&self, cx: &App) -> Option<&LiveParameter> {
        let id = self.selected_id(cx)?;
        self.parameters.iter().find(|parameter| parameter.id == id)
    }

    fn install_slider(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.changed = false;
        self.slider = None;
        self.slider_subscription = None;
        let Some(parameter) = self.selected(cx) else { return; };
        let value = parameter.normalized as f32;
        let step = if parameter.step_count == 0 { 0.001 } else { 1.0 / parameter.step_count as f32 };
        let disabled = parameter.read_only || self.busy;
        let slider = cx.new(|cx| {
            let mut state = SliderState::new(0.0..=1.0, step, [value], cx);
            state.set_disabled(disabled, window, cx);
            state
        });
        self.slider_subscription = Some(cx.subscribe(&slider, |this, _, event, cx| {
            let SliderEvent::Change(_) = event;
            this.changed = true;
            cx.notify();
        }));
        self.slider = Some(slider);
    }

    pub fn set_busy(&mut self, busy: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.busy = busy;
        self.picker.update(cx, |state, cx| state.set_disabled(busy, window, cx));
        if let Some(slider) = &self.slider {
            let disabled = busy || self.selected(cx).is_none_or(|parameter| parameter.read_only);
            slider.update(cx, |state, cx| state.set_disabled(disabled, window, cx));
        }
        cx.notify();
    }

    fn apply(&mut self, cx: &mut Context<Self>) {
        if self.busy || !self.changed { return; }
        let Some(parameter) = self.selected(cx).filter(|parameter| !parameter.read_only) else { return; };
        let Some(slider) = &self.slider else { return; };
        cx.emit(ParameterEvent::Apply { id: parameter.id, normalized: f64::from(slider.read(cx).values()[0]) });
    }
}

impl Render for ParameterEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut content = panel().gap_4()
            .child(div().flex().flex_wrap().items_center().justify_between().gap_3()
                .child(div().font_semibold().child(format!("VST3 {} · 파라미터", self.index + 1)))
                .child(div().flex().gap_2()
                    .child(button("refresh-vst3-parameters", "새로고침", false).disabled(self.busy)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(ParameterEvent::Refresh))))
                    .child(button("close-vst3-parameters", "파라미터 닫기", false).disabled(self.busy)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(ParameterEvent::Close))))));
        if self.parameters.is_empty() {
            return content.child(div().text_sm().text_color(rgb(MUTED))
                .child("이 플러그인이 공개한 파라미터가 없습니다. 원본 창을 사용해 주세요."));
        }
        content = content.child(div().max_w(px(560.)).flex().flex_col().gap_2()
            .child(div().text_xs().text_color(rgb(MUTED)).child(format!("파라미터 {}개 · 목록에서 이름을 입력해 찾을 수 있습니다.", self.parameters.len())))
            .child(select_control("vst3-parameter-picker", "VST3 파라미터 선택", &self.picker, None, window, cx)));
        if let (Some(parameter), Some(slider)) = (self.selected(cx), &self.slider) {
            let position = slider.read(cx).percentage(0);
            content = content.child(div().flex().flex_wrap().items_center().justify_between().gap_3()
                .child(div().flex().flex_col().gap_1()
                    .child(div().text_sm().child(format!("마지막 조회 값 · {}", parameter.display)))
                    .children((!parameter.unit.is_empty()).then(|| div().text_xs().text_color(rgb(MUTED))
                        .child(format!("단위 · {}", parameter.unit))))
                    .child(div().text_xs().text_color(rgb(MUTED)).child(if parameter.read_only {
                        "읽기 전용".to_owned()
                    } else if parameter.step_count == 0 {
                        "연속 조절".to_owned()
                    } else {
                        format!("{}개 선택 위치", u64::from(parameter.step_count) + 1)
                    })))
                .child(button("apply-vst3-parameter", "적용", true)
                    .disabled(self.busy || parameter.read_only || !self.changed)
                    .on_click(cx.listener(|this, _, _, cx| this.apply(cx)))));
            content = content.child(div().flex().items_center().gap_4()
                .child(Slider::new("vst3-parameter-value", slider).accessibility_label("VST3 파라미터 선택 위치")
                    .relative().flex_1().h(px(28.)).rounded(px(5.))
                    .child(div().absolute().left_0().right_0().top(relative(0.5)).mt(px(-2.))
                        .h(px(4.)).rounded_full().bg(rgb(BORDER)))
                    .child(div().absolute().left_0().top(relative(0.5)).mt(px(-2.))
                        .h(px(4.)).w(relative(position)).rounded_full().bg(rgb(ACCENT)))
                    .child(SliderThumb::new(slider, 0).accessibility_label("VST3 파라미터 선택 위치")
                        .absolute().left(relative(position)).top(relative(0.5)).ml(px(-6.)).mt(px(-6.))
                        .size(px(12.)).rounded_full().border_1().border_color(rgb(TEXT))
                        .focus_visible(|style| style.bg(rgb(ACCENT))).bg(rgb(TEXT))))
                .child(div().min_w(px(120.)).text_right().text_sm().child(format!("선택 위치 {:.1}%", position * 100.0))));
        }
        content.child(div().text_xs().text_color(rgb(MUTED))
            .child("선택 위치는 플러그인의 조절 범위 내 비율입니다. 적용을 누르면 선택한 값 하나만 전달합니다. 원본 창에서 바꾼 값은 새로고침으로 확인하며, 새로고침과 다른 파라미터 선택은 미적용 값을 버립니다. 정지 후 새 Scene 저장으로 변경값을 보존할 수 있습니다."))
    }
}
