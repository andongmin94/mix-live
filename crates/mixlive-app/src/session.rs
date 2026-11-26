//! One UI-owned session. All native calls stay on GPUI's foreground/loading thread.
//! Scene reads/writes do not resolve endpoints or load plugins. Existing files are never replaced.

use mixlive_audio_windows::{
    config::{AppSource, AuxSource, OutputBinding, PitchConfig},
    control::{LiveCommand, LiveControls},
    plugin::{ChainEdit, EffectSpec, LiveParameter, PluginArgs},
    scene::AudioScene,
    windows::{Active, LevelReadings, Prepared},
};
use mixlive_effects::VocalSettings;
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub struct Session {
    loaded: Option<(PathBuf, AudioScene)>,
    active: Option<Active>,
    live_effects: Vec<EffectSpec>,
    last_report: Instant,
    last_meter: Instant,
    pub levels: Option<LevelReadings>,
    pub message: String,
    pub error: Option<String>,
    pub device_details: String,
    pub timing: Vec<String>,
    pub report: String,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            loaded: None,
            active: None,
            live_effects: Vec::new(),
            last_report: Instant::now(),
            last_meter: Instant::now(),
            levels: None,
            message: "새 Scene을 만들거나 파일을 열어 시작 설정을 확인하세요.".into(),
            error: None,
            device_details: String::new(),
            timing: Vec::new(),
            report: String::new(),
        }
    }
}

impl Session {
    pub fn running(&self) -> bool {
        self.active.is_some()
    }

    pub fn scene(&self) -> Option<&AudioScene> {
        self.loaded.as_ref().map(|(_, scene)| scene)
    }

    pub fn path(&self) -> Option<&Path> {
        self.loaded.as_ref().map(|(path, _)| path.as_path())
    }

    /// Display the current runtime order without changing the saved startup Scene.
    pub fn effects(&self) -> &[EffectSpec] {
        if self.running() {
            &self.live_effects
        } else {
            self.scene().map_or(&[], |scene| scene.effects.as_slice())
        }
    }

    /// Source indices follow the active capture layout, not the saved startup Scene.
    pub fn apps(&self) -> &[AppSource] {
        if let Some(active) = &self.active {
            active.apps()
        } else {
            self.scene().map_or(&[], |scene| scene.route.apps.as_slice())
        }
    }

    pub fn aux_source(&self) -> Option<AuxSource> {
        if let Some(active) = &self.active {
            active.aux_source().cloned()
        } else {
            self.scene().and_then(|scene| scene.route.aux_input.as_ref().map(|input| AuxSource {
                input: input.clone(), channels: scene.route.aux_channels.clone(),
            }))
        }
    }

    pub fn output_bindings(&self) -> Vec<OutputBinding> {
        if let Some(active) = &self.active {
            active.output_bindings().to_vec()
        } else {
            self.scene().map_or_else(Vec::new, |scene| scene.route.output_bindings())
        }
    }

    pub fn controls(&self) -> Option<&LiveControls> {
        self.active.as_ref().map(|active| active.controls.as_ref())
    }

    pub fn pitch_config(&self) -> Option<PitchConfig> {
        self.active.as_ref().map(Active::pitch_config)
            .or_else(|| self.scene().map(|scene| scene.route.pitch_config()))
    }

    pub fn vocal_settings(&self) -> Option<VocalSettings> {
        self.active.as_ref().map(Active::vocal_settings)
            .or_else(|| self.scene().map(|scene| scene.route.vocal_config().settings()))
    }

    fn fail<T>(&mut self, message: &str, error: impl ToString) -> Result<T, String> {
        let error = error.to_string();
        self.message = message.into();
        self.error = Some(error.clone());
        Err(error)
    }

