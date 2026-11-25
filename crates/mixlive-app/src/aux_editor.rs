//! Explicit live AUX endpoint/channel selection. Mixer targets stay separate.
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, IntoElement,
    ParentElement, Render, Styled, Subscription, Window, div, rgb,
};
use gpui_deck::{InputState, SelectItem, SelectState, StyledExt as _};
use mixlive_audio_windows::{config::AuxSource, devices::DeviceInfo};

use crate::{
    draft::auxiliary_source,
    form::{number_input, select_control, selected, text_input},
    ui::{DANGER, MUTED, button, panel},
};

pub enum AuxEvent {
    Apply(AuxSource),
    Remove,
    Refresh,
    Close,
}

pub struct AuxEditor {
    current: Option<AuxSource>,
    microphone: (String, u16),
    devices: Vec<DeviceInfo>,
    input: Entity<SelectState>,
    channels: Entity<InputState>,
    busy: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<AuxEvent> for AuxEditor {}

impl AuxEditor {
    pub fn new(current: Option<AuxSource>, microphone: (&str, u16), devices: Vec<DeviceInfo>,
        window: &mut Window, cx: &mut Context<Self>) -> Self {
        let id = current.as_ref().map(|source| source.input.as_str());
        let items = device_items(&devices, id);
        let input = cx.new(|cx| SelectState::new(items, cx).default_value(id.unwrap_or("")));
        let channel_text = current.as_ref().map_or_else(|| "1,2".into(), |source| channels_text(&source.channels));
        let channels = text_input("실행 중 AUX 채널", &channel_text, window, cx);
        let subscriptions = vec![
            cx.observe(&input, |_, _, cx| cx.notify()),
            cx.observe(&channels, |_, _, cx| cx.notify()),
        ];
        Self { current, microphone: (microphone.0.to_owned(), microphone.1), devices, input, channels,
            busy: false, _subscriptions: subscriptions }
    }

    fn source(&self, cx: &App) -> Result<AuxSource, String> {
        auxiliary_source(selected(&self.input, cx).as_deref(), self.channels.read(cx).value().as_ref(),
            &self.devices, (&self.microphone.0, self.microphone.1))
    }

    /// Refresh availability without substituting another ID or losing channel edits.
    pub fn set_devices(&mut self, devices: Vec<DeviceInfo>, window: &mut Window, cx: &mut Context<Self>) {
        let selected = selected(&self.input, cx);
        let items = device_items(&devices, selected.as_deref());
        self.devices = devices;
        self.input.update(cx, |state, cx| state.set_items(items, window, cx));
        cx.notify();
    }

    pub fn set_busy(&mut self, busy: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.busy = busy;
        self.input.update(cx, |state, cx| state.set_disabled(busy, window, cx));
        self.channels.update(cx, |state, cx| state.set_disabled(busy, cx));
        cx.notify();
    }

    /// Acknowledged bindings update the current label, not an unapplied draft.
    pub fn set_current(&mut self, current: Option<AuxSource>, cx: &mut Context<Self>) {
        self.current = current;
        cx.notify();
    }

