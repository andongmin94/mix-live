//! Explicit draft editing of one coherent live pitch configuration.
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, IntoElement,
    ParentElement, Render, Styled, Subscription, Window, div, px, rgb,
};
use gpui_deck::{InputState, SelectItem, SelectState, StyledExt as _};
use mixlive_audio_windows::config::{PitchConfig, PitchScale};

use crate::{
    form::{number_input, select_control, selected, text_input},
    ui::{DANGER, MUTED, button, panel},
};

pub enum PitchEvent {
    Apply(PitchConfig),
    Reset,
    Close,
}

pub struct PitchEditor {
    current: PitchConfig,
    key: Entity<SelectState>,
    scale: Entity<SelectState>,
    strength: Entity<InputState>,
    retune: Entity<InputState>,
    busy: bool,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PitchEvent> for PitchEditor {}

impl PitchEditor {
    pub fn new(current: PitchConfig, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let key = cx.new(|cx| {
            SelectState::new(PitchScale::NAMES.into_iter().enumerate()
                .map(|(key, name)| SelectItem::new(key.to_string(), name)), cx)
                .default_value(current.key.to_string())
        });
        let scale = cx.new(|cx| {
            SelectState::new([
                SelectItem::new("chromatic", "Chromatic · 반음계"),
                SelectItem::new("major", "Major · 장조"),
                SelectItem::new("minor", "Minor · 자연단음계"),
            ], cx).default_value(current.scale.name())
        });
        let strength = text_input("실행 중 음정 보정 강도", &current.strength.to_string(), window, cx);
        let retune = text_input("실행 중 음정 보정 속도", &current.retune_ms.to_string(), window, cx);
        let subscriptions = vec![
            cx.observe(&key, |_, _, cx| cx.notify()),
            cx.observe(&scale, |_, _, cx| cx.notify()),
            cx.observe(&strength, |_, _, cx| cx.notify()),
            cx.observe(&retune, |_, _, cx| cx.notify()),
        ];
        Self { current, key, scale, strength, retune, busy: false, error: None, _subscriptions: subscriptions }
    }

    fn config(&self, cx: &App) -> Result<PitchConfig, String> {
        parse_config(
            &selected(&self.key, cx).unwrap_or_default(),
            &selected(&self.scale, cx).unwrap_or_default(),
            self.strength.read(cx).value().as_ref(),
            self.retune.read(cx).value().as_ref(),
        )
    }

    pub fn set_busy(&mut self, busy: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.busy = busy;
        for select in [&self.key, &self.scale] {
            select.update(cx, |state, cx| state.set_disabled(busy, window, cx));
        }
        for input in [&self.strength, &self.retune] {
            input.update(cx, |state, cx| state.set_disabled(busy, cx));
        }
        cx.notify();
    }

    /// Update the acknowledged settings without replacing an unapplied draft.
    pub fn set_current(&mut self, current: PitchConfig, cx: &mut Context<Self>) {
        self.current = current;
        cx.notify();
    }

