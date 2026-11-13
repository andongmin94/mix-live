//! New Scene creation and stopped Scene vocal editing. File operations never start audio.

use gpui::{
    App, AppContext as _, Context, Div, ElementId, Entity, EventEmitter, Focusable as _,
    InteractiveElement, IntoElement, ParentElement, Render, Role, StatefulInteractiveElement,
    Styled, Subscription, Window, div, prelude::FluentBuilder as _, px, rgb,
};
use gpui_design::{
    Input, InputAccessibility, InputBase, InputEditorStyle, InputState, Select, SelectEvent,
    SelectItem, SelectOption, SelectState, StyledExt as _,
};
use mixlive_audio_windows::{
    config::RouteOptions,
    devices::{DeviceCatalog, DeviceInfo},
    scene::AudioScene,
};

use crate::{
    draft::SceneDraft,
    sound::SoundDraft,
    sound_editor::SoundEditor,
    ui::{
        ACCENT, BACKGROUND, BORDER, DANGER, MUTED, RAISED, SURFACE, TEXT, button, panel,
        section_title,
    },
};

pub enum FormEvent {
    Save,
    Cancel,
}

struct EndpointField {
    select: Entity<SelectState>,
    channels: Entity<InputState>,
}

#[derive(Clone, Copy)]
enum Endpoint {
    Mic,
    Output,
    Send,
    Aux,
}

struct DeviceSetup {
    catalog: DeviceCatalog,
    mic: EndpointField,
    output: EndpointField,
    send: EndpointField,
    aux: EndpointField,
    aux_to: Entity<SelectState>,
}

enum Setup {
    New(DeviceSetup),
    Sound(RouteOptions),
}

pub struct SceneForm {
    setup: Setup,
    sound: Entity<SoundEditor>,
    busy: bool,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<FormEvent> for SceneForm {}

impl SceneForm {
    pub fn new(catalog: DeviceCatalog, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mic = endpoint_field(&catalog.inputs, false, "마이크 채널", "1", window, cx);
        let output = endpoint_field(&catalog.outputs, false, "주 출력 채널", "1,2", window, cx);
        let send = endpoint_field(&catalog.outputs, true, "추가 송출 채널", "1,2", window, cx);
        let aux = endpoint_field(&catalog.inputs, true, "AUX 채널", "1,2", window, cx);
        let aux_to = cx.new(|cx| {
            let mut state = SelectState::new(
                [
                    SelectItem::new("send", "추가 송출에만"),
                    SelectItem::new("output", "주 출력에만"),
                    SelectItem::new("both", "두 출력 모두"),
                ],
                cx,
            )
            .default_value("send");
            state.set_disabled(true, window, cx);
            state
        });
        let sound = cx.new(|cx| SoundEditor::new(SoundDraft::default(), window, cx));
        let mut subscriptions = vec![cx.observe(&sound, |_, _, cx| cx.notify())];
        for (endpoint, field) in [
            (Endpoint::Mic, &mic),
            (Endpoint::Output, &output),
            (Endpoint::Send, &send),
            (Endpoint::Aux, &aux),
        ] {
            subscriptions.push(cx.observe(&field.select, |_, _, cx| cx.notify()));
            subscriptions.push(cx.observe(&field.channels, |_, _, cx| cx.notify()));
            subscriptions.push(cx.subscribe_in(
                &field.select,
                window,
                move |this, _, event, window, cx| {
                    if this.is_busy(cx) {
                        return;
                    }
                    let Setup::New(setup) = &this.setup else {
                        return;
                    };
                    let SelectEvent::Change(id) = event;
                    let (devices, field) = setup.endpoint(endpoint);
                    if let Some(config) = devices
                        .iter()
                        .find(|device| device.id == id.as_ref())
                        .and_then(|device| device.config.as_ref().ok())
                    {
                        let channels = if matches!(endpoint, Endpoint::Mic) || config.channels == 1
                        {
                            "1"
                        } else {
                            "1,2"
                        };
                        field
                            .channels
                            .update(cx, |input, cx| input.set_value(channels, window, cx));
                    }
                    field
                        .channels
                        .update(cx, |input, cx| input.set_disabled(id.is_empty(), cx));
                    if matches!(endpoint, Endpoint::Aux) {
                        setup.aux_to.update(cx, |state, cx| {
                            state.set_disabled(id.is_empty(), window, cx)
                        });
                    }
                    this.error = None;
                    cx.notify();
                },
            ));
        }
        subscriptions.push(cx.observe(&aux_to, |_, _, cx| cx.notify()));
        Self {
            setup: Setup::New(DeviceSetup {
                catalog,
                mic,
                output,
                send,
                aux,
                aux_to,
            }),
            sound,
            busy: false,
            error: None,
            _subscriptions: subscriptions,
        }
    }