    fn apply(&mut self, cx: &mut Context<Self>) {
        if self.busy { return; }
        if let Ok(source) = self.source(cx)
            && Some(&source) != self.current.as_ref() {
            cx.emit(AuxEvent::Apply(source));
        }
    }
}

fn device_items(devices: &[DeviceInfo], selected: Option<&str>) -> Vec<SelectItem> {
    let mut items = vec![SelectItem::new("", "AUX 입력 장치 선택")];
    items.extend(devices.iter().map(|device| {
        let label = if device.config.is_err() { format!("사용할 수 없는 장치 · {}", device.name) }
            else { device.name.clone() };
        SelectItem::new(device.id.clone(), label)
            .disabled(device.config.is_err() && selected != Some(device.id.as_str()))
    }));
    if let Some(id) = selected.filter(|id| !devices.iter().any(|device| device.id == *id)) {
        items.push(SelectItem::new(id.to_owned(), format!("현재 없는 장치 · {id}")));
    }
    items
}

fn channels_text(channels: &[u16]) -> String {
    channels.iter().map(u16::to_string).collect::<Vec<_>>().join(", ")
}

impl Render for AuxEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let draft = self.source(cx);
        let can_apply = draft.as_ref().is_ok_and(|source| Some(source) != self.current.as_ref());
        let chosen = selected(&self.input, cx);
        let device = chosen.as_ref().and_then(|id| self.devices.iter().find(|device| device.id == *id));
        let current = self.current.as_ref().map_or_else(|| "사용 안 함".into(), |source| {
            format!("{} · 채널 {}", source.input, channels_text(&source.channels))
        });
        let mut content = panel().gap_4()
            .child(div().flex().flex_wrap().items_center().justify_between().gap_3()
                .child(div().font_semibold().child("실행 중 AUX 설정"))
                .child(div().flex().flex_wrap().gap_2()
                    .child(button("refresh-live-aux", "장치 목록 새로고침", false).disabled(self.busy)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(AuxEvent::Refresh))))
                    .child(button("apply-live-aux", "AUX 적용", true).disabled(self.busy || !can_apply)
                        .on_click(cx.listener(|this, _, _, cx| this.apply(cx))))
                    .child(button("remove-live-aux", "AUX 삭제", false).disabled(self.busy || self.current.is_none())
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(AuxEvent::Remove))))
                    .child(button("close-live-aux", "AUX 설정 닫기", false).disabled(self.busy)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(AuxEvent::Close))))))
            .child(div().text_xs().text_color(rgb(MUTED)).whitespace_normal().child(format!("현재 적용 · {current}")))
            .child(select_control("live-aux-input", "실행 중 AUX 입력 장치", &self.input, Some(&self.devices), window, cx))
            .children(device.and_then(|device| device.config.as_ref().ok()).map(|config| {
                div().text_xs().text_color(rgb(MUTED)).child(format!(
                    "{}채널 · {} Hz · {}", config.channels, config.sample_rate, config.sample_format))
            }))
            .children(chosen.as_ref().map(|id| div().text_xs().text_color(rgb(MUTED)).whitespace_normal().child(format!("ID: {id}"))))
            .child(div().flex().flex_col().gap_2()
                .child(div().text_sm().font_semibold().child("사용 채널"))
                .child(number_input("live-aux-channels", &self.channels, window, cx))
                .child(div().text_xs().text_color(rgb(MUTED)).child("1 또는 1,2 · 채널 하나는 모노, 두 개는 입력한 순서의 스테레오입니다.")));
        if self.devices.is_empty() {
            content = content.child(div().text_sm().text_color(rgb(DANGER))
                .child("표시할 입력 장치가 없습니다. 장치를 연결한 뒤 목록을 새로고침하세요."));
        }
        if chosen.is_some() {
            if let Err(error) = draft {
                content = content.child(div().text_sm().text_color(rgb(DANGER)).child(error));
            }
        } else {
            content = content.child(div().text_xs().text_color(rgb(MUTED)).child("AUX를 추가하려면 입력 장치와 채널을 선택하세요."));
        }
        content
            .child(div().text_xs().text_color(rgb(MUTED))
                .child("마이크와 같은 장치를 사용하면 마이크 채널과 겹치지 않는 채널을 선택하세요. 장치나 채널을 바꾸는 동안 AUX 소리가 잠시 끊길 수 있습니다."))
            .child(div().text_xs().text_color(rgb(MUTED))
                .child("새 AUX는 −12 dB, 음소거 해제, 모든 출력 연결 끄기로 시작합니다. AUX를 교체하면 현재 trim·음소거·보내기 설정을 유지합니다. 정지 후 새 Scene 저장으로 보존할 수 있습니다."))
    }
}
