//! Editable startup vocal settings and ordered VST3 slots. No device or native plugin access.

use std::path::{Path, PathBuf};

use mixlive_audio_windows::{
    config::{PitchScale, RouteOptions},
    plugin::{EffectSpec, PluginArgs},
    scene::AudioScene,
};
use mixlive_effects::{DynamicsConfig, EchoConfig, EqConfig, Preset, ReverbConfig, VocalConfig};

#[derive(Clone, Debug)]
pub struct SoundDraft {
    pub preset: String,
    pub pitch_key: String,
    pub pitch_scale: String,
    pub input_gain_db: String,
    pub strength: String,
    pub compressor_threshold_db: String,
    pub deess_threshold_db: String,
    pub eq_hpf_hz: String,
    pub eq_body_db: String,
    pub eq_presence_db: String,
    pub eq_air_db: String,
    pub compressor_ratio: String,
    pub compressor_attack_ms: String,
    pub compressor_release_ms: String,
    pub deess_max_db: String,
    pub reverb_mix: String,
    pub reverb_decay_secs: String,
    pub reverb_predelay_ms: String,
    pub echo_mix: String,
    pub echo_time_ms: String,
    pub echo_feedback: String,
    pub effects: Vec<EffectSpec>,
}

impl Default for SoundDraft {
    fn default() -> Self {
        Self::from_config(VocalConfig::default(), Vec::new())
    }
}

impl SoundDraft {
    pub fn from_scene(scene: &AudioScene) -> Self {
        let mut draft = Self::from_config(scene.route.vocal_config(), scene.effects.clone());
        draft.pitch_key = scene.route.pitch_key.to_string();
        draft.pitch_scale = scene.route.pitch_scale.name().into();
        draft
    }

    pub fn from_config(config: VocalConfig, effects: Vec<EffectSpec>) -> Self {
        Self {
            preset: config.preset.to_string(),
            pitch_key: "0".into(),
            pitch_scale: "chromatic".into(),
            input_gain_db: config.input_gain_db.to_string(),
            strength: config.strength.to_string(),
            compressor_threshold_db: config.compressor_threshold_db.to_string(),
            deess_threshold_db: config.deess_threshold_db.to_string(),
            eq_hpf_hz: config.eq.hpf_hz.to_string(),
            eq_body_db: config.eq.body_db.to_string(),
            eq_presence_db: config.eq.presence_db.to_string(),
            eq_air_db: config.eq.air_db.to_string(),
            compressor_ratio: config.dynamics.ratio.to_string(),
            compressor_attack_ms: config.dynamics.attack_ms.to_string(),
            compressor_release_ms: config.dynamics.release_ms.to_string(),
            deess_max_db: config.dynamics.deess_max_db.to_string(),
            reverb_mix: config.reverb.mix.to_string(),
            reverb_decay_secs: config.reverb.decay_secs.to_string(),
            reverb_predelay_ms: config.reverb.predelay_ms.to_string(),
            echo_mix: config.echo.mix.to_string(),
            echo_time_ms: config.echo.time_ms.to_string(),
            echo_feedback: config.echo.feedback.to_string(),
            effects,
        }
    }

