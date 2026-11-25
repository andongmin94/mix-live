//! One UI-owned session. All native calls stay on GPUI's foreground/loading thread.
//! Scene reads/writes do not resolve endpoints or load plugins. Existing files are never replaced.

use mixlive_audio_windows::{
    control::{LiveCommand, LiveControls},
    plugin::PluginArgs,
    scene::AudioScene,
    windows::{Active, LevelReadings, Prepared},
};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub struct Session {
    loaded: Option<(PathBuf, AudioScene)>,
    active: Option<Active>,
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

    pub fn controls(&self) -> Option<&LiveControls> {
        self.active.as_ref().map(|active| active.controls.as_ref())
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
                self.last_report = Instant::now();
                self.last_meter = Instant::now();
                self.active = Some(active);
                self.message = "실행 중 · 정지 후 새 Scene 저장으로 현재 믹서·보컬 설정과 VST3 상태를 저장할 수 있습니다.".into();
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
        let Some(active) = self.active.take() else {
            return self.fail("실행 중인 Scene이 없습니다.", "audio is not running");
        };
        let route = self.scene().expect("active session has a Scene").route.clone();
        self.report = active.report(&active.take_levels());
        self.levels = None;
        let scene = match active.finish_scene(route) {
            Ok(scene) => scene,
            Err(error) => return self.fail("오디오를 정지했지만 실행 상태를 저장하지 못했습니다.", error),
        };
        self.save_new(path, scene)?;
        self.message = "정지 후 현재 믹서·보컬 설정과 모든 VST3 상태를 새 Scene에 저장했습니다.".into();
        Ok(())
    }

    /// Call outside a mutable GPUI update. All streams stop before the worker and plugins drop.
    pub fn stop(&mut self) -> Result<(), String> {
        let Some(active) = self.active.take() else {
            return Ok(());
        };
        self.report = active.report(&active.take_levels());
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
        assert!(session.device_details.is_empty());
        assert!(session.report.is_empty());
        session.poll().unwrap();
        session.stop().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert!(session.open(directory.path().join("absent.json")).is_err());
        assert!(session.scene().is_none());
        assert!(session.path().is_none());
        assert!(session.error.is_some());
    }

    #[test]
    fn start_and_live_controls_require_an_explicit_loaded_running_session() {
        let mut session = Session::default();
        assert!(session.start().is_err());
        assert!(!session.running());
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