    pub fn open(&mut self, path: PathBuf) -> Result<(), String> {
        if self.running() {
            return self.fail(
                "Scene을 바꾸려면 먼저 정지하세요.",
                "audio is still running",
            );
        }
        // A failed selection must not leave an older scene available under a new error.
        self.loaded = None;
        self.error = None;
        self.device_details.clear();
        self.timing.clear();
        self.report.clear();
        self.levels = None;
        match AudioScene::read(&path) {
            Ok(scene) => {
                self.loaded = Some((path, scene));
                self.message = "시작 준비 · 장치와 플러그인은 시작을 누를 때 확인합니다.".into();
                Ok(())
            }
            Err(error) => self.fail("Scene을 열지 못했습니다.", error),
        }
    }

    /// Save an explicit startup configuration, then select it without opening devices or plugins.
    /// Failure leaves the previously loaded Scene intact; save_new never overwrites a file.
    pub fn save_new(&mut self, path: PathBuf, scene: AudioScene) -> Result<(), String> {
        if self.running() {
            return self.fail(
                "Scene을 만들려면 먼저 정지하세요.",
                "audio is still running",
            );
        }
        if let Err(error) = scene.save_new(&path) {
            return self.fail(
                "새 Scene을 저장하지 못했습니다. 다른 파일 이름을 선택하세요.",
                error,
            );
        }
        self.loaded = Some((path, scene));
        self.error = None;
        self.device_details.clear();
        self.timing.clear();
        self.report.clear();
        self.levels = None;
        self.message = "새 Scene을 저장했습니다. 설정을 확인한 뒤 오디오 시작을 누르세요.".into();
        Ok(())
    }

    /// Call outside a mutable GPUI app/entity update: COM/native loading may dispatch messages.
    pub fn start(&mut self) -> Result<(), String> {
        if self.running() {
            return self.fail(
                "이미 실행 중입니다.",
                "stop the current session before starting again",
            );
        }
        self.error = None;
        self.device_details.clear();
        self.timing.clear();
        self.report.clear();
        self.levels = None;
        let Some(scene) = self.scene() else {
            return self.fail("먼저 Scene을 열어 주세요.", "no scene is selected");
        };
        // GPUI owns the Windows message loop. Editors open later on explicit request,
        // on the loading thread outside App/entity updates.
        let prepared = (|| {
            let plugins = PluginArgs::default().resolve_scene(scene.effects.clone())?;
            let prepared = Prepared::new(&scene.route).map_err(|error| error.to_string())?;
            let details = prepared.describe();
            let active = prepared
                .start(&plugins)
                .map_err(|error| error.to_string())?;
            Ok::<_, String>((details, active))
        })();
        match prepared {
            Ok((details, active)) => {
                self.device_details = details;
                self.timing = active.timing.clone();
                self.live_effects = active.effects();
                self.last_report = Instant::now();
                self.last_meter = Instant::now();
                self.active = Some(active);
                self.message = "실행 중 · 정지 후 새 Scene 저장으로 현재 AUX·앱 소스·출력·믹서·보컬 설정과 VST3 상태를 저장할 수 있습니다.".into();
                Ok(())
            }
            Err(error) => self.fail("시작하지 못했습니다. 설정과 장치를 확인하세요.", error),
        }
    }

    pub fn apply(&mut self, command: LiveCommand) -> Result<(), String> {
        let Some(controls) = self.controls() else {
            return self.fail("실행 중에만 조절할 수 있습니다.", "audio is not running");
        };
        match controls.apply(command) {
            Ok(()) => Ok(()),
            Err(error) => self.fail("제어 요청을 적용하지 못했습니다.", error),
        }
    }

    pub fn editor_is_open(&self, index: usize) -> bool {
        self.active.as_ref().is_some_and(|active| active.editor_is_open(index))
    }

