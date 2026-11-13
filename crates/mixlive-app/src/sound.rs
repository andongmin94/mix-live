//! Editable startup vocal settings and ordered VST3 slots. No device or native plugin access.

use std::path::{Path, PathBuf};

use mixlive_audio_windows::{
    config::RouteOptions,
    plugin::{EffectSpec, PluginArgs},
    scene::AudioScene,
};
use mixlive_effects::{Preset, VocalConfig};

#[derive(Clone, Debug)]
pub struct SoundDraft {
    pub preset: String,
    pub input_gain_db: String,
    pub strength: String,
    pub compressor_threshold_db: String,
    pub deess_threshold_db: String,
    pub effects: Vec<EffectSpec>,
}

impl Default for SoundDraft {
    fn default() -> Self {
        Self::from_config(VocalConfig::default(), Vec::new())
    }
}

impl SoundDraft {
    pub fn from_scene(scene: &AudioScene) -> Self {
        Self::from_config(scene.route.vocal_config(), scene.effects.clone())
    }

    pub fn from_config(config: VocalConfig, effects: Vec<EffectSpec>) -> Self {
        Self {
            preset: config.preset.to_string(),
            input_gain_db: config.input_gain_db.to_string(),
            strength: config.strength.to_string(),
            compressor_threshold_db: config.compressor_threshold_db.to_string(),
            deess_threshold_db: config.deess_threshold_db.to_string(),
            effects,
        }
    }

    pub fn vocal_config(&self) -> Result<VocalConfig, String> {
        let config = VocalConfig {
            preset: self
                .preset
                .trim()
                .parse::<Preset>()
                .map_err(|error| error.to_string())?,
            input_gain_db: number(&self.input_gain_db, "입력 trim")?,
            strength: number(&self.strength, "보정 비율")?,
            compressor_threshold_db: number(&self.compressor_threshold_db, "컴프레서 threshold")?,
            deess_threshold_db: number(&self.deess_threshold_db, "디에서 threshold")?,
        };
        config.validate().map_err(|error| error.to_string())?;
        Ok(config)
    }

    /// Preserve every device, channel, queue and mix setting while editing only the vocal chain.
    /// AudioScene performs the same file validation as CLI save; start rechecks actual devices.
    pub fn apply_to(&self, route: &RouteOptions) -> Result<AudioScene, String> {
        let config = self.vocal_config()?;
        let mut route = route.clone();
        route.vocal = config.preset;
        route.input_gain_db = config.input_gain_db;
        route.vocal_strength = config.strength;
        route.vocal_threshold_db = config.compressor_threshold_db;
        route.deess_threshold_db = config.deess_threshold_db;
        AudioScene::new(route, self.effects.clone()).map_err(|error| error.to_string())
    }

    /// Repeated paths intentionally remain separate slots. Resolve all additions before mutation.
    pub fn append_plugins(&mut self, paths: Vec<PathBuf>) -> Result<(), String> {
        let additions = PluginArgs {
            vst3: paths,
            ..Default::default()
        }
        .resolve()?
        .effects;
        self.effects.extend(additions);
        Ok(())
    }

    /// Append the existing chain format, including its classes, states and anchored relative paths.
    pub fn append_chain(&mut self, path: &Path) -> Result<(), String> {
        let additions = PluginArgs {
            vst3_chain: Some(path.to_owned()),
            ..Default::default()
        }
        .resolve()?
        .effects;
        self.effects.extend(additions);
        Ok(())
    }

    pub fn move_effect(&mut self, index: usize, to: usize) -> Result<(), String> {
        if index >= self.effects.len() || to >= self.effects.len() {
            return Err("이동할 VST3 슬롯이 없습니다.".into());
        }
        let effect = self.effects.remove(index);
        self.effects.insert(to, effect);
        Ok(())
    }

    pub fn remove_effect(&mut self, index: usize) -> Result<(), String> {
        if index >= self.effects.len() {
            return Err("제거할 VST3 슬롯이 없습니다.".into());
        }
        self.effects.remove(index);
        Ok(())
    }
}

