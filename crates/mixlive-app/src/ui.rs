//! The native Scene runner. Native lifecycle calls run on GPUI's foreground
//! executor, outside entity borrows, so COM/plugin callbacks may re-enter GPUI.

use std::{
    path::PathBuf,
    time::Duration,
};

use gpui::{
    App, AppContext as _, Context, Div, ElementId, Entity, InteractiveElement, IntoElement,
    ParentElement, PathPromptOptions, Render, ScrollHandle, SharedString, Styled, Subscription,
    TitlebarOptions, Window, WindowBounds, WindowOptions, div, px, relative, rgb, size,
};
use gpui_deck::{
    Button, Root, ScrollArea, Slider, SliderEvent, SliderState, SliderThumb, StyledExt as _, Switch,
};
use mixlive_audio_windows::control::{GainTarget, LiveCommand, MuteTarget, RouteMix, RouteSource};
use mixlive_audio_windows::windows::device_catalog;
use mixlive_effects::VocalParameter;

use crate::{
    form::{FormEvent, SceneForm},
    session::Session,
};

pub(crate) const BACKGROUND: u32 = 0x0b1120;
pub(crate) const SURFACE: u32 = 0x121d30;
pub(crate) const RAISED: u32 = 0x19273c;
pub(crate) const BORDER: u32 = 0x2b3b52;
pub(crate) const TEXT: u32 = 0xe8eef8;
pub(crate) const MUTED: u32 = 0x9badc7;
pub(crate) const ACCENT: u32 = 0x73ddc3;
pub(crate) const DANGER: u32 = 0xff8997;

#[derive(Clone, Copy, PartialEq, Eq)]
enum FaderTarget {
    Gain(GainTarget),
    Route(RouteSource, RouteMix),
    Trim(RouteSource),
    Vocal(VocalParameter),
}

impl FaderTarget {
    fn command(self, value: f32) -> LiveCommand {
        match self {
            Self::Gain(target) => LiveCommand::Gain { target, db: value },
            Self::Route(source, mix) => LiveCommand::RouteGain { source, mix, db: value },
            Self::Trim(source) => LiveCommand::SourceTrim { source, db: value },
            Self::Vocal(parameter) => LiveCommand::Vocal { parameter, value },
        }
    }

    fn range(self) -> std::ops::RangeInclusive<f32> {
        match self {
            Self::Vocal(parameter) => parameter.range(),
            _ => -96.0..=0.0,
        }
    }

    fn step(self) -> f32 {
        match self {
            Self::Vocal(VocalParameter::Strength | VocalParameter::ReverbMix | VocalParameter::EchoMix | VocalParameter::EchoFeedback) => 0.01,
            _ => 0.1,
        }
    }

    fn value_label(self, value: f32) -> String {
        match self {
            Self::Vocal(VocalParameter::Strength | VocalParameter::ReverbMix | VocalParameter::EchoMix | VocalParameter::EchoFeedback) => format!("{:.1}%", value * 100.0),
            Self::Vocal(VocalParameter::ReverbDecay) => format!("{value:.2} s"),
            Self::Vocal(VocalParameter::Compressor | VocalParameter::Deesser) => format!("{value:.1} dBFS"),
            _ => format!("{value:.1} dB"),
        }
    }
}

struct Fader {
    target: FaderTarget,
    state: Entity<SliderState>,
}

enum Operation {
    Devices,
    EditDevices,
    Start,
    Stop,
    StopAndSave(PathBuf),
    Editor { index: usize, open: bool },
    Close,
}

pub struct MixLive {
    session: Session,
    busy: Option<&'static str>,
    picking: bool,
    details: bool,
    selected_name: String,
    faders: Vec<Fader>,
    subscriptions: Vec<Subscription>,
    form: Option<Entity<SceneForm>>,
    form_subscription: Option<Subscription>,
    workspace_scroll: ScrollHandle,
}

