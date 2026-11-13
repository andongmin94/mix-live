//! The native Scene runner. Native lifecycle calls run on GPUI's foreground
//! executor, outside entity borrows, so COM/plugin callbacks may re-enter GPUI.

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use gpui::{
    App, AppContext as _, Context, Div, ElementId, Entity, InteractiveElement, IntoElement,
    ParentElement, PathPromptOptions, Render, ScrollHandle, SharedString, Styled, Subscription,
    TitlebarOptions, Window, WindowBounds, WindowOptions, div, px, relative, rgb, size,
};
use gpui_design::{
    Button, Root, ScrollArea, Slider, SliderEvent, SliderState, StyledExt as _, Switch,
};
use mixlive_audio_windows::control::{GainTarget, LiveCommand, MuteTarget, RouteMix, RouteSource};
use mixlive_audio_windows::windows::device_catalog;

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
}

impl FaderTarget {
    fn command(self, db: f32) -> LiveCommand {
        match self {
            Self::Gain(target) => LiveCommand::Gain { target, db },
            Self::Route(source, mix) => LiveCommand::RouteGain { source, mix, db },
        }
    }
}

struct Fader {
    target: FaderTarget,
    state: Entity<SliderState>,
}

#[derive(Clone, Copy)]
enum Operation {
    Devices,
    Start,
    Stop,
    Close,
}

pub struct MixLive {
    session: Session,
    busy: Option<Operation>,
    picking: bool,
    details: bool,
    selected_name: String,
    faders: Vec<Fader>,
    subscriptions: Vec<Subscription>,
    form: Option<Entity<SceneForm>>,
    form_subscription: Option<Subscription>,
    workspace_scroll: ScrollHandle,
    last_refresh: Instant,
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