    /// Edit a saved startup chain without querying devices or native plugin modules.
    pub fn edit(scene: &AudioScene, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let sound = cx.new(|cx| SoundEditor::new(SoundDraft::from_scene(scene), window, cx));
        let subscription = cx.observe(&sound, |_, _, cx| cx.notify());
        Self {
            setup: Setup::Sound(scene.route.clone()),
            sound,
            busy: false,
            error: None,
            _subscriptions: vec![subscription],
        }
    }

    pub fn is_busy(&self, cx: &App) -> bool {
        self.busy || self.sound.read(cx).is_busy()
    }

    pub fn build_scene(&self, cx: &App) -> Result<AudioScene, String> {
        if self.is_busy(cx) {
            return Err("파일 선택을 마친 뒤 저장하세요.".into());
        }
        let sound = self.sound.read(cx).draft(cx);
        match &self.setup {
            Setup::New(setup) => setup.build_scene(sound, cx),
            Setup::Sound(route) => sound.apply_to(route),
        }
    }

    pub fn set_busy(&mut self, busy: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.busy = busy;
        if let Setup::New(setup) = &self.setup {
            setup.set_busy(busy, window, cx);
        }
        self.sound
            .update(cx, |sound, cx| sound.set_busy(busy, window, cx));
        cx.notify();
    }

    pub fn show_error(&mut self, error: Option<String>, cx: &mut Context<Self>) {
        self.error = error;
        cx.notify();
    }
}

impl DeviceSetup {
    fn endpoint(&self, endpoint: Endpoint) -> (&[DeviceInfo], &EndpointField) {
        match endpoint {
            Endpoint::Mic => (&self.catalog.inputs, &self.mic),
            Endpoint::Aux => (&self.catalog.inputs, &self.aux),
            Endpoint::Output => (&self.catalog.outputs, &self.output),
            Endpoint::Send => (&self.catalog.outputs, &self.send),
        }
    }

    fn build_scene(&self, sound: SoundDraft, cx: &App) -> Result<AudioScene, String> {
        SceneDraft {
            input: selected(&self.mic.select, cx),
            input_channel: self.mic.channels.read(cx).value().to_string(),
            output: selected(&self.output.select, cx),
            output_channels: self.output.channels.read(cx).value().to_string(),
            send: selected(&self.send.select, cx),
            send_channels: self.send.channels.read(cx).value().to_string(),
            aux_input: selected(&self.aux.select, cx),
            aux_channels: self.aux.channels.read(cx).value().to_string(),
            aux_to: selected(&self.aux_to, cx).unwrap_or_default(),
            sound,
        }
        .build(&self.catalog)
    }

    fn set_busy(&self, busy: bool, window: &mut Window, cx: &mut App) {
        for field in [&self.mic, &self.output, &self.send, &self.aux] {
            field
                .select
                .update(cx, |state, cx| state.set_disabled(busy, window, cx));
            let disabled = busy || selected(&field.select, cx).is_none();
            field
                .channels
                .update(cx, |input, cx| input.set_disabled(disabled, cx));
        }
        let disabled = busy || selected(&self.aux.select, cx).is_none();
        self.aux_to
            .update(cx, |state, cx| state.set_disabled(disabled, window, cx));
    }