impl MixLive {
    fn new(scene: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut session = Session::default();
        if let Some(path) = scene {
            let _ = session.open(path);
        }
        let selected_name = scene_name(&session);

        let owner = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            owner
                .update(cx, |this, cx| {
                    if this.busy.is_some() {
                        return false;
                    }
                    if this.session.running() {
                        this.perform(Operation::Close, window, cx);
                        false
                    } else {
                        true
                    }
                })
                .unwrap_or(true)
        });

        // Poll on the loading/foreground thread, but release App and entity borrows
        // before native editor resize/detach can synchronously dispatch window messages.
        cx.spawn_in(window, async move |owner, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                let taken = cx.update(|_, cx| {
                    owner.update(cx, |this, _| {
                        if this.busy.is_some() || !this.session.running() {
                            return None;
                        }
                        this.busy = Some("플러그인 상태 확인 중");
                        Some(std::mem::take(&mut this.session))
                    })
                });
                let mut session = match taken {
                    Ok(Ok(Some(session))) => session,
                    Ok(Ok(None)) => continue,
                    _ => break,
                };
                let result = session.poll();
                let alive = cx.update(|window, cx| {
                    owner.update(cx, |this, cx| {
                        this.session = session;
                        this.busy = None;
                        match result {
                            Err(_) => this.perform(Operation::Stop, window, cx),
                            Ok(changed) => {
                                if changed {
                                    cx.notify();
                                }
                            }
                        }
                    }).is_ok()
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        })
        .detach();

        Self {
            session,
            busy: None,
            picking: false,
            details: false,
            selected_name,
            faders: Vec::new(),
            subscriptions: Vec::new(),
            form: None,
            form_subscription: None,
            workspace_scroll: ScrollHandle::new(),
        }
    }

    fn choose_scene(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() || self.picking || self.session.running() || self.form.is_some() {
            return;
        }
        self.picking = true;
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("MixLive Scene 열기".into()),
        });
        cx.notify();
        cx.spawn_in(window, async move |owner, cx| {
            let result = paths.await;
            let _ = cx.update(move |_, cx| {
                let _ = owner.update(cx, |this, cx| {
                    this.picking = false;
                    match result {
                        Ok(Ok(Some(paths))) => {
                            if let Some(path) = paths.into_iter().next() {
                                let _ = this.session.open(path);
                                this.selected_name = scene_name(&this.session);
                                this.workspace_scroll.set_offset(Default::default());
                            }
                        }
                        Ok(Ok(None)) => {}
                        Ok(Err(error)) => this.session.error = Some(error.to_string()),
                        Err(error) => this.session.error = Some(error.to_string()),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn install_form(&mut self, form: Entity<SceneForm>, window: &Window, cx: &mut Context<Self>) {
        self.form_subscription =
            Some(
                cx.subscribe_in(&form, window, |this, form, event, window, cx| match event {
                    FormEvent::Save => this.save_scene(window, cx),
                    FormEvent::Cancel if !this.picking && !form.read(cx).is_busy(cx) => {
                        this.form = None;
                        this.form_subscription = None;
                        this.workspace_scroll.set_offset(Default::default());
                        cx.notify();
                    }
                    FormEvent::Cancel => {}
                }),
            );
        self.form = Some(form);
        self.workspace_scroll.set_offset(Default::default());
        cx.notify();
    }

    fn edit_sound(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() || self.picking || self.session.running() || self.form.is_some() {
            return;
        }
        let Some(scene) = self.session.scene() else {
            return;
        };
        let form = cx.new(|cx| SceneForm::edit(scene, window, cx));
        self.session.error = None;
        self.install_form(form, window, cx);
    }

    fn save_scene(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.picking || self.busy.is_some() || self.session.running() {
            return;
        }
        let Some(form) = self.form.clone() else {
            return;
        };
        let scene = match form.read(cx).build_scene(cx) {
            Ok(scene) => scene,
            Err(error) => {
                form.update(cx, |form, cx| form.show_error(Some(error), cx));
                return;
            }
        };
        let directory = self
            .session
            .path()
            .and_then(|path| path.parent())
            .filter(|path| !path.as_os_str().is_empty())
            .map(PathBuf::from)
            .map(Ok)
            .unwrap_or_else(std::env::current_dir);
        let directory = match directory {
            Ok(directory) => directory,
            Err(error) => {
                form.update(cx, |form, cx| form.show_error(Some(error.to_string()), cx));
                return;
            }
        };
        self.picking = true;
        form.update(cx, |form, cx| {
            form.show_error(None, cx);
            form.set_busy(true, window, cx);
        });
        let path = cx.prompt_for_new_path(&directory, Some("새 Scene.mixlive-scene.json"));
        cx.notify();
        cx.spawn_in(window, async move |owner, cx| {
            let result = path.await;
            let _ = cx.update(move |window, cx| {
                let _ = owner.update(cx, |this, cx| {
                    this.picking = false;
                    form.update(cx, |form, cx| form.set_busy(false, window, cx));
                    let error = match result {
                        Ok(Ok(Some(path))) => match this.session.save_new(path, scene) {
                            Ok(()) => {
                                this.selected_name = scene_name(&this.session);
                                this.form = None;
                                this.form_subscription = None;
                                this.workspace_scroll.set_offset(Default::default());
                                None
                            }
                            Err(error) => Some(format!("새 파일로 저장하지 못했습니다: {error}")),
                        },
                        Ok(Ok(None)) => None,
                        Ok(Err(error)) => Some(error.to_string()),
                        Err(error) => Some(error.to_string()),
                    };
                    if let Some(error) = error {
                        form.update(cx, |form, cx| form.show_error(Some(error), cx));
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn choose_snapshot_path(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.picking || self.busy.is_some() || !self.session.running() {
            return;
        }
        let directory = self.session.path()
            .and_then(|path| path.parent())
            .filter(|path| !path.as_os_str().is_empty())
            .map(PathBuf::from)
            .map(Ok)
            .unwrap_or_else(std::env::current_dir);
        let directory = match directory {
            Ok(directory) => directory,
            Err(error) => {
                self.session.error = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        self.picking = true;
        let path = cx.prompt_for_new_path(&directory, Some("실행 상태.mixlive-scene.json"));
        cx.notify();
        cx.spawn_in(window, async move |owner, cx| {
            let result = path.await;
            let _ = cx.update(move |window, cx| {
                let _ = owner.update(cx, |this, cx| {
                    this.picking = false;
                    match result {
                        Ok(Ok(Some(path))) if this.session.running() && this.busy.is_none() => {
                            this.perform(Operation::StopAndSave(path), window, cx);
                        }
                        Ok(Ok(_)) => {}
                        Ok(Err(error)) => this.session.error = Some(error.to_string()),
                        Err(error) => this.session.error = Some(error.to_string()),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn perform(&mut self, operation: Operation, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        if matches!(
            operation,
            Operation::Start | Operation::Devices | Operation::EditDevices
        ) && (self.picking || self.form.is_some() || self.session.running())
        {
            return;
        }
        if matches!(operation, Operation::Start | Operation::EditDevices)
            && self.session.scene().is_none()
        {
            return;
        }
        let editor_only = matches!(operation, Operation::Editor { .. });
        if editor_only && (!self.session.running() || self.picking) {
            return;
        }
        self.busy = Some(match &operation {
            Operation::Devices | Operation::EditDevices => "장치 조회 중",
            Operation::Start => "시작 중",
            Operation::Stop | Operation::Close => "정지 중",
            Operation::StopAndSave(_) => "정지 및 저장 중",
            Operation::Editor { .. } => "VST3 원본 창 변경 중",
        });
        if !editor_only {
            self.faders.clear();
            self.subscriptions.clear();
        }
        let mut session = std::mem::take(&mut self.session);
        cx.notify();
        cx.spawn_in(window, async move |owner, cx| {
            // These synchronous native calls MUST be outside App/entity update.
            let catalog = match &operation {
                Operation::Devices | Operation::EditDevices => {
                    Some(device_catalog().map_err(|error| error.to_string()))
                }
                Operation::Start => {
                    let _ = session.start();
                    None
                }
                Operation::Editor { index, open } => {
                    let _ = session.set_editor_open(*index, *open);
                    None
                }
                Operation::StopAndSave(path) => {
                    let _ = session.stop_and_save(path.clone());
                    None
                }
                Operation::Stop | Operation::Close => {
                    let _ = session.stop();
                    None
                }
            };
            let _ = cx.update(move |window, cx| {
                let _ = owner.update(cx, |this, cx| {
                    this.session = session;
                    this.selected_name = scene_name(&this.session);
                    this.busy = None;
                    if let Some(catalog) = catalog {
                        match catalog {
                            Ok(catalog) => {
                                this.session.error = None;
                                let form = cx.new(|cx| {
                                    if matches!(operation, Operation::EditDevices) {
                                        SceneForm::edit_devices(
                                            this.session.scene().expect("selected Scene to edit"),
                                            catalog, window, cx,
                                        )
                                    } else {
                                        SceneForm::new(catalog, window, cx)
                                    }
                                });
                                this.install_form(form, window, cx);
                            }
                            Err(error) => {
                                this.session.message = "장치 목록을 읽지 못했습니다. 연결을 확인하고 장치 설정을 다시 여세요.".into();
                                this.session.error = Some(error);
                            }
                        }
                    }
                    if !editor_only {
                        this.install_faders(cx);
                    }
                    cx.notify();
                });
                if matches!(operation, Operation::Close) {
                    window.remove_window();
                }
            });
        })
        .detach();
    }

    fn install_faders(&mut self, cx: &mut Context<Self>) {
        self.faders.clear();
        self.subscriptions.clear();
        let Some(controls) = self.session.controls() else {
            return;
        };
        let mut values = vec![(
            FaderTarget::Gain(GainTarget::Master),
            controls.gain_db(GainTarget::Master).expect("master exists"),
        )];
        for mix in controls.mixes() {
            let target = mix.gain_target();
            values.push((FaderTarget::Gain(target), controls.gain_db(target).expect("configured mix")));
        }
        for source in controls.sources() {
            if source != RouteSource::Mic {
                values.push((FaderTarget::Trim(source), controls.source_trim_db(source).expect("configured source")));
            }
            for mix in controls.mixes() {
                let (_, db) = controls
                    .route_state(source, mix)
                    .expect("configured route exists");
                values.push((FaderTarget::Route(source, mix), db));
            }
        }
        for parameter in controls.vocal_parameters() {
            values.push((FaderTarget::Vocal(parameter), controls.vocal_value(parameter)));
        }
        for (target, value) in values {
            let state = cx.new(|cx| SliderState::new(target.range(), target.step(), [value], cx));
            self.subscriptions
                .push(cx.subscribe(&state, move |this, _, event, cx| {
                    let SliderEvent::Change(values) = event;
                    let _ = this.session.apply(target.command(values[0]));
                    cx.notify();
                }));
            self.faders.push(Fader { target, state });
        }
    }

    fn mute(
        &self,
        id: impl Into<ElementId>,
        label: &str,
        target: MuteTarget,
        cx: &Context<Self>,
    ) -> Switch {
        let checked = self
            .session
            .controls()
            .expect("live controls")
            .is_muted(target);
        let owner = cx.entity();
        toggle(id, label.to_owned(), checked, true).on_change(move |muted, _, _, cx| {
            owner.update(cx, |this, cx| {
                let _ = this.session.apply(LiveCommand::Mute { target, muted });
                cx.notify();
            });
        })
    }

    fn fader(
        &self,
        id: impl Into<ElementId>,
        label: &str,
        target: FaderTarget,
        value: f32,
        cx: &App,
    ) -> gpui::AnyElement {
        let state = &self
            .faders
            .iter()
            .find(|fader| fader.target == target)
            .expect("live fader installed")
            .state;
        let percentage = state.read(cx).percentage(0);
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap_4()
            .child(
                Slider::new(id, state)
                    .accessibility_label(label.to_owned())
                    .relative()
                    .flex_1()
                    .h(px(28.))
                    .rounded(px(5.))
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .top(relative(0.5))
                            .mt(px(-2.))
                            .h(px(4.))
                            .rounded_full()
                            .bg(rgb(BORDER)),
                    )
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top(relative(0.5))
                            .mt(px(-2.))
                            .h(px(4.))
                            .w(relative(percentage))
                            .rounded_full()
                            .bg(rgb(ACCENT)),
                    )
                    .child(
                        SliderThumb::new(state, 0)
                            .accessibility_label(label.to_owned())
                            .absolute()
                            .left(relative(percentage))
                            .top(relative(0.5))
                            .ml(px(-6.))
                            .mt(px(-6.))
                            .size(px(12.))
                            .rounded_full()
                            .border_1()
                            .border_color(rgb(TEXT))
                            .focus_visible(|style| style.bg(rgb(ACCENT)))
                            .bg(rgb(TEXT)),
                    ),
            )
            .child(
                div()
                    .w(px(70.))
                    .text_right()
                    .text_sm()
                    .child(target.value_label(value)),
            )
            .into_any_element()
    }

    fn header(&self, cx: &Context<Self>) -> Div {
        let running = self.session.running();
        let (status, color) = match self.busy {
            Some(status) => (status, MUTED),
            None if running => ("오디오 실행 중", ACCENT),
            None if self.form.is_some() => ("Scene 설정", MUTED),
            None if self.session.error.is_some() => ("확인 필요", DANGER),
            None if self.session.scene().is_some() => ("시작 준비", MUTED),
            None => ("정지", MUTED),
        };
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_5()
            .flex_shrink_0()
            .px_7()
            .py_5()
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .size(px(40.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(11.))
                            .bg(rgb(ACCENT))
                            .text_color(rgb(BACKGROUND))
                            .text_xl()
                            .font_bold()
                            .child("M"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().text_xl().font_semibold().child("MixLive"))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(MUTED))
                                    .child("내 소리를 만들고, 보낼 곳을 정하세요."),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .mr_2()
                            .text_sm()
                            .text_color(rgb(color))
                            .child(div().size(px(6.)).rounded_full().bg(rgb(color)))
                            .child(status),
                    )
                    .children(running.then(|| {
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .mr_2()
                            .text_xs()
                            .child("전체 음소거")
                            .child(self.mute("mute-all", "전체 출력 음소거", MuteTarget::All, cx))
                    }))
                    .child(
                        button("new-scene", "새 Scene", false)
                            .disabled(
                                running
                                    || self.busy.is_some()
                                    || self.picking
                                    || self.form.is_some(),
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.perform(Operation::Devices, window, cx)
                            })),
                    )
                    .child(
                        button("open-scene", "Scene 열기", false)
                            .disabled(
                                running
                                    || self.busy.is_some()
                                    || self.picking
                                    || self.form.is_some(),
                            )
                            .on_click(
                                cx.listener(|this, _, window, cx| this.choose_scene(window, cx)),
                            ),
                    )
                    .child(if running {
                        button("stop-audio", "정지", false).disabled(self.picking).on_click(cx.listener(
                            |this, _, window, cx| this.perform(Operation::Stop, window, cx),
                        ))
                    } else {
                        button("start-audio", "오디오 시작", true)
                            .disabled(
                                self.session.scene().is_none()
                                    || self.busy.is_some()
                                    || self.picking
                                    || self.form.is_some(),
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.perform(Operation::Start, window, cx)
                            }))
                    }),
            )
    }

    fn scene_summary(&self, cx: &Context<Self>) -> gpui::AnyElement {
        let Some(scene) = self.session.scene() else {
            let (title, description) = if self.busy.is_some() {
                (
                    self.selected_name.clone(),
                    "오디오 경로를 준비하거나 정리하고 있습니다. 잠시 기다려 주세요.",
                )
            } else {
                (
                    "새 Scene으로 시작하세요".to_owned(),
                    "장치를 선택해 새 Scene을 만들거나, 저장한 파일을 열어 주세요.",
                )
            };
            return panel()
                .py_12()
                .items_center()
                .gap_3()
                .child(div().text_2xl().font_semibold().child(title))
                .child(div().text_sm().text_color(rgb(MUTED)).child(description))
                .into_any_element();
        };
        let route = &scene.route;
        let mut sources = panel()
            .flex_1()
            .min_w_0()
            .gap_4()
            .child(section_title("01", "소스"))
            .child(device(
                "내 목소리",
                &route.input,
                format!(
                    "입력 채널 {} · 입력 보정 {:+.1} dB",
                    route.input_channel, route.input_gain_db
                ),
            ));
        if let Some(aux) = &route.aux_input {
            sources = sources.child(device(
                "AUX / MR",
                aux,
                format!(
                    "입력 채널 {} · {:+.1} dB",
                    channels(&route.aux_channels),
                    route.aux_gain_db
                ),
            ));
        } else {
            sources = sources.child(
                div()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child("AUX / MR 미선택"),
            );
        }

        for (index, app) in route.apps.iter().enumerate() {
            sources = sources.child(device("앱 캡처", &app.path,
                format!("App {} · 채널 {} · {:+.1} dB", index + 1, channels(&app.channels), app.gain_db)));
        }

        let vocal = self.session.controls().map_or_else(|| route.vocal_config(), |controls| controls.vocal_config());
        let mut sound = panel()
            .flex_1()
            .min_w_0()
            .gap_4()
            .child(section_title("02", "보컬 체인"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .font_semibold()
                            .child(format!("내장 보정 · {}", vocal.preset)),
                    )
                    .child(div().text_xs().text_color(rgb(MUTED)).child(format!(
                        "보정 비율 {:.0}% · 이후 VST3 {}개",
                        vocal.strength * 100.0,
                        scene.effects.len()
                    )))
                    .child(div().text_xs().text_color(rgb(MUTED)).child(if vocal.reverb.mix == 0.0 {
                        "스테레오 리버브 · 꺼짐".to_owned()
                    } else {
                        format!("리버브 {:.1}% · {:.2} s · pre-delay {:.1} ms", vocal.reverb.mix * 100.0,
                            vocal.reverb.decay_secs, vocal.reverb.predelay_ms)
                    }))
                    .child(div().text_xs().text_color(rgb(MUTED)).child(if vocal.echo.mix == 0.0 {
                        "스테레오 에코 · 꺼짐".to_owned()
                    } else {
                        format!("에코 {:.1}% · {:.1} ms · 피드백 {:.1}%", vocal.echo.mix * 100.0, vocal.echo.time_ms, vocal.echo.feedback * 100.0)
                    })),
            );
        if scene.effects.is_empty() {
            sound = sound.child(
                div()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child("선택된 VST3 없음"),
            );
        }
        for (index, effect) in scene.effects.iter().enumerate() {
            let name = effect
                .plugin
                .file_name()
                .unwrap_or(effect.plugin.as_os_str())
                .to_string_lossy()
                .into_owned();
            let bypassed = self.session.controls().map_or(effect.bypassed, |controls| {
                controls.effect_bypassed(index).expect("configured live VST3 slot")
            });
            sound = sound.child(
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .child(
                        div()
                            .w_5()
                            .text_sm()
                            .text_color(rgb(ACCENT))
                            .child(format!("{:02}", index + 1)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().truncate().text_sm().child(name))
                            .child(div().text_xs().text_color(rgb(MUTED)).child(
                                if effect.state.is_some() {
                                    "저장된 시작 상태"
                                } else {
                                    "플러그인 기본 시작 상태"
                                },
                            ))
                            .child(div().text_xs().text_color(rgb(MUTED)).child(
                                if bypassed { "우회 · 지연 유지" } else { "효과 적용" },
                            ))
                            .children(self.session.running().then(|| {
                                let label = if bypassed { "우회 끄기" } else { "우회 켜기" };
                                button(("vst3-bypass", index), label, false)
                                    .accessibility_label(format!("VST3 {} {label}", index + 1))
                                    .disabled(self.busy.is_some() || self.picking)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let _ = this.session.apply(LiveCommand::EffectBypass { index, bypassed: !bypassed });
                                        cx.notify();
                                    }))
                            }))
                            .children(self.session.running().then(|| {
                                let open = !self.session.editor_is_open(index);
                                let label = if open { "원본 창 열기" } else { "원본 창 닫기" };
                                button(("vst3-editor", index), label, false)
                                    .accessibility_label(format!("VST3 {} {label}", index + 1))
                                    .disabled(self.busy.is_some() || self.picking)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.perform(Operation::Editor { index, open }, window, cx);
                                    }))
                            })),
                    ),
            );
        }
        sound = sound
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child("AUX / MR과 앱 소스는 보컬 체인을 거치지 않습니다."),
            )
            .child(
                button("edit-sound", "보컬 설정", false)
                    .disabled(
                        self.session.running()
                            || self.busy.is_some()
                            || self.picking
                            || self.form.is_some(),
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.edit_sound(window, cx))),
            );

        let mut destinations = panel()
            .flex_1()
            .min_w_0()
            .gap_4()
            .child(section_title("03", "목적지"))
            .child(device(
                "주 출력",
                &route.output,
                format!("출력 채널 {}", channels(&route.output_channels)),
            ));
        if let Some(send) = &route.send {
            destinations = destinations.child(device(
                "추가 송출",
                send,
                format!("출력 채널 {}", channels(&route.send_channels)),
            ));
        } else {
            destinations = destinations.child(
                div()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child("추가 송출 미선택"),
            );
        }
        if let Some(broadcast) = &route.broadcast {
            destinations = destinations.child(device("방송 송출", broadcast,
                format!("출력 채널 {}", channels(&route.broadcast_channels))));
        } else {
            destinations = destinations.child(div().text_sm().text_color(rgb(MUTED)).child("방송 송출 미선택"));
        }
        destinations = destinations.child(
            button("edit-devices", "장치·출력 설정", false)
                .disabled(
                    self.session.running()
                        || self.busy.is_some()
                        || self.picking
                        || self.form.is_some(),
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.perform(Operation::EditDevices, window, cx)
                })),
        );

        div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().text_xs().text_color(rgb(MUTED)).child("현재 Scene"))
                            .child(
                                div()
                                    .truncate()
                                    .text_lg()
                                    .font_semibold()
                                    .child(self.selected_name.clone()),
                            )
                            .child(
                                div().truncate().text_xs().text_color(rgb(MUTED)).child(
                                    self.session
                                        .path()
                                        .expect("loaded Scene path")
                                        .display()
                                        .to_string(),
                                ),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child("열기만으로 오디오가 시작되지 않습니다."),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_4()
                    .items_start()
                    .child(sources)
                    .child(sound)
                    .child(destinations),
            )
            .into_any_element()
    }

    fn live_vocal(&self, cx: &Context<Self>) -> Div {
        let controls = self.session.controls().expect("running session");
        let config = controls.vocal_config();
        let mut fields = div().flex().flex_wrap().gap_4();
        for parameter in controls.vocal_parameters() {
            let label = match parameter {
                VocalParameter::Trim => "입력 trim",
                VocalParameter::Strength => "보정 비율",
                VocalParameter::Compressor => "컴프레서 threshold",
                VocalParameter::Deesser => "디에서 threshold",
                VocalParameter::ReverbMix => "리버브 비율",
                VocalParameter::ReverbDecay => "잔향 시간",
                VocalParameter::EchoMix => "에코 비율",
                VocalParameter::EchoFeedback => "에코 피드백",
            };
            fields = fields.child(div().min_w(px(250.)).flex_1().flex().flex_col().gap_1()
                .child(div().text_sm().font_semibold().child(label))
                .child(self.fader(("vocal-param", parameter as usize), &format!("실시간 {label}"),
                    FaderTarget::Vocal(parameter), controls.vocal_value(parameter), cx)));
        }
        let hint = if config.preset == mixlive_effects::Preset::Off {
            "EQ/다이내믹스 프리셋이 꺼져 있습니다. 정지 후 프리셋을 선택하면 보정 비율과 threshold를 조절할 수 있습니다."
        } else {
            "보정 비율을 0으로 내리면 EQ/다이내믹스를 우회합니다. 리버브와 에코는 별도로 조절합니다."
        };
        panel().gap_3()
            .child(div().flex().items_center().justify_between().gap_3()
                .child(div().font_semibold().child("실시간 보컬 보정"))
                .child(div().text_xs().text_color(rgb(MUTED))
                    .child(format!("{} · pre-delay {:.1} ms · echo {:.1} ms", config.preset, config.reverb.predelay_ms, config.echo.time_ms))))
            .child(fields)
            .child(div().text_xs().text_color(rgb(MUTED)).child(hint))
            .child(div().text_xs().text_color(rgb(MUTED))
                .child("프리셋과 pre-delay, 에코 시간은 정지 상태에서 변경합니다. 조절값은 약 10 ms 전환 후 반영되며, 정지 후 새 Scene 저장으로 보존할 수 있습니다."))
    }

    fn mixer(&self, cx: &Context<Self>) -> gpui::AnyElement {
        let controls = self.session.controls().expect("running session");
        let levels = self.session.levels.as_ref();
        let scene = self.session.scene().expect("running Scene");
        let mut source_mutes = div().flex().items_center().gap_5().child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_sm()
                .child("내 목소리 음소거")
                .child(self.mute("mute-mic", "내 목소리 음소거", MuteTarget::Mic, cx)),
        );
        if controls.has_aux() {
            source_mutes = source_mutes.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_sm()
                    .child("AUX / MR 음소거")
                    .child(self.mute("mute-aux", "AUX / MR 음소거", MuteTarget::Aux, cx)),
            );
        }
        let mut routing = panel().flex_1().min_w_0().gap_5().child(
            div()
                .flex()
                .flex_col()
                .gap_3()
                .child(div().font_semibold().child("소스별 보내기"))
                .child(source_mutes)
                .child(peak_meter("마이크 입력 · trim 전", levels.map(|l| l.mic_input)))
                .child(peak_meter("내장 보정 후 · VST3 전", levels.map(|l| l.vocal.output_peak)))
                .child(peak_meter("보컬 체인 후 · mute 전", levels.map(|l| l.voice)))
                .children(controls.has_aux().then(|| {
                    div().flex().flex_col().gap_2()
                        .child(peak_meter("AUX 입력 · trim/mute 전", levels.and_then(|l| l.aux_input)))
                        .child(div().text_sm().child("AUX / MR 입력 trim"))
                        .child(self.fader("aux-trim", "AUX / MR 입력 trim", FaderTarget::Trim(RouteSource::Aux),
                            controls.source_trim_db(RouteSource::Aux).expect("configured AUX"), cx))
                }))
                .children(levels.map(|l| {
                    div().text_xs().text_color(rgb(MUTED)).child(format!(
                        "내장 보정 GR · 컴프레서 {:.1} / 디에서 {:.1} dB",
                        l.vocal.compression_db, l.vocal.deessing_db))
                })),
        );
        for (index, app) in scene.route.apps.iter().enumerate() {
            let label = source_name(RouteSource::App(index), scene);
            routing = routing.child(div().flex().flex_col().gap_2().pt_3().border_t_1().border_color(rgb(BORDER))
                .child(div().flex().items_center().justify_between().gap_2()
                    .child(div().flex_1().min_w_0().font_semibold().child(label.clone()))
                    .child(self.mute(("mute-app", index), &format!("{label} 음소거"), MuteTarget::App(index), cx)))
                .child(peak_meter("앱 입력 · trim/mute 전", levels.and_then(|l| l.app_inputs[index])))
                .child(div().text_sm().child("입력 trim"))
                .child(self.fader(("app-trim", index), &format!("{label} 입력 trim"),
                    FaderTarget::Trim(RouteSource::App(index)),
                    controls.source_trim_db(RouteSource::App(index)).expect("configured app"), cx))
                .child(div().text_xs().text_color(rgb(MUTED)).truncate().child(app.path.clone())));
        }
        for mix in controls.mixes() {
            let destination = mix_name(mix);
            let mut group = div()
                .flex()
                .flex_col()
                .gap_3()
                .pt_4()
                .border_t_1()
                .border_color(rgb(BORDER))
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .text_color(rgb(MUTED))
                        .child(format!("{destination}에 보내기")),
                );
            for (source_index, source) in controls.sources().enumerate() {
                let source_label = source_name(source, scene);
                let (enabled, db) = controls
                    .route_state(source, mix)
                    .expect("configured route exists");
                let key = source_index * RouteMix::ALL.len() + mix.index();
                let owner = cx.entity();
                group = group.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_3()
                                .child(div().text_sm().child(source_label.clone()))
                                .child(
                                    toggle(
                                        ("route-enabled", key),
                                        format!("{source_label} → {destination}"),
                                        enabled,
                                        false,
                                    )
                                    .on_change(
                                        move |enabled, _, _, cx| {
                                            owner.update(cx, |this, cx| {
                                                let _ = this.session.apply(LiveCommand::Route {
                                                    source,
                                                    mix,
                                                    enabled,
                                                });
                                                cx.notify();
                                            });
                                        },
                                    ),
                                ),
                        )
                        .child(self.fader(
                            ("route-level", key),
                            &format!("{source_label} → {destination} 음량"),
                            FaderTarget::Route(source, mix),
                            db,
                            cx,
                        )),
                );
            }
            routing = routing.child(group);
        }

        let master_db = controls.gain_db(GainTarget::Master).expect("master exists");
        let mut output = panel().w(px(330.)).flex_shrink_0().gap_4()
            .child(div().font_semibold().child("최종 출력 음량"))
            .child(div().flex().flex_col().gap_1()
                .child(div().text_sm().child("전체 음량"))
                .child(div().text_xs().text_color(rgb(MUTED)).child("모든 출력에 공통 적용됩니다."))
                .child(self.fader("master-level", "전체 출력 음량",
                    FaderTarget::Gain(GainTarget::Master), master_db, cx)));
        for mix in controls.mixes() {
            let slot = mix.index();
            let name = mix_name(mix);
            let target = mix.gain_target();
            let db = controls.gain_db(target).expect("configured mix");
            let meter_label = match mix {
                RouteMix::Output => "모니터 믹스 peak",
                RouteMix::Send => "통화 믹스 peak",
                RouteMix::Broadcast => "방송 믹스 peak",
            };
            output = output.child(div().flex().flex_col().gap_2()
                .pt_4().border_t_1().border_color(rgb(BORDER))
                .child(peak_meter(meter_label, levels.and_then(|l| l.outputs[slot])))
                .child(div().text_sm().font_semibold().child(name))
                .child(self.fader(("mix-level", slot), &format!("{name} 추가 감쇠"),
                    FaderTarget::Gain(target), db, cx))
                .child(div().text_xs().text_color(rgb(MUTED)).child(format!("최종 감쇠: {:.1} dB", master_db + db)))
                .child(div().flex().items_center().justify_between().text_sm()
                    .child("음소거")
                    .child(self.mute(("mute-mix", slot), &format!("{name} 음소거"), mix.mute_target(), cx))));
        }
        output = output.child(div().text_xs().text_color(rgb(MUTED)).pt_3().child(
            "출력 peak는 감쇠·mute·sample guard 후, 장치 채널 매핑 전입니다. 음소거와 경로 선택은 각각 유지됩니다.",
        ));

        div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div().flex().items_center().justify_between().gap_4().child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(div().text_lg().font_semibold().child("라이브 믹서"))
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(MUTED))
                                .child("이번 실행에만 적용"),
                        ),
                ),
            )
            .child(self.live_vocal(cx))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap_4()
                    .child(routing)
                    .child(output),
            )
            .into_any_element()
    }

    fn diagnostics(&self, cx: &Context<Self>) -> Div {
        let mut content = panel().gap_3().child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_sm()
                        .child(self.session.message.clone()),
                )
                .child(
                    button(
                        "toggle-details",
                        if self.details {
                            "상세 상태 접기"
                        } else {
                            "상세 상태 펼치기"
                        },
                        false,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.details = !this.details;
                        cx.notify();
                    })),
                ),
        );
        if self.session.running() {
            content = content.child(
                button("save-running-scene", "정지 후 새 Scene 저장", true)
                    .disabled(self.picking || self.busy.is_some())
                    .on_click(cx.listener(|this, _, window, cx| this.choose_snapshot_path(window, cx))),
            );
        } else if let Some(scene) = self.session.scene() {
            let initial = mixlive_audio_windows::control::LiveControls::from_route(&scene.route)
                .expect("loaded Scene validated");
            content = content.child(detail_text("저장된 믹서 시작 설정", &initial.describe()));
        }
        if self.details {
            if !self.session.device_details.is_empty() {
                content = content.child(detail_text("장치 설정", &self.session.device_details));
            }
            if !self.session.timing.is_empty() {
                content = content.child(detail_text(
                    "보고된 버퍼 / 큐 정보 · 실측 왕복 지연 아님",
                    &self.session.timing.join("\n"),
                ));
            }
            if !self.session.report.is_empty() {
                content = content.child(detail_text("엔진 상태", &self.session.report));
            }
            if self.session.device_details.is_empty() && self.session.report.is_empty() {
                content = content.child(
                    div()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child("오디오 시작 후 실제 엔진 상태를 표시합니다."),
                );
            }
        }
        content
    }
}