    pub fn pitch_reference(&self) -> Result<(u8, PitchScale), String> {
        let key = self.pitch_key.parse::<u8>().ok().filter(|key| *key < 12).ok_or("음정 기준 조성을 선택하세요.")?;
        let scale = <PitchScale as clap::ValueEnum>::from_str(&self.pitch_scale, false).map_err(|_| "음정 기준 스케일을 선택하세요.")?;
        Ok((key, scale))
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
            eq: EqConfig {
                hpf_hz: number(&self.eq_hpf_hz, "하이패스 주파수")?,
                body_db: number(&self.eq_body_db, "저역 EQ 보정")?,
                presence_db: number(&self.eq_presence_db, "존재감 EQ 보정")?,
                air_db: number(&self.eq_air_db, "고역 EQ 보정")?,
            },
            dynamics: DynamicsConfig {
                ratio: number(&self.compressor_ratio, "컴프레서 ratio")?,
                attack_ms: number(&self.compressor_attack_ms, "컴프레서 attack")?,
                release_ms: number(&self.compressor_release_ms, "컴프레서 release")?,
                deess_max_db: number(&self.deess_max_db, "디에서 최대 감쇠")?,
            },
            reverb: ReverbConfig {
                mix: number(&self.reverb_mix, "리버브 비율")?,
                decay_secs: number(&self.reverb_decay_secs, "잔향 시간")?,
                predelay_ms: number(&self.reverb_predelay_ms, "pre-delay")?,
            },
            echo: EchoConfig {
                mix: number(&self.echo_mix, "에코 비율")?,
                time_ms: number(&self.echo_time_ms, "에코 시간")?,
                feedback: number(&self.echo_feedback, "에코 피드백")?,
            },
        };
        config.validate().map_err(|error| error.to_string())?;
        Ok(config)
    }

    /// Preserve every device, channel, queue and mix setting while editing only the vocal chain.
    /// AudioScene performs the same file validation as CLI save; start rechecks actual devices.
    pub fn apply_to(&self, route: &RouteOptions) -> Result<AudioScene, String> {
        let config = self.vocal_config()?;
        let mut route = route.clone();
        route.set_vocal_config(config);
        (route.pitch_key, route.pitch_scale) = self.pitch_reference()?;
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
            "--pitch-key", "9", "--pitch-scale", "minor",
            "--vocal-strength",
            "0.625",
            "--vocal-threshold-db",
            "-23.75",
            "--deess-threshold-db",
            "-29.125",
            "--reverb-mix", "0.2",
            "--reverb-decay-secs", "1.75",
            "--reverb-predelay-ms", "22.5",
            "--echo-mix", "0.125", "--echo-time-ms", "300", "--echo-feedback", "0.25",
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
                bypassed: false,
            },
            EffectSpec {
                plugin: PathBuf::from("not-installed.vst3"),
                class_id: Some("FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF".into()),
                state: Some(vec![4, 5]),
                bypassed: true,
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
        draft.pitch_key = "2".into();
        draft.pitch_scale = "major".into();
        draft.input_gain_db = " -3.25 ".into();
        draft.strength = "0.375".into();
        draft.compressor_threshold_db = "-18.5".into();
        draft.deess_threshold_db = "-32.5".into();
        draft.eq_hpf_hz = "95".into();
        draft.eq_body_db = "1.5".into();
        draft.eq_presence_db = "-2".into();
        draft.eq_air_db = "2.5".into();
        draft.compressor_ratio = "3.5".into();
        draft.compressor_attack_ms = "5".into();
        draft.compressor_release_ms = "220".into();
        draft.deess_max_db = "7".into();
        draft.reverb_mix = "0.375".into();
        draft.reverb_decay_secs = "2.5".into();
        draft.reverb_predelay_ms = "7.5".into();
        draft.echo_mix = "0.375".into();
        draft.echo_time_ms = "420".into();
        draft.echo_feedback = "0.625".into();
        let changed = draft.apply_to(&original.route).unwrap();
        let mut expected = original.route.clone();
        expected.vocal = Preset::Bright;
        expected.pitch_key = 2;
        expected.pitch_scale = PitchScale::Major;
        expected.input_gain_db = -3.25;
        expected.vocal_strength = 0.375;
        expected.vocal_threshold_db = -18.5;
        expected.deess_threshold_db = -32.5;
        expected.eq_hpf_hz = 95.0;
        expected.eq_body_db = 1.5;
        expected.eq_presence_db = -2.0;
        expected.eq_air_db = 2.5;
        expected.compressor_ratio = 3.5;
        expected.compressor_attack_ms = 5.0;
        expected.compressor_release_ms = 220.0;
        expected.deess_max_db = 7.0;
        expected.reverb_mix = 0.375;
        expected.reverb_decay_secs = 2.5;
        expected.reverb_predelay_ms = 7.5;
        expected.echo_mix = 0.375;
        expected.echo_time_ms = 420.0;
        expected.echo_feedback = 0.625;
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
            r#"{"schema":2,"effects":[
            {"plugin":"relative.vst3","class_id":"0123456789ABCDEF0123456789abcdef","state":[7,8],"bypassed":false},
            {"plugin":"relative.vst3","class_id":"FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF","state":[9],"bypassed":true}
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
        assert!(!draft.effects[1].bypassed && draft.effects[2].bypassed);
        assert!(!expected_path.exists());

        let before = draft.effects.clone();
        for invalid in [
            r#"{"schema":99,"effects":[{"plugin":"x.vst3"}]}"#,
            r#"{"schema":2,"effects":[{"plugin":"x.vst3","state":[1],"bypassed":false}]}"#,
            r#"{"schema":2,"effects":[{"plugin":"x.vst3","bypassed":false},{"plugin":"","bypassed":false}]}"#,
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
        draft.pitch_key = "12".into();
        assert!(draft.apply_to(&original.route).is_err());
        draft.pitch_key = "0".into();
        draft.pitch_scale = "invalid".into();
        assert!(draft.apply_to(&original.route).is_err());
        draft.pitch_scale = "chromatic".into();
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
        draft.deess_threshold_db = "-30".into();
        draft.eq_hpf_hz = "201".into();
        assert!(draft.vocal_config().is_err());
        draft.eq_hpf_hz = "75".into();
        draft.eq_body_db = "6.1".into();
        assert!(draft.vocal_config().is_err());
        draft.eq_body_db = "0".into();
        draft.eq_presence_db = "-6.1".into();
        assert!(draft.vocal_config().is_err());
        draft.eq_presence_db = "0".into();
        draft.eq_air_db = "6.1".into();
        assert!(draft.vocal_config().is_err());
        draft.eq_air_db = "0".into();
        draft.compressor_ratio = "0.9".into();
        assert!(draft.vocal_config().is_err());
        draft.compressor_ratio = "2.5".into();
        draft.compressor_attack_ms = "0".into();
        assert!(draft.vocal_config().is_err());
        draft.compressor_attack_ms = "8".into();
        draft.compressor_release_ms = "501".into();
        assert!(draft.vocal_config().is_err());
        draft.compressor_release_ms = "140".into();
        draft.deess_max_db = "12.1".into();
        assert!(draft.vocal_config().is_err());
        draft.deess_max_db = "4".into();
        draft.reverb_mix = "1.01".into();
        assert!(draft.vocal_config().is_err());
        draft.reverb_mix = "0.25".into();
        draft.reverb_decay_secs = "NaN".into();
        assert!(draft.vocal_config().is_err());
        draft.reverb_decay_secs = "1.25".into();
        draft.reverb_predelay_ms = "81".into();
        assert!(draft.vocal_config().is_err());
        draft.reverb_predelay_ms = "15".into();
        draft.echo_mix = "1.01".into();
        assert!(draft.vocal_config().is_err());
        draft.echo_mix = "0.25".into();
        draft.echo_time_ms = "9".into();
        assert!(draft.vocal_config().is_err());
        draft.echo_time_ms = "250".into();
        draft.echo_feedback = "0.9".into();
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
