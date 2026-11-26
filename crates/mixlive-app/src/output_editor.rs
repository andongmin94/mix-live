//! One explicit endpoint/channel edit for the currently active output mixes.
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, IntoElement,
    ParentElement, Render, Styled, Subscription, Window, div, rgb,
};
use gpui_deck::{InputState, SelectItem, SelectState, StyledExt as _};
use mixlive_audio_windows::{config::OutputBinding, control::RouteMix, devices::DeviceInfo};

use crate::{
    draft::output_bindings,
    form::{number_input, select_control, selected, text_input},
    ui::{BORDER, DANGER, MUTED, button, mix_name, panel},
};

pub enum OutputEvent {
    Apply(Vec<OutputBinding>),
    Refresh,
    Close,
}

struct OutputField {
    mix: RouteMix,
    output: Entity<SelectState>,
    channels: Entity<InputState>,
}

pub struct OutputEditor {
    current: Vec<OutputBinding>,
    devices: Vec<DeviceInfo>,
    fields: Vec<OutputField>,
    busy: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<OutputEvent> for OutputEditor {}

impl OutputEditor {
    pub fn new(current: Vec<OutputBinding>, devices: Vec<DeviceInfo>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut fields = Vec::with_capacity(current.len());
        let mut subscriptions = Vec::with_capacity(current.len() * 2);
        for binding in &current {
            let items = device_items(&devices, Some(&binding.output));
            let output = cx.new(|cx| SelectState::new(items, cx).default_value(&binding.output));
            let channels = text_input(&format!("실행 중 {} 출력 채널", mix_name(binding.mix)),
                &channels_text(&binding.channels), window, cx);
            subscriptions.push(cx.observe(&output, |_, _, cx| cx.notify()));
            subscriptions.push(cx.observe(&channels, |_, _, cx| cx.notify()));
            fields.push(OutputField { mix: binding.mix, output, channels });
        }
        Self { current, devices, fields, busy: false, _subscriptions: subscriptions }
    }

    fn bindings(&self, cx: &App) -> Result<Vec<OutputBinding>, String> {
        let drafts: Vec<_> = self.fields.iter().map(|field| {
            (field.mix, selected(&field.output, cx), field.channels.read(cx).value().to_string())
        }).collect();
        let enabled = std::array::from_fn(|index| self.current.iter().any(|binding| binding.mix.index() == index));
        output_bindings(&drafts, &self.devices, enabled)
    }

    /// Preserve every selected ID and channel draft when availability changes.
    pub fn set_devices(&mut self, devices: Vec<DeviceInfo>, window: &mut Window, cx: &mut Context<Self>) {
        self.devices = devices;
        for field in &self.fields {
            let selected = selected(&field.output, cx);
            let items = device_items(&self.devices, selected.as_deref());
            field.output.update(cx, |state, cx| state.set_items(items, window, cx));
        }
        cx.notify();
    }

    pub fn set_busy(&mut self, busy: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.busy = busy;
        for field in &self.fields {
            field.output.update(cx, |state, cx| state.set_disabled(busy, window, cx));
            field.channels.update(cx, |state, cx| state.set_disabled(busy, cx));
        }
        cx.notify();
    }

    /// The active mix set is fixed for the run. Updating the acknowledged labels
    /// leaves endpoint/channel drafts intact, including after a rejected request.
    pub fn set_current(&mut self, current: Vec<OutputBinding>, cx: &mut Context<Self>) {
        self.current = current;
        cx.notify();
    }