impl Render for MixLive {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = ScrollArea::new("workspace", &self.workspace_scroll)
            .accessibility_label("Scene 작업 영역")
            .flex_1()
            .min_h_0()
            .content_styles(|style| style.flex().flex_col().gap_6().px_7().py_6())
            .scrollbar_styles(|styles| styles.thumb(|thumb| thumb.bg(rgb(MUTED)).radius(px(3.))));
        if let Some(error) = &self.session.error
            && self.form.is_none()
        {
            body = body.child(
                panel()
                    .border_color(rgb(DANGER))
                    .gap_2()
                    .child(
                        div()
                            .font_semibold()
                            .text_color(rgb(DANGER))
                            .child("오디오 설정을 확인해 주세요"),
                    )
                    .child(div().text_sm().child(error.clone())),
            );
        }
        if let Some(form) = &self.form {
            body = body.child(form.clone());
        } else {
            body = body.child(self.scene_summary(cx));
            if self.session.running() {
                body = body.child(self.mixer(cx));
            }
            if self.busy.is_none()
                && (self.session.scene().is_some() || self.session.error.is_some())
            {
                body = body.child(self.diagnostics(cx));
            }
        }
        div().size_full().flex().flex_col().bg(rgb(BACKGROUND)).text_color(rgb(TEXT))
            .font_family("Malgun Gothic").text_sm()
            .child(self.header(cx))
            .child(body)
            .child(div().flex_shrink_0().px_7().py_3().border_t_1().border_color(rgb(BORDER)).text_xs().text_color(rgb(MUTED))
                .child("일반 정지는 변경값을 버립니다. 정지 후 새 Scene 저장을 선택하면 현재 믹서·보컬 설정과 VST3 상태를 보존합니다."))
    }
}