    fn apply(&mut self, cx: &mut Context<Self>) {
        if self.busy { return; }
        match self.config(cx) {
            Ok(config) => {
                self.error = None;
                if config != self.current { cx.emit(PitchEvent::Apply(config)); }
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }
}

fn parse_config(key: &str, scale: &str, strength: &str, retune: &str) -> Result<PitchConfig, String> {
    let config = PitchConfig {
        key: key.trim().parse().map_err(|_| "음정 기준 조성을 선택하세요.")?,
        scale: <PitchScale as clap::ValueEnum>::from_str(scale, false)
            .map_err(|_| "음정 기준 스케일을 선택하세요.")?,
        strength: strength.trim().parse().map_err(|_| "음정 보정 강도에 숫자를 입력하세요.")?,
        retune_ms: retune.trim().parse().map_err(|_| "음정 보정 속도에 숫자를 입력하세요.")?,
    };
    config.validate()?;
    Ok(config)
}

impl Render for PitchEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let unchanged = self.config(cx).is_ok_and(|draft| draft == self.current);
        let mut content = panel().gap_4()
            .child(div().flex().flex_wrap().items_center().justify_between().gap_3()
                .child(div().font_semibold().child("실행 중 음정 설정 · 실험적 보정"))
                .child(div().flex().gap_2()
                    .child(button("reset-live-pitch", "현재 설정으로 되돌리기", false).disabled(self.busy)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(PitchEvent::Reset))))
                    .child(button("apply-live-pitch", "음정 설정 적용", true).disabled(self.busy || unchanged)
                        .on_click(cx.listener(|this, _, _, cx| this.apply(cx))))
                    .child(button("close-live-pitch", "음정 설정 닫기", false).disabled(self.busy)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(PitchEvent::Close))))))
            .child(div().text_xs().text_color(rgb(MUTED)).child(format!(
                "현재 적용 · {} {} · 강도 {:.0}% · retune {:.1} ms",
                PitchScale::NAMES[usize::from(self.current.key)], self.current.scale.name(),
                self.current.strength * 100.0, self.current.retune_ms)))
            .child(div().flex().flex_wrap().gap_5()
                .child(div().flex_1().min_w(px(180.)).flex().flex_col().gap_2()
                    .child(div().text_sm().font_semibold().child("조성"))
                    .child(select_control("live-pitch-key", "실행 중 음정 기준 조성", &self.key, None, window, cx)))
                .child(div().flex_1().min_w(px(220.)).flex().flex_col().gap_2()
                    .child(div().text_sm().font_semibold().child("스케일"))
                    .child(select_control("live-pitch-scale", "실행 중 음정 기준 스케일", &self.scale, None, window, cx))))
            .child(div().flex().flex_wrap().gap_5()
                .child(div().flex().flex_col().gap_2()
                    .child(div().text_sm().font_semibold().child("음정 보정 강도"))
                    .child(number_input("live-pitch-strength", &self.strength, window, cx))
                    .child(div().text_xs().text_color(rgb(MUTED)).child("0~1 · 0은 보정 끄기")))
                .child(div().flex().flex_col().gap_2()
                    .child(div().text_sm().font_semibold().child("음정 보정 속도"))
                    .child(number_input("live-pitch-retune", &self.retune, window, cx))
                    .child(div().text_xs().text_color(rgb(MUTED)).child("5~200 ms · 목표값 smoothing"))));
        if let Some(error) = &self.error {
            content = content.child(div().text_sm().text_color(rgb(DANGER)).child(error.clone()));
        }
        content.child(div().text_xs().text_color(rgb(MUTED))
            .child("적용을 누르면 네 값을 함께 반영합니다. 보정을 켜면 알고리즘 지연이 추가되며 상세 상태에서 확인할 수 있습니다. 정지 후 새 Scene 저장으로 적용한 설정을 보존합니다."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pitch_draft_parses_one_config_and_rejects_invalid_numeric_edits() {
        assert_eq!(parse_config("2", "major", " 0.75 ", " 35.5 ").unwrap(),
            PitchConfig { key: 2, scale: PitchScale::Major, strength: 0.75, retune_ms: 35.5 });
        assert_eq!(parse_config("11", "minor", "0", "5").unwrap().strength, 0.0);
        assert_eq!(parse_config("0", "chromatic", "1", "200").unwrap().retune_ms, 200.0);
        for value in ["", "abc", "NaN", "inf", "-0.1", "1.01"] {
            assert!(parse_config("0", "major", value, "20").is_err());
        }
        for value in ["", "abc", "NaN", "inf", "4.9", "200.1"] {
            assert!(parse_config("0", "major", "0.5", value).is_err());
        }
        assert!(parse_config("12", "major", "0.5", "20").is_err());
        assert!(parse_config("", "major", "0.5", "20").is_err());
        assert!(parse_config("0", "", "0.5", "20").is_err());
    }
}