        // The timer waits in the scheduler; poll/update itself remains on the
        // foreground thread that loaded the native chain. No audio callback UI.
        cx.spawn_in(window, async move |owner, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                let alive = cx.update(|window, cx| {
                    owner.update(cx, |this, cx| this.poll(window, cx)).is_ok()
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
            last_refresh: Instant::now(),
        }
    }

    fn poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() || !self.session.running() {
            return;
        }
        if self.session.poll().is_err() {
            self.perform(Operation::Stop, window, cx);
            return;
        }
        if self.last_refresh.elapsed() >= Duration::from_secs(1) {
            self.last_refresh = Instant::now();
            cx.notify();
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

    fn perform(&mut self, operation: Operation, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        if matches!(operation, Operation::Start | Operation::Devices)
            && (self.picking || self.form.is_some() || self.session.running())
        {
            return;
        }
        if matches!(operation, Operation::Start) && self.session.scene().is_none() {
            return;
        }
        self.busy = Some(operation);
        self.faders.clear();
        self.subscriptions.clear();
        let mut session = std::mem::take(&mut self.session);
        cx.notify();
        cx.spawn_in(window, async move |owner, cx| {
            // These synchronous native calls MUST be outside App/entity update.
            let catalog = match operation {
                Operation::Devices => Some(device_catalog().map_err(|error| error.to_string())),
                Operation::Start => {
                    let _ = session.start();
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
                    this.busy = None;
                    if let Some(catalog) = catalog {
                        match catalog {
                            Ok(catalog) => {
                                this.session.error = None;
                                let form = cx.new(|cx| SceneForm::new(catalog, window, cx));
                                this.install_form(form, window, cx);
                            }
                            Err(error) => {
                                this.session.message = "장치 목록을 읽지 못했습니다. 연결을 확인하고 새 Scene을 다시 누르세요.".into();
                                this.session.error = Some(error);
                            }
                        }
                    }
                    this.install_faders(cx);
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
        if controls.mix_count() == 2 {
            values.push((
                FaderTarget::Gain(GainTarget::Send),
                controls.gain_db(GainTarget::Send).expect("send exists"),
            ));
        }
        for source in [RouteSource::Mic, RouteSource::Aux] {
            if source == RouteSource::Aux && !controls.has_aux() {
                continue;
            }
            for mix in [RouteMix::Output, RouteMix::Send]
                .into_iter()
                .take(controls.mix_count())
            {
                let (_, db) = controls
                    .route_state(source, mix)
                    .expect("configured route exists");
                values.push((FaderTarget::Route(source, mix), db));
            }
        }
        for (target, value) in values {
            let state = cx.new(|cx| SliderState::new(-96.0..=0.0, 0.1, cx).default_value(value));
            self.subscriptions
                .push(cx.subscribe(&state, move |this, _, event, cx| {
                    let SliderEvent::Change(db) = event;
                    let _ = this.session.apply(target.command(*db));
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
        db: f32,
        cx: &App,
    ) -> gpui::AnyElement {
        let state = &self
            .faders
            .iter()
            .find(|fader| fader.target == target)
            .expect("live fader installed")
            .state;
        let percentage = state.read(cx).percentage();
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
                    .focus_visible(|style| style.bg(rgb(RAISED)))
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
                        div()
                            .absolute()
                            .left(relative(percentage))
                            .top(relative(0.5))
                            .ml(px(-6.))
                            .mt(px(-6.))
                            .size(px(12.))
                            .rounded_full()
                            .bg(rgb(TEXT)),
                    ),
            )
            .child(
                div()
                    .w(px(70.))
                    .text_right()
                    .text_sm()
                    .child(format!("{db:.1} dB")),
            )
            .into_any_element()
    }

    fn header(&self, cx: &Context<Self>) -> Div {
        let running = self.session.running();
        let (status, color) = match self.busy {
            Some(Operation::Devices) => ("장치 조회 중", MUTED),
            Some(Operation::Start) => ("시작 중", MUTED),
            Some(Operation::Stop | Operation::Close) => ("정지 중", MUTED),
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
                        button("stop-audio", "정지", false).on_click(cx.listener(
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
                            .child(format!("내장 보정 · {}", route.vocal)),
                    )
                    .child(div().text_xs().text_color(rgb(MUTED)).child(format!(
                        "보정 비율 {:.0}% · 이후 VST3 {}개",
                        route.vocal_strength * 100.0,
                        scene.effects.len()
                    ))),
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
                                    "저장된 상태"
                                } else {
                                    "플러그인 기본 상태"
                                },
                            )),
                    ),
            );
        }
        sound = sound
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child("AUX / MR은 보컬 체인을 거치지 않습니다."),
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

    fn mixer(&self, cx: &Context<Self>) -> gpui::AnyElement {
        let controls = self.session.controls().expect("running session");
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
                .child(source_mutes),
        );
        for (mix_index, mix) in [RouteMix::Output, RouteMix::Send]
            .into_iter()
            .take(controls.mix_count())
            .enumerate()
        {
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
            for (source_index, source) in
                [RouteSource::Mic, RouteSource::Aux].into_iter().enumerate()
            {
                if source == RouteSource::Aux && !controls.has_aux() {
                    continue;
                }
                let source_label = source_name(source);
                let (enabled, db) = controls
                    .route_state(source, mix)
                    .expect("configured route exists");
                let key = source_index * 2 + mix_index;
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
                                .child(div().text_sm().child(source_label))
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
        let mut output = panel()
            .w(px(330.))
            .flex_shrink_0()
            .gap_4()
            .child(div().font_semibold().child("최종 출력 음량"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_sm().child("전체 음량"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child("주 출력과 추가 송출에 함께 적용됩니다."),
                    )
                    .child(self.fader(
                        "master-level",
                        "전체 출력 음량",
                        FaderTarget::Gain(GainTarget::Master),
                        master_db,
                        cx,
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_sm()
                    .child("주 출력 음소거")
                    .child(self.mute("mute-output", "주 출력 음소거", MuteTarget::Output, cx)),
            );
        if controls.mix_count() == 2 {
            let send_db = controls.gain_db(GainTarget::Send).expect("send exists");
            output = output.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .pt_4()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .child(div().text_sm().child("송출 추가 감쇠"))
                    .child(self.fader(
                        "send-level",
                        "추가 송출 감쇠",
                        FaderTarget::Gain(GainTarget::Send),
                        send_db,
                        cx,
                    ))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child(format!("송출 최종 감쇠: {:.1} dB", master_db + send_db)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .text_sm()
                            .child("추가 송출 음소거")
                            .child(self.mute(
                                "mute-send",
                                "추가 송출 음소거",
                                MuteTarget::Send,
                                cx,
                            )),
                    ),
            );
        }
        output = output.child(div().text_xs().text_color(rgb(MUTED)).pt_3().child(
            "음소거와 경로 선택은 각각 유지됩니다. 음량을 바꿔도 음소거는 해제되지 않습니다.",
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
                .child("실행 중 조절값은 Scene에 저장되지 않습니다. 다시 시작하면 저장된 설정을 사용합니다."))
    }
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

fn source_name(source: RouteSource) -> &'static str {
    match source {
        RouteSource::Mic => "내 목소리",
        RouteSource::Aux => "AUX / MR",
    }
}

fn mix_name(mix: RouteMix) -> &'static str {
    match mix {
        RouteMix::Output => "주 출력",
        RouteMix::Send => "추가 송출",
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

pub(crate) fn button(id: &'static str, label: &'static str, primary: bool) -> Button {
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
        gpui_design::init(cx);
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
