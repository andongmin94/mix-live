//! New Scene creation and stopped Scene editing. File operations never start audio.

use gpui::{
    App, AppContext as _, Context, Div, ElementId, Entity, EventEmitter, Focusable as _,
    InteractiveElement, IntoElement, ParentElement, Render, Role, StatefulInteractiveElement,
    Styled, Subscription, Window, div, prelude::FluentBuilder as _, px, rgb,
};
use gpui_deck::{
    Input, InputAccessibility, InputBase, InputEditorStyle, InputState, Select, SelectEvent,
    SelectItem, SelectOption, SelectState, StyledExt as _,
};
use mixlive_audio_windows::{
    config::{AppSource, AuxDestination, RouteOptions, MAX_APPS},
    control::{MixSettings, RouteMix},
    devices::{DeviceCatalog, DeviceInfo, resolve_apps},
    scene::AudioScene,
};

use crate::{
    draft::{SceneDraft, channels, number},
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
    Broadcast,
    Aux,
}

struct AppField {
    source: AppSource,
    channels: Entity<InputState>,
    trim: Entity<InputState>,
    output_gain: Entity<InputState>,
    send_gain: Entity<InputState>,
    broadcast_gain: Entity<InputState>,
}

impl AppField {
    fn new(source: AppSource, window: &mut Window, cx: &mut App) -> Self {
        let label = |field: &str| format!("{} {field}", source.path);
        let channels = text_input(&label("채널"), &source.channels.iter().map(u16::to_string).collect::<Vec<_>>().join(","), window, cx);
        let trim = text_input(&label("입력 trim"), &source.gain_db.to_string(), window, cx);
        let output_gain = text_input(&label("주 출력 음량"), &source.output_gain_db.to_string(), window, cx);
        let send_gain = text_input(&label("추가 송출 음량"), &source.send_gain_db.to_string(), window, cx);
        let broadcast_gain = text_input(&label("방송 송출 음량"), &source.broadcast_gain_db.to_string(), window, cx);
        Self { source, channels, trim, output_gain, send_gain, broadcast_gain }
    }

    fn build(&self, cx: &App) -> Result<AppSource, String> {
        let mut source = self.source.clone();
        source.channels = channels(self.channels.read(cx).value().as_ref())?;
        source.gain_db = number(self.trim.read(cx).value().as_ref(), "앱 입력 trim")?;
        source.output_gain_db = number(self.output_gain.read(cx).value().as_ref(), "앱 주 출력 음량")?;
        source.send_gain_db = number(self.send_gain.read(cx).value().as_ref(), "앱 추가 송출 음량")?;
        source.broadcast_gain_db = number(self.broadcast_gain.read(cx).value().as_ref(), "앱 방송 송출 음량")?;
        Ok(source)
    }

    fn set_busy(&self, busy: bool, has_send: bool, has_broadcast: bool, cx: &mut App) {
        for input in [&self.channels, &self.trim, &self.output_gain] {
            input.update(cx, |input, cx| input.set_disabled(busy, cx));
        }
        self.send_gain.update(cx, |input, cx| input.set_disabled(busy || !has_send, cx));
        self.broadcast_gain.update(cx, |input, cx| input.set_disabled(busy || !has_broadcast, cx));
    }
}

#[derive(Clone, Copy)]
enum AppAction { Remove, Mute, Route(RouteMix) }

struct DeviceSetup {
    catalog: DeviceCatalog,
    mic: EndpointField,
    output: EndpointField,
    send: EndpointField,
    broadcast: EndpointField,
    aux: EndpointField,
    app_picker: Entity<SelectState>,
    apps: Vec<AppField>,
    aux_to: Entity<SelectState>,
    output_gain: Entity<InputState>,
    monitor_gain: Entity<InputState>,
    send_gain: Entity<InputState>,
    broadcast_gain: Entity<InputState>,
    aux_gain: Entity<InputState>,
    queue_ms: Entity<InputState>,
    mix: MixSettings,
}