fn number(value: &str, label: &str) -> Result<f32, String> {
    value
        .trim()
        .parse()
        .map_err(|_| format!("{label}에는 숫자를 입력하세요."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use mixlive_audio_windows::config::{Cli, Command};

    fn route() -> RouteOptions {
        let cli = Cli::parse_from([
            "mixlive-audio",
            "check",
            "--input",
            "not-installed-mic",
            "--input-channel",
            "2",
            "--aux-input",
            "not-installed-aux",
            "--aux-channels",
            "4,3",
            "--aux-gain-db",
            "-7.5",
            "--aux-to",
            "both",
            "--output",
            "not-installed-output",
            "--output-channels",
            "4,3",
            "--send",
            "not-installed-send",
            "--send-channels",
            "2,1",
            "--send-gain-db",
            "-8.5",
            "--input-gain-db",
            "2.375",
            "--output-gain-db",
            "-25.25",
            "--vocal",
            "warm",
            "--vocal-strength",
            "0.625",
            "--vocal-threshold-db",
            "-23.75",
            "--deess-threshold-db",
            "-29.125",
            "--queue-ms",
            "47",
        ]);
        let Command::Check(route) = cli.command else {
            panic!("expected check")
        };
        route
    }

    fn states() -> Vec<EffectSpec> {
        vec![
            EffectSpec {
                plugin: PathBuf::from("not-installed.vst3"),
                class_id: Some("0123456789ABCDEF0123456789abcdef".into()),
                state: Some(vec![1, 2, 3]),
            },
            EffectSpec {
                plugin: PathBuf::from("not-installed.vst3"),
                class_id: Some("FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF".into()),
                state: Some(vec![4, 5]),
            },
        ]
    }

    #[test]
    fn unchanged_sound_preserves_the_loaded_scene_and_vocal_edits_preserve_routing() {
        let original = AudioScene::new(route(), states()).unwrap();
        let mut draft = SoundDraft::from_scene(&original);
        let unchanged = draft.apply_to(&original.route).unwrap();
        assert_eq!(unchanged.route, original.route);
        assert_eq!(unchanged.effects, original.effects);

        draft.preset = "bright".into();
        draft.input_gain_db = " -3.25 ".into();
        draft.strength = "0.375".into();
        draft.compressor_threshold_db = "-18.5".into();
        draft.deess_threshold_db = "-32.5".into();
        let changed = draft.apply_to(&original.route).unwrap();
        let mut expected = original.route.clone();
        expected.vocal = Preset::Bright;
        expected.input_gain_db = -3.25;
        expected.vocal_strength = 0.375;
        expected.vocal_threshold_db = -18.5;
        expected.deess_threshold_db = -32.5;
        assert_eq!(changed.route, expected);
        assert_eq!(changed.effects, original.effects);
        assert_eq!(original.route, route());
    }

    #[test]
    fn duplicate_plugin_slots_keep_their_own_class_and_state_when_moved_or_removed() {
        let original = AudioScene::new(route(), states()).unwrap();
        let mut draft = SoundDraft::from_scene(&original);
        draft
            .append_plugins(vec![original.effects[0].plugin.clone()])
            .unwrap();
        assert_eq!(draft.effects.len(), 3);
        assert!(draft.effects[2].class_id.is_none());
        assert!(draft.effects[2].state.is_none());
        draft.move_effect(0, 2).unwrap();
        assert_eq!(draft.effects[0], original.effects[1]);
        assert_eq!(draft.effects[2], original.effects[0]);
        draft.remove_effect(1).unwrap();
        assert_eq!(
            draft.effects,
            [original.effects[1].clone(), original.effects[0].clone()]
        );

        let before = draft.effects.clone();
        assert!(draft.move_effect(2, 0).is_err());
        assert!(draft.move_effect(0, 2).is_err());
        assert!(draft.remove_effect(usize::MAX).is_err());
        assert_eq!(draft.effects, before);
        draft.remove_effect(1).unwrap();
        draft.remove_effect(0).unwrap();
        assert!(draft.apply_to(&original.route).unwrap().effects.is_empty());
    }

    #[test]
    fn chain_import_appends_anchored_slots_and_failed_import_is_atomic() {
        let directory = tempfile::tempdir().unwrap();
        let folder = directory.path().join("import");
        std::fs::create_dir(&folder).unwrap();
        let path = folder.join("voice.chain.json");
        std::fs::write(
            &path,
            r#"{"schema":1,"effects":[
            {"plugin":"relative.vst3","class_id":"0123456789ABCDEF0123456789abcdef","state":[7,8]},
            {"plugin":"relative.vst3","class_id":"FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF","state":[9]}
        ]}"#,
        )
        .unwrap();
        let mut draft = SoundDraft::default();
        draft
            .append_plugins(vec![PathBuf::from("existing.vst3")])
            .unwrap();
        let existing = draft.effects[0].clone();
        draft.append_chain(&path).unwrap();
        assert_eq!(draft.effects.len(), 3);
        assert_eq!(draft.effects[0], existing);
        let expected_path = folder.canonicalize().unwrap().join("relative.vst3");
        assert_eq!(draft.effects[1].plugin, expected_path);
        assert_eq!(draft.effects[2].plugin, expected_path);
        assert_eq!(draft.effects[1].state.as_deref(), Some([7, 8].as_slice()));
        assert_eq!(draft.effects[2].state.as_deref(), Some([9].as_slice()));
        assert!(!expected_path.exists());

        let before = draft.effects.clone();
        for invalid in [
            r#"{"schema":99,"effects":[{"plugin":"x.vst3"}]}"#,
            r#"{"schema":1,"effects":[{"plugin":"x.vst3","state":[1]}]}"#,
            r#"{"schema":1,"effects":[{"plugin":"x.vst3"},{"plugin":""}]}"#,
        ] {
            std::fs::write(&path, invalid).unwrap();
            assert!(draft.append_chain(&path).is_err());
            assert_eq!(draft.effects, before);
        }
        assert!(draft.append_chain(&folder.join("absent.json")).is_err());
        assert!(
            draft
                .append_plugins(vec![PathBuf::from("valid.vst3"), PathBuf::new()])
                .is_err()
        );
        assert_eq!(draft.effects, before);
    }

    #[test]
    fn invalid_vocal_edits_do_not_turn_into_defaults_or_alter_the_source_scene() {
        let original = AudioScene::new(route(), states()).unwrap();
        for invalid in ["", "NaN", "inf", "-0.1", "1.1", "many"] {
            let mut draft = SoundDraft::from_scene(&original);
            draft.strength = invalid.into();
            assert!(draft.apply_to(&original.route).is_err());
        }
        let mut draft = SoundDraft::from_scene(&original);
        draft.preset = "unknown".into();
        assert!(draft.vocal_config().is_err());
        draft.preset = "off".into();
        draft.input_gain_db = "19".into();
        assert!(draft.vocal_config().is_err());
        draft.input_gain_db = "0".into();
        draft.compressor_threshold_db = "-61".into();
        assert!(draft.vocal_config().is_err());
        draft.compressor_threshold_db = "-24".into();
        draft.deess_threshold_db = "1".into();
        assert!(draft.vocal_config().is_err());
        assert_eq!(original.route, route());
    }

    #[test]
    fn edited_sound_round_trips_through_scene_without_loading_missing_devices_or_plugins() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("edited.mixlive-scene.json");
        let original = AudioScene::new(route(), states()).unwrap();
        let mut draft = SoundDraft::from_scene(&original);
        draft.preset = "natural".into();
        draft.strength = "0.5".into();
        draft.move_effect(1, 0).unwrap();
        let edited = draft.apply_to(&original.route).unwrap();
        edited.save_new(&path).unwrap();
        let loaded = AudioScene::read(&path).unwrap();
        assert_eq!(loaded.route, edited.route);
        assert_eq!(loaded.effects, edited.effects);
        assert_eq!(loaded.effects[0], original.effects[1]);
        assert_eq!(loaded.effects[1], original.effects[0]);
        assert!(loaded.effects.iter().all(|effect| !effect.plugin.exists()));
    }
}