    fn apply(&mut self, cx: &mut Context<Self>) {
        if self.busy { return; }
        if let Ok(bindings) = self.bindings(cx)
            && bindings != self.current {
            cx.emit(OutputEvent::Apply(bindings));
        }
    }
}

fn device_items(devices: &[DeviceInfo], selected: Option<&str>) -> Vec<SelectItem> {
    let mut items = vec![SelectItem::new("", "출력 장치 선택")];
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

impl Render for OutputEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let draft = self.bindings(cx);
        let can_apply = draft.as_ref().is_ok_and(|bindings| bindings != &self.current);
        let mut content = panel().gap_4()
            .child(div().flex().flex_wrap().items_center().justify_between().gap_3()
                .child(div().font_semibold().child("실행 중 출력 설정"))
                .child(div().flex().flex_wrap().gap_2()
                    .child(button("refresh-live-outputs", "장치 목록 새로고침", false).disabled(self.busy)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(OutputEvent::Refresh))))
                    .child(button("apply-live-outputs", "출력 설정 적용", true).disabled(self.busy || !can_apply)
                        .on_click(cx.listener(|this, _, _, cx| this.apply(cx))))
                    .child(button("close-live-outputs", "출력 설정 닫기", false).disabled(self.busy)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(OutputEvent::Close))))))
            .child(div().text_xs().text_color(rgb(MUTED))
                .child("현재 사용 중인 출력의 장치와 채널을 함께 적용합니다. 같은 장치를 사용할 때는 서로 겹치지 않는 채널을 선택하세요."));
        for field in &self.fields {
            let (select_id, channels_id, label) = match field.mix {
                RouteMix::Output => ("live-monitor-output", "live-monitor-channels", "실행 중 모니터 출력 장치"),
                RouteMix::Send => ("live-send-output", "live-send-channels", "실행 중 통화 출력 장치"),
                RouteMix::Broadcast => ("live-broadcast-output", "live-broadcast-channels", "실행 중 방송 출력 장치"),
            };
            let current = self.current.iter().find(|binding| binding.mix == field.mix).expect("active output mix");
            let chosen = selected(&field.output, cx);
            let device = chosen.as_ref().and_then(|id| self.devices.iter().find(|device| device.id == *id));
            let mut row = div().flex().flex_col().gap_3().pt_4().border_t_1().border_color(rgb(BORDER))
                .child(div().font_semibold().child(mix_name(field.mix)))
                .child(div().text_xs().text_color(rgb(MUTED)).whitespace_normal().child(format!(
                    "현재 적용 · {} · 채널 {}", current.output, channels_text(&current.channels))))
                .child(select_control(select_id, label, &field.output, Some(&self.devices), window, cx))
                .children(device.and_then(|device| device.config.as_ref().ok()).map(|config| {
                    div().text_xs().text_color(rgb(MUTED)).child(format!(
                        "{}채널 · {} Hz · {}", config.channels, config.sample_rate, config.sample_format))
                }))
                .children(chosen.as_ref().map(|id| div().text_xs().text_color(rgb(MUTED)).whitespace_normal().child(format!("ID: {id}"))))
                .child(div().flex().items_center().gap_3()
                    .child(div().text_sm().child("사용 채널"))
                    .child(number_input(channels_id, &field.channels, window, cx)));
            if let Some(id) = chosen {
                if let Some(device) = device {
                    if let Err(error) = &device.config {
                        row = row.child(div().text_sm().text_color(rgb(DANGER)).child(error.clone()));
                    }
                } else {
                    row = row.child(div().text_sm().text_color(rgb(DANGER)).child(format!("현재 목록에 없는 장치입니다: {id}")));
                }
            }
            content = content.child(row);
        }
        if self.devices.is_empty() {
            content = content.child(div().text_sm().text_color(rgb(DANGER))
                .child("표시할 출력 장치가 없습니다. 장치를 연결한 뒤 목록을 새로고침하세요."));
        }
        if let Err(error) = draft {
            content = content.child(div().text_sm().text_color(rgb(DANGER)).child(error));
        }
        content
            .child(div().text_xs().text_color(rgb(MUTED))
                .child("채널 하나는 좌우를 합친 모노, 두 개는 입력한 순서의 스테레오입니다. 장치나 채널을 바꾸는 동안 해당 출력이 잠시 끊길 수 있습니다."))
            .child(div().text_xs().text_color(rgb(MUTED))
                .child("현재 믹서·소스·보컬 설정은 유지합니다. 정지 후 새 Scene 저장으로 적용한 출력 설정을 보존할 수 있습니다."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refreshed_output_items_retain_selected_missing_or_unsupported_id() {
        let unavailable = DeviceInfo { id: "chosen".into(), name: "Output".into(), config: Err("unavailable".into()) };
        let items = device_items(std::slice::from_ref(&unavailable), Some("chosen"));
        let chosen = items.iter().find(|item| item.value().as_ref() == "chosen").unwrap();
        assert!(!chosen.is_disabled());
        let items = device_items(&[], Some("chosen"));
        let chosen = items.iter().find(|item| item.value().as_ref() == "chosen").unwrap();
        assert!(!chosen.is_disabled());
        assert!(chosen.label().contains("현재 없는 장치"));
        let items = device_items(&[unavailable], None);
        assert!(items.iter().find(|item| item.value().as_ref() == "chosen").unwrap().is_disabled());
    }
}