    /// Prepare native effects and retire removed slots outside mutable GPUI updates.
    pub fn edit_chain(&mut self, edit: ChainEdit) -> Result<(), String> {
        let Some(active) = &mut self.active else {
            return self.fail("오디오 실행 중에만 체인을 변경할 수 있습니다.", "audio is not running");
        };
        let result = active.edit_chain(edit);
        // A cleanup error can occur after the new order is already active. Always display
        // the authoritative runtime order, never an optimistic edit of the loaded Scene.
        self.live_effects = active.effects();
        self.timing = active.timing.clone();
        match result {
            Ok(()) => {
                self.error = None;
                self.message = "VST3 체인 변경 적용 · 정지 후 새 Scene 저장으로 현재 순서와 상태를 보존할 수 있습니다.".into();
                Ok(())
            }
            Err(error) => self.fail("VST3 체인 변경을 완료하지 못했습니다.", error),
        }
    }

    /// Prepare and start the selected process client outside mutable GPUI updates.
    pub fn add_app(&mut self, source: AppSource) -> Result<(), String> {
        let Some(active) = &mut self.active else {
            return self.fail("오디오 실행 중에만 앱을 추가할 수 있습니다.", "audio is not running");
        };
        let previous = active.apps().to_vec();
        let result = active.add_app(source);
        self.timing = active.timing.clone();
        if active.apps() != previous {
            // An acknowledged layout can change before a later cleanup error. Old
            // meter slots must never be displayed against the new source names.
            self.levels = None;
            self.report.clear();
        }
        match result {
            Ok(()) => {
                self.error = None;
                self.message = "앱 추가 완료 · 소스별 보내기에서 연결할 출력을 켜세요. 정지 후 새 Scene 저장으로 보존할 수 있습니다.".into();
                Ok(())
            }
            Err(error) => self.fail("앱 추가 중 오류가 발생했습니다.", error),
        }
    }

    /// Retire only the selected capture client; the runtime publishes the compacted layout.
    pub fn remove_app(&mut self, index: usize) -> Result<(), String> {
        let Some(active) = &mut self.active else {
            return self.fail("오디오 실행 중에만 앱을 삭제할 수 있습니다.", "audio is not running");
        };
        let previous = active.apps().to_vec();
        let result = active.remove_app(index);
        self.timing = active.timing.clone();
        if active.apps() != previous {
            self.levels = None;
            self.report.clear();
        }
        match result {
            Ok(()) => {
                self.error = None;
                self.message = "앱 삭제 완료 · 남은 소스의 믹서 설정은 유지됩니다. 정지 후 새 Scene 저장으로 보존할 수 있습니다.".into();
                Ok(())
            }
            Err(error) => self.fail("앱 삭제를 완료하지 못했습니다.", error),
        }
    }

    /// Native device activation and retirement stay outside mutable GPUI updates.
    pub fn set_aux_source(&mut self, source: Option<AuxSource>) -> Result<(), String> {
        let Some(active) = &mut self.active else {
            return self.fail("오디오 실행 중에만 AUX를 변경할 수 있습니다.", "audio is not running");
        };
        let previous = active.aux_source().cloned();
        let result = active.set_aux_source(source);
        self.timing = active.timing.clone();
        if active.aux_source() != previous.as_ref() {
            // The acknowledged source is authoritative even after a later error.
            if let Some(levels) = &mut self.levels { levels.aux_input = None; }
            self.report.clear();
        }
        let present = active.aux_source().is_some();
        match result {
            Ok(()) => {
                self.error = None;
                self.message = if !present {
                    "AUX 삭제 완료 · 정지 후 새 Scene 저장으로 보존할 수 있습니다."
                } else if previous.is_none() {
                    "AUX 추가 완료 · 소스별 보내기에서 연결할 출력을 켜세요. 정지 후 새 Scene 저장으로 보존할 수 있습니다."
                } else {
                    "AUX 적용 완료 · 현재 trim·음소거·보내기 설정을 유지합니다. 정지 후 새 Scene 저장으로 보존할 수 있습니다."
                }.into();
                Ok(())
            }
            Err(error) => self.fail("AUX 변경을 완료하지 못했습니다.", error),
        }
    }