/// Scale the measured sample peak over a -60..0 dBFS rail; silence is not -60 dBFS.
fn peak_display(peak: Option<f32>) -> (f32, String, u32) {
    match peak {
        None => (0.0, "측정 대기".into(), MUTED),
        Some(0.0) => (0.0, "−∞ dBFS".into(), ACCENT),
        Some(peak) => {
            let db = 20.0 * peak.log10();
            let fill = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
            (fill, format!("{db:.1} dBFS"), if db >= 0.0 { DANGER } else if db >= -6.0 { 0xf2c879 } else { ACCENT })
        }
    }
}

fn peak_meter(label: &'static str, peak: Option<f32>) -> Div {
    let (fill, value, color) = peak_display(peak);
    div().flex().flex_col().gap_1()
        .child(div().flex().items_center().justify_between().gap_2()
            .child(div().text_xs().text_color(rgb(MUTED)).child(label))
            .child(div().text_xs().text_color(rgb(color)).child(value)))
        .child(div().h(px(7.)).w_full().rounded_full().bg(rgb(BACKGROUND)).overflow_hidden()
            .child(div().h_full().w(relative(fill)).rounded_full().bg(rgb(color))))
}

fn scene_name(session: &Session) -> String {
    session
        .path()
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn channels(values: &[u16]) -> String {
    values
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn source_name(source: RouteSource, scene: &mixlive_audio_windows::scene::AudioScene) -> String {
    match source {
        RouteSource::Mic => "내 목소리".into(),
        RouteSource::Aux => "AUX / MR".into(),
        RouteSource::App(index) => {
            let app = &scene.route.apps[index];
            let name = std::path::Path::new(&app.path).file_name().and_then(|name| name.to_str()).unwrap_or(&app.path);
            format!("App {} · {name}", index + 1)
        }
    }
}

fn mix_name(mix: RouteMix) -> &'static str {
    match mix {
        RouteMix::Output => "모니터",
        RouteMix::Send => "통화 송출",
        RouteMix::Broadcast => "방송 송출",
    }
}

pub(crate) fn panel() -> Div {
    div()
        .flex_shrink_0()
        .flex()
        .flex_col()
        .p_5()
        .rounded(px(14.))
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(SURFACE))
}