enum Setup {
    Devices {
        fields: Box<DeviceSetup>,
        editing: bool,
    },
    Sound(Box<RouteOptions>),
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
        Self::with_devices(catalog, SceneDraft::default(), false, window, cx)
    }

    pub fn edit_devices(
        scene: &AudioScene,
        catalog: DeviceCatalog,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_devices(catalog, SceneDraft::from_scene(scene), true, window, cx)
    }

    fn with_devices(
        catalog: DeviceCatalog,
        draft: SceneDraft,
        editing: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mic = endpoint_field(
            &catalog.inputs,
            false,
            "마이크 채널",
            draft.input.as_deref(),
            &draft.input_channel,
            window,
            cx,
        );
        let output = endpoint_field(
            &catalog.outputs,
            false,
            "주 출력 채널",
            draft.output.as_deref(),
            &draft.output_channels,
            window,
            cx,
        );
        let send = endpoint_field(
            &catalog.outputs,
            true,
            "추가 송출 채널",
            draft.send.as_deref(),
            &draft.send_channels,
            window,
            cx,
        );
        let broadcast = endpoint_field(
            &catalog.outputs, true, "방송 송출 채널",
            draft.broadcast.as_deref(), &draft.broadcast_channels, window, cx,
        );
        let aux = endpoint_field(
            &catalog.inputs,
            true,
            "AUX 채널",
            draft.aux_input.as_deref(),
            &draft.aux_channels,
            window,
            cx,
        );
        let app_picker = cx.new(|cx| {
            let mut items = vec![SelectItem::new("", "추가할 앱 선택")];
            for app in &catalog.apps {
                let label = match &app.target {
                    Ok(target) => format!("{} · PID {}", app.path, target.pid),
                    Err(error) => format!("{} · {error}", app.path),
                };
                items.push(SelectItem::new(app.path.clone(), label).disabled(app.target.is_err()));
            }
            SelectState::new(items, cx).default_value("")
        });
        let apps = draft.apps.into_iter().map(|source| AppField::new(source, window, cx)).collect();
        let aux_to = cx.new(|cx| {
            SelectState::new(
                [
                    SelectItem::new("send", "추가 송출에만"),
                    SelectItem::new("output", "주 출력에만"),
                    SelectItem::new("both", "모니터와 통화"),
                    SelectItem::new("broadcast", "방송 송출에만"),
                    SelectItem::new("output-broadcast", "모니터와 방송"),
                    SelectItem::new("send-broadcast", "통화와 방송"),
                    SelectItem::new("all", "세 출력 모두"),
                    SelectItem::new("off", "어느 출력에도 보내지 않음"),
                ],
                cx,
            )
            .default_value(&draft.aux_to)
        });
        let output_gain = text_input("전체 시작 음량", &draft.output_gain_db, window, cx);
        let monitor_gain = text_input("모니터 시작 음량", &draft.monitor_gain_db, window, cx);
        let send_gain = text_input("추가 송출 시작 음량", &draft.send_gain_db, window, cx);
        let broadcast_gain = text_input("방송 송출 시작 음량", &draft.broadcast_gain_db, window, cx);
        let aux_gain = text_input("AUX 입력 음량", &draft.aux_gain_db, window, cx);
        let queue_ms = text_input("장치별 queue 목표", &draft.queue_ms, window, cx);
        let sound = cx.new(|cx| SoundEditor::new(draft.sound, window, cx));
        let mut subscriptions = vec![cx.observe(&sound, |_, _, cx| cx.notify())];
        for (endpoint, field) in [
            (Endpoint::Mic, &mic),
            (Endpoint::Output, &output),
            (Endpoint::Send, &send),
            (Endpoint::Broadcast, &broadcast),
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
                    let Setup::Devices { fields: setup, .. } = &this.setup else {
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
                    setup.set_busy(false, window, cx);
                    this.error = None;
                    cx.notify();
                },
            ));
        }
        subscriptions.push(cx.observe(&app_picker, |_, _, cx| cx.notify()));
        subscriptions.push(cx.observe(&aux_to, |_, _, cx| cx.notify()));
        for input in [&output_gain, &monitor_gain, &send_gain, &broadcast_gain, &aux_gain, &queue_ms] {
            subscriptions.push(cx.observe(input, |_, _, cx| cx.notify()));
        }
        let fields = DeviceSetup {
            catalog,
            mic,
            output,
            send,
            broadcast,
            aux,
            app_picker,
            apps,
            aux_to,
            output_gain,
            monitor_gain,
            send_gain,
            broadcast_gain,
            aux_gain,
            queue_ms,
            mix: draft.mix,
        };
        fields.set_busy(false, window, cx);
        Self {
            setup: Setup::Devices {
                fields: Box::new(fields),
                editing,
            },
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
            setup: Setup::Sound(Box::new(scene.route.clone())),
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
            Setup::Devices { fields, .. } => fields.build_scene(sound, cx),
            Setup::Sound(route) => sound.apply_to(route),
        }
    }

    pub fn set_busy(&mut self, busy: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.busy = busy;
        if let Setup::Devices { fields, .. } = &self.setup {
            fields.set_busy(busy, window, cx);
        }
        self.sound
            .update(cx, |sound, cx| sound.set_busy(busy, window, cx));
        cx.notify();
    }

    fn add_app(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_busy(cx) { return; }
        let Setup::Devices { fields, .. } = &mut self.setup else { return; };
        let Some(path) = selected(&fields.app_picker, cx) else { return; };
        if fields.apps.len() >= MAX_APPS || fields.apps.iter().any(|app| app.source.path.eq_ignore_ascii_case(&path)) {
            return;
        }
        let source = AppSource::new(path);
        let mut sources: Vec<_> = fields.apps.iter().map(|app| app.source.clone()).collect();
        sources.push(source.clone());
        if let Err(error) = resolve_apps(&sources, &fields.catalog.apps) {
            self.error = Some(error);
        } else {
            let field = AppField::new(source, window, cx);
            field.set_busy(false, selected(&fields.send.select, cx).is_some(),
                selected(&fields.broadcast.select, cx).is_some(), cx);
            fields.apps.push(field);
            self.error = None;
        }
        cx.notify();
    }

    fn edit_app(&mut self, index: usize, action: AppAction, cx: &mut Context<Self>) {
        if self.is_busy(cx) { return; }
        let Setup::Devices { fields, .. } = &mut self.setup else { return; };
        let Some(app) = fields.apps.get_mut(index) else { return; };
        match action {
            AppAction::Remove => { fields.apps.remove(index); }
            AppAction::Mute => { app.source.muted = !app.source.muted; }
            AppAction::Route(mix) => {
                let mut enabled = std::array::from_fn(|index| app.source.to.includes(index));
                enabled[mix.index()] = !enabled[mix.index()];
                app.source.to = AuxDestination::from_enabled(enabled);
            }
        }
        self.error = None;
        cx.notify();
    }

    fn app_sources(&self, fields: &DeviceSetup, window: &Window, cx: &Context<Self>) -> Div {
        let busy = self.is_busy(cx);
        let has_send = selected(&fields.send.select, cx).is_some();
        let has_broadcast = selected(&fields.broadcast.select, cx).is_some();
        let can_add = selected(&fields.app_picker, cx).is_some_and(|path| {
            fields.catalog.apps.iter().any(|app| app.path == path && app.target.is_ok())
                && !fields.apps.iter().any(|app| app.source.path.eq_ignore_ascii_case(&path))
        });
        let mut rack = panel().gap_4()
            .child(section_title("APP", "앱 소스"))
            .child(div().text_xs().text_color(rgb(MUTED))
                .child(format!("실행 중인 앱 최대 {MAX_APPS}개 · 서로 다른 프로세스 트리만 선택 · 원래 재생 경로는 유지됩니다.")))
            .child(div().flex().items_center().gap_2()
                .child(div().flex_1().min_w_0().child(select_control("app-picker", "추가할 앱", &fields.app_picker, None, window, cx)))
                .child(button("add-app", "앱 추가", true)
                    .disabled(busy || !can_add || fields.apps.len() >= MAX_APPS)
                    .on_click(cx.listener(|this, _, window, cx| this.add_app(window, cx)))));
        for (index, app) in fields.apps.iter().enumerate() {
            let slot = index + 1;
            let source = &app.source;
            let output_on = source.to.includes(0);
            let send_on = source.to.includes(1);
            let broadcast_on = source.to.includes(2);
            let problem = match fields.catalog.apps.iter().find(|candidate| candidate.path == source.path) {
                Some(candidate) => candidate.target.as_ref().err().cloned(),
                None => Some("저장된 앱이 현재 실행 목록에 없습니다. 앱을 실행하고 설정을 다시 열거나 슬롯을 삭제하세요.".into()),
            };
            rack = rack.child(div().id(("app-source", index)).flex().flex_col().gap_3()
                .pt_3().border_t_1().border_color(rgb(BORDER))
                .child(div().flex().items_center().justify_between().gap_3()
                    .child(div().flex_1().min_w_0().font_semibold().whitespace_normal()
                        .child(format!("{slot:02}  {}", source.path)))
                    .child(button("app-remove", "삭제", false).accessibility_label(format!("앱 {slot} 삭제"))
                        .disabled(busy).on_click(cx.listener(move |this, _, _, cx| this.edit_app(index, AppAction::Remove, cx)))))
                .children(problem.map(|error| div().text_xs().text_color(rgb(DANGER)).child(error)))
                .child(div().flex().flex_wrap().gap_2()
                    .child(button("app-mute", if source.muted { "음소거 중" } else { "음소거 해제" }, source.muted)
                        .accessibility_label(format!("앱 {slot} 음소거")).disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| this.edit_app(index, AppAction::Mute, cx))))
                    .child(button("app-output", if output_on { "주 출력 연결됨" } else { "주 출력 꺼짐" }, output_on)
                        .accessibility_label(format!("앱 {slot} 주 출력")).disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| this.edit_app(index, AppAction::Route(RouteMix::Output), cx))))
                    .child(button("app-send", if send_on { "추가 송출 연결됨" } else { "추가 송출 꺼짐" }, send_on)
                        .accessibility_label(format!("앱 {slot} 추가 송출")).disabled(busy || (!has_send && !send_on))
                        .on_click(cx.listener(move |this, _, _, cx| this.edit_app(index, AppAction::Route(RouteMix::Send), cx))))
                    .child(button("app-broadcast", if broadcast_on { "방송 송출 연결됨" } else { "방송 송출 꺼짐" }, broadcast_on)
                        .accessibility_label(format!("앱 {slot} 방송 송출")).disabled(busy || (!has_broadcast && !broadcast_on))
                        .on_click(cx.listener(move |this, _, _, cx| this.edit_app(index, AppAction::Route(RouteMix::Broadcast), cx)))))
                .child(div().flex().flex_wrap().gap_4()
                    .children([("app-channels", "채널 1 또는 1,2", &app.channels),
                        ("app-trim", "입력 trim (dB)", &app.trim),
                        ("app-output-gain", "주 출력 (dB)", &app.output_gain),
                        ("app-send-gain", "통화 송출 (dB)", &app.send_gain),
                        ("app-broadcast-gain", "방송 송출 (dB)", &app.broadcast_gain)].into_iter().map(|(id, label, input)| {
                        div().flex().flex_col().gap_1().child(div().text_xs().text_color(rgb(MUTED)).child(label))
                            .child(number_input(id, input, window, cx))
                    }))));
        }
        rack.child(div().text_xs().text_color(rgb(MUTED))
            .child("새 앱은 모든 경로가 꺼져 있습니다. 통화 수신 앱을 통화 송출로 보내지 않도록 직접 지정하세요."))
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
            Endpoint::Broadcast => (&self.catalog.outputs, &self.broadcast),
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
            broadcast: selected(&self.broadcast.select, cx),
            broadcast_channels: self.broadcast.channels.read(cx).value().to_string(),
            aux_input: selected(&self.aux.select, cx),
            apps: self.apps.iter().map(|app| app.build(cx)).collect::<Result<Vec<_>, _>>()?,
            aux_channels: self.aux.channels.read(cx).value().to_string(),
            aux_to: selected(&self.aux_to, cx).unwrap_or_default(),
            output_gain_db: self.output_gain.read(cx).value().to_string(),
            monitor_gain_db: self.monitor_gain.read(cx).value().to_string(),
            send_gain_db: self.send_gain.read(cx).value().to_string(),
            broadcast_gain_db: self.broadcast_gain.read(cx).value().to_string(),
            aux_gain_db: self.aux_gain.read(cx).value().to_string(),
            queue_ms: self.queue_ms.read(cx).value().to_string(),
            mix: self.mix,
            sound,
        }
        .build(&self.catalog)
    }

    fn set_busy(&self, busy: bool, window: &mut Window, cx: &mut App) {
        for endpoint in [
            Endpoint::Mic,
            Endpoint::Output,
            Endpoint::Send,
            Endpoint::Broadcast,
            Endpoint::Aux,
        ] {
            let (devices, field) = self.endpoint(endpoint);
            field
                .select
                .update(cx, |state, cx| state.set_disabled(busy, window, cx));
            let available = selected(&field.select, cx).is_some_and(|id| {
                devices
                    .iter()
                    .any(|device| device.id == id && device.config.is_ok())
            });
            field
                .channels
                .update(cx, |input, cx| input.set_disabled(busy || !available, cx));
        }
        self.app_picker.update(cx, |state, cx| state.set_disabled(busy, window, cx));
        let has_send = selected(&self.send.select, cx).is_some();
        let has_broadcast = selected(&self.broadcast.select, cx).is_some();
        for app in &self.apps { app.set_busy(busy, has_send, has_broadcast, cx); }
        let aux_disabled = busy || selected(&self.aux.select, cx).is_none();
        self.aux_to
            .update(cx, |state, cx| state.set_disabled(aux_disabled, window, cx));
        self.aux_gain
            .update(cx, |input, cx| input.set_disabled(aux_disabled, cx));
        let send_disabled = busy || selected(&self.send.select, cx).is_none();
        self.send_gain
            .update(cx, |input, cx| input.set_disabled(send_disabled, cx));
        self.broadcast_gain.update(cx, |input, cx| input.set_disabled(busy || !has_broadcast, cx));
        for input in [&self.output_gain, &self.monitor_gain, &self.queue_ms] {
            input.update(cx, |input, cx| input.set_disabled(busy, cx));
        }
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
                .child("AUX는 선택한 녹음 장치의 소리를 받습니다. 앱 소스는 아래에서 별도로 추가합니다."),
        );
        let destinations = panel()
            .flex_1()
            .min_w_0()
            .gap_5()
            .child(section_title("02", "목적지 선택"))
            .child(self.device_field(Endpoint::Output, "new-output", "주 출력 · 필수", window, cx))
            .child(self.device_field(Endpoint::Send, "new-send", "추가 송출 · 선택", window, cx))
            .child(self.device_field(Endpoint::Broadcast, "new-broadcast", "방송 송출 · 선택", window, cx))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child("헤드폰이나 설치된 가상 케이블의 재생 장치를 선택하세요."),
            );
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(div().flex().items_start().gap_5().child(sources).child(destinations))
            .child(
                panel()
                    .gap_4()
                    .child(section_title("03", "시작 음량과 queue"))
                    .child(
                        div().flex().flex_wrap().gap_5()
                            .child(self.setting("initial-output-gain", "전체 시작 음량", &self.output_gain, "−96~0 dB · 모든 출력에 공통 적용", window, cx))
                            .child(self.setting("initial-monitor-gain", "모니터 시작 음량", &self.monitor_gain, "−96~0 dB · 주 출력 추가 감쇠", window, cx))
                            .child(self.setting("initial-send-gain", "추가 송출 시작 음량", &self.send_gain, "−96~0 dB · 통화 송출 추가 감쇠", window, cx))
                            .child(self.setting("initial-broadcast-gain", "방송 송출 시작 음량", &self.broadcast_gain, "−96~0 dB · 방송 송출 추가 감쇠", window, cx))
                            .child(self.setting("initial-aux-gain", "AUX 입력 음량", &self.aux_gain, "−96~0 dB · AUX를 섞기 전 감쇠", window, cx))
                            .child(self.setting("initial-queue", "장치별 queue 목표", &self.queue_ms, "10~100 ms · 하드웨어 버퍼나 실측 왕복 지연이 아닙니다", window, cx)),
                    )
                    .child(div().text_xs().text_color(rgb(MUTED))
                        .child("저장된 시작 설정입니다. 실행 중 바꾼 음소거·음량·경로를 가져오지 않습니다.")),
            )
    }

    fn setting(
        &self,
        id: &'static str,
        label: &'static str,
        input: &Entity<InputState>,
        description: &'static str,
        window: &Window,
        cx: &App,
    ) -> Div {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .w(px(225.))
            .child(div().text_sm().child(label))
            .child(number_input(id, input, window, cx))
            .child(div().text_xs().text_color(rgb(MUTED)).child(description))
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
        let chosen_id = selected(&field.select, cx);
        let chosen = chosen_id
            .as_ref()
            .and_then(|id| devices.iter().find(|device| device.id == *id));
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
                    .child("표시할 장치가 없습니다. 연결 후 장치 설정을 다시 여세요."),
            );
        } else if chosen.is_none() && devices.iter().all(|device| device.config.is_err()) {
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
        if let Some(device_id) = chosen_id {
            let config = chosen.and_then(|device| device.config.as_ref().ok());
            let detail = match chosen {
                Some(device) => match &device.config {
                    Ok(config) => div().text_xs().text_color(rgb(MUTED)).child(format!(
                        "{}채널 · {} Hz · {}",
                        config.channels, config.sample_rate, config.sample_format
                    )),
                    Err(error) => div().text_xs().text_color(rgb(DANGER)).child(error.clone()),
                },
                None => div().text_xs().text_color(rgb(DANGER))
                    .child("저장된 장치가 현재 목록에 없습니다. 장치를 연결해 설정을 다시 열거나 다른 장치를 선택하세요."),
            };
            content = content
                .child(detail)
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .whitespace_normal()
                        .child(format!("ID: {device_id}")),
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
                        .child(div().text_xs().text_color(rgb(MUTED)).child(match config {
                            Some(config) if matches!(endpoint, Endpoint::Mic) => {
                                format!("1~{} 중 하나", config.channels)
                            }
                            Some(config) => {
                                format!("1~{} · 하나 또는 좌,우", config.channels)
                            }
                            None => {
                                "사용 가능한 장치를 선택하면 채널을 바꿀 수 있습니다.".to_owned()
                            }
                        })),
                );
        }
        content
    }
}