    /// Prepare and retire playback streams outside mutable GPUI updates.
    pub fn set_output_bindings(&mut self, bindings: Vec<OutputBinding>) -> Result<(), String> {
        let Some(active) = &mut self.active else {
            return self.fail("오디오 실행 중에만 출력을 변경할 수 있습니다.", "audio is not running");
        };
        let previous = active.output_bindings().to_vec();
        let result = active.set_output_bindings(bindings);
        self.timing = active.timing.clone();
        if active.output_bindings() != previous {
            // Shared endpoint regrouping can also replace a stream whose own mix
            // binding is unchanged. Clear only the cached output measurement interval.
            if let Some(levels) = &mut self.levels {
                levels.outputs.fill(None);
                levels.limiters.fill(None);
            }
            self.report.clear();
        }
        match result {
            Ok(()) => {
                self.error = None;
                self.message = "출력 설정 적용 완료 · 현재 믹서·소스·보컬 설정을 유지합니다. 정지 후 새 Scene 저장으로 보존할 수 있습니다.".into();
                Ok(())
            }
            Err(error) => self.fail("출력 변경을 완료하지 못했습니다.", error),
        }
    }

    /// Apply one coherent pitch configuration outside mutable GPUI updates.
    pub fn set_pitch_config(&mut self, config: PitchConfig) -> Result<(), String> {
        let Some(active) = &mut self.active else {
            return self.fail("오디오 실행 중에만 음정 설정을 적용할 수 있습니다.", "audio is not running");
        };
        let previous = active.pitch_config();
        let result = active.set_pitch_config(config);
        self.timing = active.timing.clone();
        if active.pitch_config() != previous {
            // A cached reading includes the old scale target. Refresh it before showing
            // readings under the newly acknowledged settings, even after a later error.
            if let Some(levels) = &mut self.levels { levels.pitch = None; }
            self.report.clear();
        }
        match result {
            Ok(()) => {
                self.error = None;
                self.message = "음정 설정 적용 · 정지 후 새 Scene 저장으로 현재 설정을 보존할 수 있습니다.".into();
                Ok(())
            }
            Err(error) => self.fail("음정 설정을 적용하지 못했습니다.", error),
        }
    }

    /// Apply only the three prepared vocal settings, keeping the live fader targets.
    pub fn set_vocal_settings(&mut self, settings: VocalSettings) -> Result<(), String> {
        let Some(active) = &mut self.active else {
            return self.fail("오디오 실행 중에만 보컬 설정을 적용할 수 있습니다.", "audio is not running");
        };
        let previous = active.vocal_settings();
        let result = active.set_vocal_settings(settings);
        self.timing = active.timing.clone();
        if active.vocal_settings() != previous { self.report.clear(); }
        match result {
            Ok(()) => {
                self.error = None;
                self.message = "보컬 설정 적용 · 정지 후 새 Scene 저장으로 현재 설정을 보존할 수 있습니다.".into();
                Ok(())
            }
            Err(error) => self.fail("보컬 설정을 적용하지 못했습니다.", error),
        }
    }

    /// Native controller access must stay outside App/entity update, like original editors.
    pub fn effect_parameters(&mut self, index: usize) -> Result<Vec<LiveParameter>, String> {
        let Some(active) = &mut self.active else {
            return self.fail("오디오 시작 후 파라미터를 확인할 수 있습니다.", "audio is not running");
        };
        match active.effect_parameters(index) {
            Ok(parameters) => {
                self.error = None;
                self.message = format!("VST3 {} 파라미터 조회 · 오디오 실행 중", index + 1);
                Ok(parameters)
            }
            Err(error) => self.fail("VST3 파라미터를 읽지 못했습니다.", error),
        }
    }