pub(crate) fn section_title(number: &'static str, title: &'static str) -> Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .child(div().text_xs().text_color(rgb(ACCENT)).child(number))
        .child(div().font_semibold().child(title))
}

fn device(label: &'static str, id: &str, detail: String) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_sm().font_semibold().child(label))
        .child(div().text_xs().text_color(rgb(MUTED)).child(detail))
        .child(
            div()
                .truncate()
                .text_xs()
                .text_color(rgb(MUTED))
                .child(id.to_owned()),
        )
}

fn detail_text(label: &'static str, text: &str) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .pt_3()
        .border_t_1()
        .border_color(rgb(BORDER))
        .child(
            div()
                .text_xs()
                .font_semibold()
                .text_color(rgb(MUTED))
                .child(label),
        )
        .child(div().text_xs().child(text.to_owned()))
}

pub(crate) fn button(id: impl Into<ElementId>, label: &'static str, primary: bool) -> Button {
    Button::new(id)
        .accessibility_label(label)
        .child(label)
        .h(px(38.))
        .px_4()
        .rounded(px(8.))
        .border_1()
        .border_color(rgb(if primary { ACCENT } else { BORDER }))
        .bg(rgb(if primary { ACCENT } else { SURFACE }))
        .text_color(rgb(if primary { BACKGROUND } else { TEXT }))
        .text_sm()
        .font_semibold()
        .hover(|style| style.opacity(0.85))
        .focus_visible(|style| style.border_color(rgb(TEXT)))
        .styles(|styles| styles.disabled(|style| style.opacity(0.4)))
}