    fn render(&self, window: &Window, cx: &App) -> Div {
        let mut sources = panel()
            .flex_1()
            .min_w_0()
            .gap_5()
            .child(section_title("01", "소스 선택"))
            .child(self.device_field(Endpoint::Mic, "new-mic", "내 목소리 · 필수", window, cx))
            .child(self.device_field(Endpoint::Aux, "new-aux", "AUX / MR · 선택", window, cx));
        if selected(&self.aux.select, cx).is_some() {
            sources = sources.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child("AUX를 보낼 곳"),
                    )
                    .child(select_control(
                        "new-aux-to",
                        "AUX를 보낼 곳",
                        &self.aux_to,
                        None,
                        window,
                        cx,
                    )),
            );
        }
        sources = sources.child(
            div()
                .text_xs()
                .text_color(rgb(MUTED))
                .child("AUX는 녹음 장치의 소리를 받습니다. 보컬 보정은 거치지 않습니다."),
        );
        let destinations = panel()
            .flex_1()
            .min_w_0()
            .gap_5()
            .child(section_title("02", "목적지 선택"))
            .child(self.device_field(Endpoint::Output, "new-output", "주 출력 · 필수", window, cx))
            .child(self.device_field(Endpoint::Send, "new-send", "추가 송출 · 선택", window, cx))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child("헤드폰이나 설치된 가상 케이블의 재생 장치를 선택하세요."),
            );
        div()
            .flex()
            .items_start()
            .gap_5()
            .child(sources)
            .child(destinations)
    }

    fn device_field(
        &self,
        endpoint: Endpoint,
        id: &'static str,
        label: &'static str,
        window: &Window,
        cx: &App,
    ) -> Div {
        let (devices, field) = self.endpoint(endpoint);
        let chosen = selected(&field.select, cx)
            .and_then(|id| devices.iter().find(|device| device.id == id));
        let mut content = div()
            .flex()
            .flex_col()
            .gap_2()
            .min_w_0()
            .child(div().font_semibold().child(label))
            .child(select_control(
                id,
                label,
                &field.select,
                Some(devices),
                window,
                cx,
            ));
        if devices.is_empty() {
            content = content.child(
                div()
                    .text_xs()
                    .text_color(rgb(DANGER))
                    .child("표시할 장치가 없습니다. 연결 후 새 Scene을 다시 여세요."),
            );
        } else if devices.iter().all(|device| device.config.is_err()) {
            // SelectState cannot open a list with no enabled items. Keep the
            // actual failure reasons visible even when nothing is selectable.
            for device in devices {
                if let Err(error) = &device.config {
                    content = content.child(
                        div()
                            .text_xs()
                            .text_color(rgb(DANGER))
                            .child(format!("{}: {error}", device.name)),
                    );
                }
            }
        }
        if let Some(device) = chosen {
            let config = device
                .config
                .as_ref()
                .expect("unsupported options are disabled");
            content = content
                .child(div().text_xs().text_color(rgb(MUTED)).child(format!(
                    "{}채널 · {} Hz · {}",
                    config.channels, config.sample_rate, config.sample_format
                )))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .whitespace_normal()
                        .child(format!("ID: {}", device.id)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(MUTED))
                                .w(px(76.))
                                .child("사용 채널"),
                        )
                        .child(number_input(id, &field.channels, window, cx))
                        .child(div().text_xs().text_color(rgb(MUTED)).child(
                            if matches!(endpoint, Endpoint::Mic) {
                                format!("1~{} 중 하나", config.channels)
                            } else {
                                format!("1~{} · 하나 또는 좌,우", config.channels)
                            },
                        )),
                );
        }
        content
    }
}

impl Render for SceneForm {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.is_busy(cx);
        let (title, description) = match &self.setup {
            Setup::New(_) => (
                "새 Scene 만들기",
                "장치와 보컬 설정을 새 파일로 저장하세요. 오디오는 시작 버튼으로 켭니다.",
            ),
            Setup::Sound(_) => (
                "보컬 설정 편집",
                "저장된 시작 설정을 편집합니다. 장치·라우팅 설정을 유지해 새 Scene으로 저장합니다.",
            ),
        };
        let mut content = div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap_5()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().text_2xl().font_semibold().child(title))
                            .child(div().text_sm().text_color(rgb(MUTED)).child(description)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_shrink_0()
                            .gap_2()
                            .child(
                                button("cancel-new-scene", "취소", false)
                                    .accessibility_label("새 Scene 취소")
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if !this.is_busy(cx) {
                                            cx.emit(FormEvent::Cancel);
                                        }
                                    })),
                            )
                            .child(
                                button("save-new-scene", "새 파일로 저장", true)
                                    .accessibility_label("새 Scene 파일로 저장")
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if !this.is_busy(cx) {
                                            cx.emit(FormEvent::Save);
                                        }
                                    })),
                            ),
                    ),
            )
            .children(self.error.as_ref().map(|error| {
                panel()
                    .id("new-scene-error")
                    .role(Role::Alert)
                    .aria_label(error.clone())
                    .gap_2()
                    .border_color(rgb(DANGER))
                    .child(
                        div()
                            .font_semibold()
                            .text_color(rgb(DANGER))
                            .child("설정을 확인해 주세요"),
                    )
                    .child(div().text_sm().child(error.clone()))
            }));
        if let Setup::New(setup) = &self.setup {
            content = content.child(setup.render(window, cx))
                .child(div().text_xs().text_color(rgb(MUTED))
                    .child("시작 음량 −18 dB · AUX −12 dB. 채널은 이 WASAPI 장치가 노출한 번호입니다. ASIO·물리 단자·하드웨어 루프백 번호와의 대응은 장치에서 확인하세요."));
        }
        content.child(self.sound.clone())
    }
}