    /// Submit exactly one explicit edit and read back the same active instance.
    pub fn set_effect_parameter(&mut self, index: usize, id: u32, normalized: f64) -> Result<Vec<LiveParameter>, String> {
        let Some(active) = &mut self.active else {
            return self.fail("오디오 실행 중에만 파라미터를 적용할 수 있습니다.", "audio is not running");
        };
        match active.set_effect_parameter(index, id, normalized) {
            Ok(parameters) => {
                self.error = None;
                self.message = format!("VST3 {} 파라미터 적용 · 오디오 실행 중", index + 1);
                Ok(parameters)
            }
            Err(error) => self.fail("VST3 파라미터 적용 또는 결과 조회 중 오류가 발생했습니다. 새로고침으로 값을 확인하세요.", error),
        }
    }

    /// Call outside a mutable GPUI update. An unavailable editor is not an audio-path fault.
    pub fn set_editor_open(&mut self, index: usize, open: bool) -> Result<(), String> {
        let Some(active) = &mut self.active else {
            return self.fail("오디오 시작 후 원본 창을 열 수 있습니다.", "audio is not running");
        };
        match active.set_editor_open(index, open) {
            Ok(()) => {
                self.error = None;
                self.message = format!("VST3 {} 원본 창 {} · 오디오 실행 중", index + 1,
                    if open { "열림" } else { "닫힘" });
                Ok(())
            }
            Err(error) => self.fail("VST3 원본 창을 변경하지 못했습니다.", error),
        }
    }

    /// Native editor servicing can resize/detach windows. Call outside mutable GPUI updates.
    /// GPUI owns message dispatch; there is no Win32 pump here.
    pub fn poll(&mut self) -> Result<bool, String> {
        let Some(active) = &mut self.active else {
            return Ok(false);
        };
        let dismissed = match active.poll_plugin() {
            Ok(dismissed) => dismissed,
            Err(error) => return self.fail("오디오 경로 오류 · 정지 중입니다.", error),
        };
        if let Some(error) = active.health.error() {
            return self.fail("오디오 경로 오류 · 정지 중입니다.", error);
        }
        let now = Instant::now();
        let meter_updated = now.duration_since(self.last_meter) >= Duration::from_millis(100);
        if meter_updated {
            let levels = active.take_levels();
            self.levels = Some(levels);
            self.last_meter = now;
            if now.duration_since(self.last_report) >= Duration::from_secs(1) {
                self.report = active.report(&levels);
                self.last_report = now;
            }
        }
        if dismissed {
            self.message = "VST3 원본 창 닫힘 · 오디오 실행 중".into();
        }
        Ok(dismissed || meter_updated)
    }

    /// Explicitly finish and save a new Scene. Failure or an occupied path keeps the old selection.
    /// Like start/stop, call outside a mutable GPUI update.
    pub fn stop_and_save(&mut self, path: PathBuf) -> Result<(), String> {
        let Some(mut active) = self.active.take() else {
            return self.fail("실행 중인 Scene이 없습니다.", "audio is not running");
        };
        self.live_effects.clear();
        let route = self.scene().expect("active session has a Scene").route.clone();
        let levels = active.take_levels();
        self.report = active.report(&levels);
        self.levels = None;
        let scene = match active.finish_scene(route) {
            Ok(scene) => scene,
            Err(error) => return self.fail("오디오를 정지했지만 실행 상태를 저장하지 못했습니다.", error),
        };
        self.save_new(path, scene)?;
        self.message = "정지 후 현재 AUX·앱 소스·출력·믹서·보컬 설정과 모든 VST3 상태를 새 Scene에 저장했습니다.".into();
        Ok(())
    }