fn toggle(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    checked: bool,
    mute: bool,
) -> Switch {
    Switch::new(id)
        .accessibility_label(label)
        .checked(checked)
        .relative()
        .w(px(38.))
        .h(px(22.))
        .rounded_full()
        .bg(rgb(if checked {
            if mute { DANGER } else { ACCENT }
        } else {
            BORDER
        }))
        .border_1()
        .border_color(rgb(if checked { TEXT } else { MUTED }))
        .focus_visible(|style| style.border_color(rgb(TEXT)))
        .child(
            div()
                .absolute()
                .top(px(3.))
                .left(px(if checked { 19. } else { 3. }))
                .size(px(14.))
                .rounded_full()
                .bg(rgb(if checked { BACKGROUND } else { TEXT })),
        )
}

pub fn run(scene: Option<PathBuf>) {
    gpui_platform::application().run(move |cx| {
        gpui_deck::init(cx);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.open_window(
            WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some("MixLive".into()),
                    appears_transparent: false,
                    traffic_light_position: None,
                }),
                window_bounds: Some(WindowBounds::centered(size(px(1120.), px(840.)), cx)),
                window_min_size: Some(size(px(980.), px(660.))),
                app_id: Some("MixLive".into()),
                ..Default::default()
            },
            move |window, cx| {
                let view = cx.new(|cx| MixLive::new(scene, window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            },
        )
        .expect("failed to open MixLive window");
        cx.activate(true);
    });
}

#[cfg(test)]
mod meter_tests {
    use super::*;
    #[test]
    fn sample_peak_display_has_a_logarithmic_scale_and_no_synthetic_signal() {
        assert_eq!(peak_display(None).0, 0.0);
        assert_eq!(peak_display(Some(0.0)).1, "−∞ dBFS");
        assert_eq!(peak_display(Some(0.0001)).0, 0.0);
        assert!((peak_display(Some(10.0_f32.powf(-30.0 / 20.0))).0 - 0.5).abs() < 1e-6);
        assert_eq!(peak_display(Some(1.0)), (1.0, "0.0 dBFS".into(), DANGER));
        assert_eq!(peak_display(Some(2.0)), (1.0, "6.0 dBFS".into(), DANGER));
    }
}