impl Render for SceneForm {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.is_busy(cx);
        let (title, description) = match &self.setup {
            Setup::Devices { editing: false, .. } => (
                "새 Scene 만들기",
                "장치와 보컬 설정을 새 파일로 저장하세요. 오디오는 시작 버튼으로 켭니다.",
            ),
            Setup::Devices { editing: true, .. } => (
                "장치·출력 설정",
                "저장된 장치·채널·시작 음량을 편집합니다. 현재 장치를 확인하고 새 Scene으로 저장합니다.",
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
        if let Setup::Devices { fields, .. } = &self.setup {
            content = content.child(fields.render(window, cx))
                .child(self.app_sources(fields, window, cx))
                .child(div().text_xs().text_color(rgb(MUTED))
                    .child("채널은 이 WASAPI 장치가 노출한 번호입니다. ASIO·물리 단자·하드웨어 루프백 번호와의 대응은 장치에서 확인하세요."));
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
    saved_id: Option<&str>,
    value: &str,
    window: &mut Window,
    cx: &mut Context<SceneForm>,
) -> EndpointField {
    let mut items = Vec::new();
    if optional {
        items.push(SelectItem::new("", "사용 안 함"));
    }
    items.extend(devices.iter().map(|device| {
        let unavailable = device.config.is_err();
        let label = if unavailable {
            format!("사용할 수 없는 장치 · {}", device.name)
        } else {
            device.name.clone()
        };
        // Preserve an explicit saved binding even when it cannot currently run.
        // The real catalog remains unchanged; build_scene rejects this selection.
        SelectItem::new(device.id.clone(), label)
            .disabled(unavailable && saved_id != Some(device.id.as_str()))
    }));
    if let Some(id) = saved_id.filter(|id| !devices.iter().any(|device| device.id == *id)) {
        items.push(SelectItem::new(
            id.to_owned(),
            format!("현재 없는 장치 · {id}"),
        ));
    }
    let select = cx.new(|cx| {
        let state = SelectState::new(items, cx);
        if let Some(id) = saved_id {
            state.default_value(id)
        } else if optional {
            state.default_value("")
        } else {
            state
        }
    });
    let channels = text_input(label, value, window, cx);
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