    /// Call outside a mutable GPUI update. All streams stop before the worker and plugins drop.
    pub fn stop(&mut self) -> Result<(), String> {
        let Some(mut active) = self.active.take() else {
            return Ok(());
        };
        self.live_effects.clear();
        let levels = active.take_levels();
        self.report = active.report(&levels);
        self.levels = None;
        let result = active.finish(None).map_err(|error| error.to_string());
        match result {
            Ok(()) => {
                self.error = None;
                self.message = "정지됨 · 다시 시작하면 저장된 Scene 설정을 사용합니다.".into();
                Ok(())
            }
            Err(error) => {
                // Keep the specific first failure (for example a plugin slot/restart reason).
                if self.error.is_none() {
                    self.error = Some(error.clone());
                }
                self.message = "오류로 정지했습니다. 장치와 플러그인을 확인하세요.".into();
                Err(error)
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Unexpected view/app teardown still releases the existing engine on its owner thread.
        // No state save or background shutdown is introduced by the GUI.
        drop(self.active.take());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use mixlive_audio_windows::{
        config::{Cli, Command},
        control::MuteTarget,
        plugin::EffectSpec,
    };

    #[test]
    fn opening_a_scene_never_opens_devices_or_plugins_and_bad_selection_clears_it() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("노래.mixlive-scene.json");
        let cli = Cli::parse_from([
            "mixlive-audio",
            "check",
            "--input",
            "not-a-device",
            "--output",
            "also-not-a-device",
            "--app",
            "C:\\Apps\\browser.exe",
            "--aux-input",
            "not-an-aux-device",
            "--aux-to",
            "off",
            "--aux-channels",
            "2,1",
        ]);
        let Command::Check(route) = cli.command else {
            panic!("expected route")
        };
        AudioScene::new(
            route,
            vec![EffectSpec {
                plugin: directory.path().join("missing.vst3"),
                class_id: None,
                state: None,
                bypassed: false,
            }],
        )
        .unwrap()
        .save_new(&path)
        .unwrap();
        let original = std::fs::read(&path).unwrap();
        let mut session = Session::default();
        session.open(path.clone()).unwrap();
        assert!(!session.running());
        assert!(session.controls().is_none());
        assert_eq!(session.path(), Some(path.as_path()));
        assert_eq!(session.scene().unwrap().effects.len(), 1);
        let pitch = session.scene().unwrap().route.pitch_config();
        assert_eq!(session.pitch_config(), Some(pitch));
        assert!(session.set_pitch_config(PitchConfig { strength: 0.75, ..pitch }).is_err());
        assert_eq!(session.pitch_config(), Some(pitch));
        assert_eq!(session.scene().unwrap().route.pitch_config(), pitch);
        let vocal = session.scene().unwrap().route.vocal_config().settings();
        assert_eq!(session.vocal_settings(), Some(vocal));
        assert!(session.set_vocal_settings(VocalSettings { echo_time_ms: 450.0, ..vocal }).is_err());
        assert_eq!(session.vocal_settings(), Some(vocal));
        assert_eq!(session.scene().unwrap().route.vocal_config().settings(), vocal);
        assert_eq!(session.effects(), session.scene().unwrap().effects.as_slice());
        assert!(session.edit_chain(ChainEdit::Remove { index: 0 }).is_err());
        assert_eq!(session.effects().len(), 1);
        assert_eq!(session.apps(), session.scene().unwrap().route.apps.as_slice());
        assert_eq!(session.apps().len(), 1);
        let apps = session.apps().to_vec();
        assert!(session.add_app(AppSource::new("C:\\Apps\\player.exe".into())).is_err());
        assert!(session.remove_app(0).is_err());
        assert_eq!(session.apps(), apps);
        let aux = session.aux_source().unwrap();
        assert_eq!(aux.input, "not-an-aux-device");
        assert_eq!(aux.channels, [2, 1]);
        assert!(session.set_aux_source(None).is_err());
        assert!(session.set_aux_source(Some(AuxSource { input: "other-device".into(), channels: vec![1] })).is_err());
        assert_eq!(session.aux_source(), Some(aux));
        let outputs = session.output_bindings();
        assert_eq!(outputs, session.scene().unwrap().route.output_bindings());
        let mut changed = outputs.clone();
        changed[0].output = "other-output".into();
        changed[0].channels = vec![2, 1];
        assert!(session.set_output_bindings(changed).is_err());
        assert_eq!(session.output_bindings(), outputs);
        assert!(session.device_details.is_empty());
        assert!(session.report.is_empty());
        session.poll().unwrap();
        session.stop().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert!(session.open(directory.path().join("absent.json")).is_err());
        assert!(session.scene().is_none());
        assert!(session.pitch_config().is_none());
        assert!(session.path().is_none());
        assert!(session.vocal_settings().is_none());
        assert!(session.apps().is_empty());
        assert!(session.aux_source().is_none());
        assert!(session.output_bindings().is_empty());
        assert!(session.error.is_some());
    }

    #[test]
    fn start_and_live_controls_require_an_explicit_loaded_running_session() {
        let mut session = Session::default();
        assert!(session.start().is_err());
        assert!(!session.running());
        assert!(session.pitch_config().is_none());
        assert!(session.set_pitch_config(PitchConfig::default()).is_err());
        assert!(session.vocal_settings().is_none());
        assert!(session.set_vocal_settings(mixlive_effects::VocalConfig::default().settings()).is_err());
        assert!(
            session
                .apply(LiveCommand::Mute {
                    target: MuteTarget::All,
                    muted: true
                })
                .is_err()
        );
        assert!(session.controls().is_none());
        assert!(!session.editor_is_open(0));
        assert!(session.set_editor_open(0, true).is_err());
        assert!(session.effect_parameters(0).is_err());
        assert!(session.set_effect_parameter(0, 0, 0.5).is_err());
        assert!(session.edit_chain(ChainEdit::Remove { index: 0 }).is_err());
        assert!(session.effects().is_empty());
        assert!(session.apps().is_empty());
        assert!(session.add_app(AppSource::new("not-running.exe".into())).is_err());
        assert!(session.remove_app(0).is_err());
        assert!(session.aux_source().is_none());
        assert!(session.set_aux_source(Some(AuxSource { input: "not-running".into(), channels: vec![1, 2] })).is_err());
        assert!(session.output_bindings().is_empty());
        assert!(session.set_output_bindings(Vec::new()).is_err());
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("no-live-scene.json");
        assert!(session.stop_and_save(path.clone()).is_err());
        assert!(!path.exists());
        session.stop().unwrap();
        assert!(!session.running());
    }

    #[test]
    fn saving_selects_a_new_scene_without_starting_and_collision_keeps_the_previous_scene() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("new.mixlive-scene.json");
        let cli = Cli::parse_from([
            "mixlive-audio",
            "check",
            "--input",
            "absent-mic",
            "--output",
            "absent-output",
        ]);
        let Command::Check(route) = cli.command else {
            panic!("expected route")
        };
        let mut session = Session::default();
        session
            .save_new(
                path.clone(),
                AudioScene::new(route.clone(), vec![]).unwrap(),
            )
            .unwrap();
        assert_eq!(session.path(), Some(path.as_path()));
        assert_eq!(session.scene().unwrap().route, route);
        assert!(!session.running());
        assert!(session.controls().is_none());
        assert!(session.device_details.is_empty());
        let original = std::fs::read(&path).unwrap();

        let occupied = directory.path().join("existing.json");
        std::fs::write(&occupied, b"do not replace").unwrap();
        let mut different = route.clone();
        different.input = "different-absent-mic".into();
        assert!(
            session
                .save_new(
                    occupied.clone(),
                    AudioScene::new(different, vec![]).unwrap()
                )
                .is_err()
        );
        assert_eq!(std::fs::read(&occupied).unwrap(), b"do not replace");
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert_eq!(session.path(), Some(path.as_path()));
        assert_eq!(session.scene().unwrap().route, route);
        assert!(!session.running());
    }
}