pub(crate) fn text_input(
    label: &str,
    value: &str,
    window: &mut Window,
    cx: &mut App,
) -> Entity<InputState> {
    cx.new(|cx| {
        let mut state = InputState::new(window, cx).default_value(value.to_owned());
        state.set_editor_style(InputEditorStyle {
            foreground: rgb(TEXT).into(),
            muted_foreground: rgb(MUTED).into(),
            background: rgb(BACKGROUND).into(),
            border: rgb(BORDER).into(),
            selection: rgb(RAISED).into(),
            caret: rgb(ACCENT).into(),
            ..Default::default()
        });
        state.set_accessibility(
            InputAccessibility::new(Some(Role::TextInput))
                .with_label(Some(label.to_owned().into())),
        );
        state
    })
}

fn endpoint_field(
    devices: &[DeviceInfo],
    optional: bool,
    label: &'static str,
    value: &'static str,
    window: &mut Window,
    cx: &mut Context<SceneForm>,
) -> EndpointField {
    let mut items = Vec::new();
    if optional {
        items.push(SelectItem::new("", "사용 안 함"));
    }
    items.extend(devices.iter().map(|device| {
        SelectItem::new(device.id.clone(), device.name.clone()).disabled(device.config.is_err())
    }));
    let select = cx.new(|cx| {
        let state = SelectState::new(items, cx);
        if optional {
            state.default_value("")
        } else {
            state
        }
    });
    let channels = text_input(label, value, window, cx);
    channels.update(cx, |state, cx| state.set_disabled(true, cx));
    EndpointField { select, channels }
}

pub(crate) fn selected(state: &Entity<SelectState>, cx: &App) -> Option<String> {
    state
        .read(cx)
        .selected_item()
        .map(|item| item.value().to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) fn number_input(
    id: &'static str,
    state: &Entity<InputState>,
    window: &Window,
    cx: &App,
) -> InputBase {
    InputBase::new((ElementId::from(id), "channels"))
        .role(None::<Role>)
        .semantic_owner::<Input>(state.entity_id())
        .focused(state.focus_handle(cx).is_focused(window))
        .disabled(!state.read(cx).is_editable())
        .h(px(34.))
        .w(px(82.))
        .px_2()
        .rounded(px(6.))
        .bg(rgb(BACKGROUND))
        .border_1()
        .border_color(rgb(BORDER))
        .styles(|styles| styles.focused(|style| style.border_color(rgb(ACCENT))))
        .child(Input::new(state))
}

pub(crate) fn select_control(
    id: &'static str,
    label: &'static str,
    state: &Entity<SelectState>,
    devices: Option<&[DeviceInfo]>,
    window: &Window,
    cx: &App,
) -> Select {
    let model = state.read(cx);
    let selected_label = model
        .selected_item()
        .map(|item| item.label().clone())
        .unwrap_or_else(|| "장치를 선택하세요".into());
    let options = model.items().iter().enumerate().map(|(index, item)| {
        let mut option = SelectOption::new(state, index)
            .px_3()
            .py_2()
            .rounded(px(6.))
            .flex()
            .flex_col()
            .gap_1()
            .min_w_0()
            .when(model.highlighted_index() == Some(index), |this| {
                this.bg(rgb(RAISED))
            })
            .when(item.is_disabled(), |this| this.opacity(0.6))
            .child(
                div()
                    .text_sm()
                    .whitespace_normal()
                    .child(item.label().clone()),
            );
        if let Some(device) = devices.and_then(|devices| {
            devices
                .iter()
                .find(|device| device.id == item.value().as_ref())
        }) {
            option = option.child(
                div()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .whitespace_normal()
                    .child(device.id.clone()),
            );
            if let Err(error) = &device.config {
                option = option.child(
                    div()
                        .text_xs()
                        .text_color(rgb(DANGER))
                        .whitespace_normal()
                        .child(error.clone()),
                );
            }
        }
        option
    });
    Select::new(id, state)
        .accessibility_label(label)
        .w_full()
        .min_w_0()
        .rounded(px(7.))
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(BACKGROUND))
        .focus_visible(|style| style.border_color(rgb(ACCENT)))
        .when(model.is_disabled(), |this| this.opacity(0.45))
        .trigger(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .px_3()
                .py_2()
                .child(div().flex_1().min_w_0().truncate().child(selected_label))
                .child(div().text_color(rgb(MUTED)).child("⌄")),
        )
        .content(
            div()
                .id((ElementId::from(id), "options"))
                .w(px(510.))
                .max_w(window.viewport_size().width - px(32.))
                .max_h(px(340.))
                .overflow_y_scroll()
                .p_1()
                .rounded(px(9.))
                .border_1()
                .border_color(rgb(BORDER))
                .bg(rgb(SURFACE))
                .text_color(rgb(TEXT))
                .children(options),
        )
}
