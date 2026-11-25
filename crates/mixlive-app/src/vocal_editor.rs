//! Draft editing of the preset and wet delay times; live fader values stay separate.
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, IntoElement,
    ParentElement, Render, Styled, Subscription, Window, div, px, rgb,
};
use gpui_deck::{InputState, SelectItem, SelectState, StyledExt as _};
use mixlive_effects::{Preset, VocalSettings};

use crate::{
    form::{number_input, select_control, selected, text_input},
    ui::{DANGER, MUTED, button, panel},
};

pub enum VocalEvent {
    Apply(VocalSettings),
    Reset,
    Close,
}

pub struct VocalEditor {
    current: VocalSettings,
    preset: Entity<SelectState>,
    predelay: Entity<InputState>,
    echo_time: Entity<InputState>,
    busy: bool,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<VocalEvent> for VocalEditor {}

impl VocalEditor {
    pub fn new(current: VocalSettings, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let preset = cx.new(|cx| {
            SelectState::new([
                SelectItem::new("off", "Off · EQ/다이내믹스 끄기"),
                SelectItem::new("natural", "Natural · 자연스럽게"),
                SelectItem::new("warm", "Warm · 따뜻하게"),
                SelectItem::new("bright", "Bright · 밝게"),
            ], cx).default_value(current.preset.to_string())
        });
        let predelay = text_input("실행 중 리버브 pre-delay", &current.reverb_predelay_ms.to_string(), window, cx);
        let echo_time = text_input("실행 중 에코 시간", &current.echo_time_ms.to_string(), window, cx);
        let subscriptions = vec![
            cx.observe(&preset, |_, _, cx| cx.notify()),
            cx.observe(&predelay, |_, _, cx| cx.notify()),
            cx.observe(&echo_time, |_, _, cx| cx.notify()),
        ];
        Self { current, preset, predelay, echo_time, busy: false, error: None, _subscriptions: subscriptions }
    }

    fn settings(&self, cx: &App) -> Result<VocalSettings, String> {
        parse_settings(
            &selected(&self.preset, cx).unwrap_or_default(),
            self.predelay.read(cx).value().as_ref(),
            self.echo_time.read(cx).value().as_ref(),
        )
    }

    pub fn set_busy(&mut self, busy: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.busy = busy;
        self.preset.update(cx, |state, cx| state.set_disabled(busy, window, cx));
        for input in [&self.predelay, &self.echo_time] {
            input.update(cx, |state, cx| state.set_disabled(busy, cx));
        }
        cx.notify();
    }

    /// Acknowledged settings can change without discarding an unapplied draft.
    pub fn set_current(&mut self, current: VocalSettings, cx: &mut Context<Self>) {
        self.current = current;
        cx.notify();
    }

    fn apply(&mut self, cx: &mut Context<Self>) {
        if self.busy { return; }
        match self.settings(cx) {
            Ok(settings) => {
                self.error = None;
                if settings != self.current { cx.emit(VocalEvent::Apply(settings)); }
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }
}

fn parse_settings(preset: &str, predelay: &str, echo_time: &str) -> Result<VocalSettings, String> {
    let settings = VocalSettings {
        preset: preset.trim().parse::<Preset>().map_err(|error| error.to_string())?,
        reverb_predelay_ms: predelay.trim().parse().map_err(|_| "리버브 pre-delay에 숫자를 입력하세요.")?,
        echo_time_ms: echo_time.trim().parse().map_err(|_| "에코 시간에 숫자를 입력하세요.")?,
    };
    settings.validate().map_err(|error| error.to_string())?;
    Ok(settings)
}

impl Render for VocalEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let unchanged = self.settings(cx).is_ok_and(|draft| draft == self.current);
        let mut content = panel().gap_4()
            .child(div().flex().flex_wrap().items_center().justify_between().gap_3()
                .child(div().font_semibold().child("실행 중 보컬 설정"))
                .child(div().flex().flex_wrap().gap_2()
                    .child(button("reset-live-vocal", "현재 설정으로 되돌리기", false).disabled(self.busy)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(VocalEvent::Reset))))
                    .child(button("apply-live-vocal", "보컬 설정 적용", true).disabled(self.busy || unchanged)
                        .on_click(cx.listener(|this, _, _, cx| this.apply(cx))))
                    .child(button("close-live-vocal", "보컬 설정 닫기", false).disabled(self.busy)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(VocalEvent::Close))))))
            .child(div().text_xs().text_color(rgb(MUTED)).child(format!(
                "현재 적용 · {} · pre-delay {:.1} ms · echo {:.1} ms",
                self.current.preset, self.current.reverb_predelay_ms, self.current.echo_time_ms)))
            .child(div().max_w(px(440.)).flex().flex_col().gap_2()
                .child(div().text_sm().font_semibold().child("보컬 프리셋"))
                .child(select_control("live-vocal-preset", "실행 중 보컬 프리셋", &self.preset, None, window, cx)))
            .child(div().flex().flex_wrap().gap_5()
                .child(div().flex().flex_col().gap_2()
                    .child(div().text_sm().font_semibold().child("리버브 pre-delay"))
                    .child(number_input("live-vocal-predelay", &self.predelay, window, cx))
                    .child(div().text_xs().text_color(rgb(MUTED)).child("0~80 ms · 효과음만")))
                .child(div().flex().flex_col().gap_2()
                    .child(div().text_sm().font_semibold().child("에코 시간"))
                    .child(number_input("live-vocal-echo-time", &self.echo_time, window, cx))
                    .child(div().text_xs().text_color(rgb(MUTED)).child("10~1000 ms · 효과음만"))));
        if let Some(error) = &self.error {
            content = content.child(div().text_sm().text_color(rgb(DANGER)).child(error.clone()));
        }
        content
            .child(div().text_xs().text_color(rgb(MUTED))
                .child("적용을 누르면 세 값을 함께 반영합니다. 시간을 바꾸는 동안 효과음의 높이나 질감이 달라질 수 있습니다. 원음은 추가로 지연하지 않습니다."))
            .child(div().text_xs().text_color(rgb(MUTED))
                .child("프리셋을 켜거나 끌 때 소리가 순간적으로 달라질 수 있습니다. 음량·EQ·효과 비율 등의 조절값은 유지하며, 정지 후 새 Scene 저장으로 적용한 설정을 보존합니다."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vocal_draft_validates_preset_and_finite_wet_times() {
        assert_eq!(parse_settings("warm", " 25.5 ", " 450 ").unwrap(),
            VocalSettings { preset: Preset::Warm, reverb_predelay_ms: 25.5, echo_time_ms: 450.0 });
        assert_eq!(parse_settings("off", "0", "10").unwrap().preset, Preset::Off);
        assert_eq!(parse_settings("bright", "80", "1000").unwrap().echo_time_ms, 1000.0);
        for value in ["", "abc", "NaN", "inf", "-0.1", "80.1"] {
            assert!(parse_settings("natural", value, "250").is_err());
        }
        for value in ["", "abc", "NaN", "inf", "9.9", "1000.1"] {
            assert!(parse_settings("natural", "15", value).is_err());
        }
        assert!(parse_settings("", "15", "250").is_err());
        assert!(parse_settings("other", "15", "250").is_err());
    }
}
