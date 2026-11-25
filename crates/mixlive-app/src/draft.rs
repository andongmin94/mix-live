//! Editable startup devices and sound in the existing schema. This layer has no device I/O.

use mixlive_audio_windows::{
    config::{AuxDestination, InputMap, OutputMap, RouteOptions},
    control::MixSettings,
    devices::{DeviceCatalog, DeviceConfig, DeviceInfo},
    scene::AudioScene,
};
use mixlive_effects::VocalChain;

use crate::sound::SoundDraft;

pub struct SceneDraft {
    pub input: Option<String>,
    pub input_channel: String,
    pub output: Option<String>,
    pub output_channels: String,
    pub output_gain_db: String,
    pub send: Option<String>,
    pub send_channels: String,
    pub send_gain_db: String,
    pub aux_input: Option<String>,
    pub aux_app: Option<String>,
    pub aux_channels: String,
    pub aux_gain_db: String,
    pub aux_to: String,
    pub queue_ms: String,
    pub mix: MixSettings,
    pub sound: SoundDraft,
}

impl Default for SceneDraft {
    fn default() -> Self {
        Self {
            input: None,
            input_channel: "1".into(),
            output: None,
            output_channels: "1,2".into(),
            output_gain_db: "-18".into(),
            send: None,
            send_channels: "1,2".into(),
            send_gain_db: "0".into(),
            aux_input: None,
            aux_app: None,
            aux_channels: "1,2".into(),
            aux_gain_db: "-12".into(),
            aux_to: "send".into(),
            queue_ms: "30".into(),
            mix: MixSettings::default(),
            sound: SoundDraft::default(),
        }
    }
}

impl SceneDraft {
    /// Keep every saved setting, including channels and levels of disabled optional endpoints.
    /// Missing or unsupported IDs remain explicit; build never substitutes a different device.
    pub fn from_scene(scene: &AudioScene) -> Self {
        let route = &scene.route;
        let channel_text = |channels: &[u16]| {
            channels
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(",")
        };
        Self {
            input: Some(route.input.clone()),
            input_channel: route.input_channel.to_string(),
            output: Some(route.output.clone()),
            output_channels: channel_text(&route.output_channels),
            output_gain_db: route.output_gain_db.to_string(),
            send: route.send.clone(),
            send_channels: channel_text(&route.send_channels),
            send_gain_db: route.send_gain_db.to_string(),
            aux_input: route.aux_input.clone(),
            aux_app: route.aux_app.clone(),
            aux_channels: channel_text(&route.aux_channels),
            aux_gain_db: route.aux_gain_db.to_string(),
            aux_to: match route.aux_to {
                AuxDestination::Off => "off",
                AuxDestination::Output => "output",
                AuxDestination::Send => "send",
                AuxDestination::Both => "both",
            }
            .into(),
            queue_ms: route.queue_ms.to_string(),
            mix: route.mix,
            sound: SoundDraft::from_scene(scene),
        }
    }

    pub fn build(&self, catalog: &DeviceCatalog) -> Result<AudioScene, String> {
        let input = selected(&self.input, &catalog.inputs, "내 목소리 입력")?;
        let output = selected(&self.output, &catalog.outputs, "주 출력")?;
        let input_config = configuration(input)?;
        let output_config = configuration(output)?;
        let input_channel = self
            .input_channel
            .trim()
            .parse::<u16>()
            .map_err(|_| "마이크 채널은 1부터 시작하는 정수 하나를 입력하세요.".to_owned())?;
        if input_channel == 0 || input_channel > input_config.channels {
            return Err(format!(
                "마이크 채널은 선택한 장치의 1..={} 범위여야 합니다.",
                input_config.channels
            ));
        }
        let output_channels = channels(&self.output_channels)?;
        OutputMap::new(&output_channels, output_config.channels)?;
        let send_channels = channels(&self.send_channels)?;
        if self.send.is_some() {
            let send = selected(&self.send, &catalog.outputs, "추가 송출")?;
            OutputMap::new(&send_channels, configuration(send)?.channels)?;
        }
        let aux_channels = channels(&self.aux_channels)?;
        if self.aux_input.is_some() {
            let aux = selected(&self.aux_input, &catalog.inputs, "AUX / MR 입력")?;
            InputMap::new(&aux_channels, configuration(aux)?.channels)?;
        }
        if let Some(path) = &self.aux_app {
            if self.aux_input.is_some() {
                return Err("AUX 장치와 앱 중 하나만 선택하세요.".into());
            }
            let app = catalog.apps.iter().find(|app| &app.path == path)
                .ok_or("저장된 앱이 현재 실행 목록에 없습니다. 앱을 실행하고 장치 설정을 다시 여세요.")?;
            app.target.as_ref().map_err(Clone::clone)?;
            InputMap::new(&aux_channels, 2)?;
        }
        let aux_to = match self.aux_to.as_str() {
            "off" => AuxDestination::Off,
            "output" => AuxDestination::Output,
            "send" => AuxDestination::Send,
            "both" => AuxDestination::Both,
            _ => return Err("AUX를 보낼 목적지를 선택하세요.".into()),
        };
        if (self.aux_input.is_some() || self.aux_app.is_some()) && aux_to.includes(1) && self.send.is_none() {
            return Err("AUX를 추가 송출로 보내려면 추가 송출 장치를 선택하세요. 주 출력만 사용하려면 AUX 목적지를 주 출력으로 바꾸세요.".into());
        }
        let vocal = self.sound.vocal_config()?;
        let route =
            RouteOptions {
                input: input.id.clone(),
                input_channel,
                aux_input: self.aux_input.clone(),
                aux_app: self.aux_app.clone(),
                aux_channels,
                aux_gain_db: number(&self.aux_gain_db, "AUX 음량")?,
                aux_to,
                output: output.id.clone(),
                output_channels,
                send: self.send.clone(),
                send_channels,
                input_gain_db: vocal.input_gain_db,
                output_gain_db: number(&self.output_gain_db, "전체 출력 음량")?,
                send_gain_db: number(&self.send_gain_db, "추가 송출 감쇠")?,
                vocal: vocal.preset,
                vocal_strength: vocal.strength,
                vocal_threshold_db: vocal.compressor_threshold_db,
                deess_threshold_db: vocal.deess_threshold_db,
                mix: self.mix,
                queue_ms: self.queue_ms.trim().parse().map_err(|_| {
                    "Queue 목표에는 10~100 범위의 정수(ms)를 입력하세요.".to_owned()
                })?,
            };
        // Use the same coefficient/sample-rate validation as Prepared without opening devices.
        // Start rechecks the live endpoint configuration; the catalog is only a point-in-time view.
        VocalChain::new(input_config.sample_rate, route.vocal_config())
            .map_err(|error| error.to_string())?;
        AudioScene::new(route, self.sound.effects.clone()).map_err(|error| error.to_string())
    }
}

fn selected<'a>(
    id: &Option<String>,
    devices: &'a [DeviceInfo],
    label: &str,
) -> Result<&'a DeviceInfo, String> {
    let id = id
        .as_deref()
        .ok_or_else(|| format!("{label} 장치를 선택하세요."))?;
    devices
        .iter()
        .find(|device| device.id == id)
        .ok_or_else(|| {
            format!(
                "{label} 장치가 조회한 목록에 없습니다. 장치 설정을 다시 열어 장치를 확인하세요."
            )
        })
}

fn configuration(device: &DeviceInfo) -> Result<&DeviceConfig, String> {
    device
        .config
        .as_ref()
        .map_err(|error| format!("{}: {error}", device.name))
}

fn channels(text: &str) -> Result<Vec<u16>, String> {
    text.split(',')
        .map(|channel| {
            channel
                .trim()
                .parse::<u16>()
                .map_err(|_| "채널은 1 또는 1,2처럼 쉼표로 구분한 정수를 입력하세요.".to_owned())
        })
        .collect()
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
    use mixlive_audio_windows::{
        config::{Cli, Command},
        plugin::EffectSpec,
    };

    fn device(id: &str, channels: u16) -> DeviceInfo {
        DeviceInfo {
            id: id.into(),
            name: "Same display name".into(),
            config: Ok(DeviceConfig {
                channels,
                sample_rate: 48_000,
                sample_format: "f32".into(),
            }),
        }
    }

    fn catalog() -> DeviceCatalog {
        DeviceCatalog {
            inputs: vec![device("input-a", 4), device("input-b", 1)],
            outputs: vec![device("output-a", 4), device("output-b", 1)],
            apps: vec![mixlive_audio_windows::devices::AppInfo {
                path: "C:\\Apps\\browser.exe".into(),
                target: Ok(mixlive_audio_windows::devices::ProcessTarget { pid: 11, started: 1 }),
            }],
        }
    }

    fn draft() -> SceneDraft {
        SceneDraft {
            input: Some("input-a".into()),
            output: Some("output-a".into()),
            ..SceneDraft::default()
        }
    }

    fn saved_scene() -> AudioScene {
        let cli = Cli::parse_from([
            "mixlive-audio",
            "check",
            "--input",
            "input-a",
            "--input-channel",
            "2",
            "--aux-input",
            "input-a",
            "--aux-channels",
            "4,3",
            "--aux-gain-db",
            "-9.75",
            "--aux-to",
            "both",
            "--output",
            "output-a",
            "--output-channels",
            "4,3",
            "--send",
            "output-b",
            "--send-channels",
            "1",
            "--send-gain-db",
            "-6.5",
            "--output-gain-db",
            "-23.25",
            "--queue-ms",
            "47",
            "--vocal",
            "warm",
            "--input-gain-db",
            "2.375",
            "--vocal-strength",
            "0.625",
            "--vocal-threshold-db",
            "-23.75",
            "--deess-threshold-db",
            "-29.125",
        ]);
        let Command::Check(mut route) = cli.command else {
            panic!("expected check")
        };
        route.mix.mute_all = true;
        route.mix.mute_aux = true;
        route.mix.mic_output_enabled = false;
        route.mix.mic_output_gain_db = -19.25;
        route.mix.aux_send_gain_db = -6.5;
        AudioScene::new(
            route,
            vec![
                EffectSpec {
                    plugin: "not-installed.vst3".into(),
                    class_id: Some("0123456789ABCDEF0123456789ABCDEF".into()),
                    state: Some(vec![1, 2]),
                },
                EffectSpec {
                    plugin: "not-installed.vst3".into(),
                    class_id: Some("FEDCBA9876543210FEDCBA9876543210".into()),
                    state: Some(vec![3, 4]),
                },
            ],
        )
        .unwrap()
    }

    #[test]
    fn new_scene_uses_exact_ids_and_existing_cli_defaults() {
        assert!(SceneDraft::default().input.is_none());
        assert!(SceneDraft::default().output.is_none());
        let scene = draft().build(&catalog()).unwrap();
        let cli = Cli::parse_from([
            "mixlive-audio",
            "check",
            "--input",
            "input-a",
            "--output",
            "output-a",
        ]);
        let Command::Check(route) = cli.command else {
            panic!("expected check")
        };
        assert_eq!(scene.route, route);
        assert!(scene.effects.is_empty());

        let mut draft = draft();
        draft.input = Some("input-b".into());
        draft.output = Some("output-b".into());
        draft.output_channels = "1".into();
        let scene = draft.build(&catalog()).unwrap();
        assert_eq!(scene.route.input, "input-b");
        assert_eq!(scene.route.output, "output-b");
        assert_eq!(scene.route.output_channels, [1]);
    }

    #[test]
    fn application_aux_uses_an_explicit_running_tree_and_preserves_the_saved_path() {
        let mut draft = draft();
        draft.aux_app = Some("C:\\Apps\\browser.exe".into());
        draft.aux_to = "output".into();
        let mut devices = catalog();
        let scene = draft.build(&devices).unwrap();
        assert!(scene.route.aux_input.is_none());
        assert_eq!(scene.route.aux_app, draft.aux_app);
        let restored = SceneDraft::from_scene(&scene);
        devices.apps[0].target.as_mut().unwrap().pid = 22;
        assert_eq!(restored.build(&devices).unwrap().route, scene.route);
        devices.apps[0].target = Err("ambiguous process tree".into());
        assert!(restored.build(&devices).unwrap_err().contains("ambiguous"));
        devices.apps.clear();
        assert!(restored.build(&devices).is_err());
    }

    #[test]
    fn loaded_scene_preserves_all_fields_including_disabled_optional_settings() {
        for optional in [true, false] {
            let mut original = saved_scene();
            if !optional {
                original.route.send = None;
                original.route.aux_input = None;
                // Inactive mappings retain their shape without using another endpoint's range.
                original.route.send_channels = vec![21, 20];
                original.route.aux_channels = vec![18];
            }
            let restored = SceneDraft::from_scene(&original).build(&catalog()).unwrap();
            assert_eq!(restored.route, original.route);
            assert_eq!(restored.effects, original.effects);
        }
    }

    #[test]
    fn device_and_startup_level_edits_preserve_sound_and_slots_when_saved() {
        let original = saved_scene();
        let mut draft = SceneDraft::from_scene(&original);
        draft.input = Some("input-b".into());
        draft.input_channel = "1".into();
        draft.output = Some("output-b".into());
        draft.output_channels = "1".into();
        draft.send = Some("output-a".into());
        draft.send_channels = "3,4".into();
        draft.aux_input = Some("input-a".into());
        draft.aux_channels = "2,1".into();
        draft.aux_to = "output".into();
        draft.output_gain_db = " -24.5 ".into();
        draft.send_gain_db = "-4.25".into();
        draft.aux_gain_db = "-18.75".into();
        draft.queue_ms = " 35 ".into();
        let changed = draft.build(&catalog()).unwrap();
        let mut expected = original.route.clone();
        expected.input = "input-b".into();
        expected.input_channel = 1;
        expected.output = "output-b".into();
        expected.output_channels = vec![1];
        expected.send = Some("output-a".into());
        expected.send_channels = vec![3, 4];
        expected.aux_input = Some("input-a".into());
        expected.aux_channels = vec![2, 1];
        expected.aux_to = AuxDestination::Output;
        expected.output_gain_db = -24.5;
        expected.send_gain_db = -4.25;
        expected.aux_gain_db = -18.75;
        expected.queue_ms = 35;
        assert_eq!(changed.route, expected);
        assert_eq!(changed.effects, original.effects);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("edited.mixlive-scene.json");
        changed.save_new(&path).unwrap();
        let reopened = AudioScene::read(&path).unwrap();
        assert_eq!(reopened.route, expected);
        assert_eq!(reopened.effects, original.effects);
    }

    #[test]
    fn unavailable_saved_optional_devices_remain_explicit_until_changed() {
        let mut original = saved_scene();
        original.route.send = Some("missing-send".into());
        let mut draft = SceneDraft::from_scene(&original);
        assert!(
            draft
                .build(&catalog())
                .unwrap_err()
                .contains("추가 송출 장치가 조회한 목록에 없습니다")
        );
        assert_eq!(draft.send, original.route.send);
        draft.send = Some("input-b".into());
        assert!(
            draft
                .build(&catalog())
                .unwrap_err()
                .contains("추가 송출 장치가 조회한 목록에 없습니다")
        );
        draft.send = Some("output-b".into());
        let mut devices = catalog();
        devices.outputs[1].config = Err("unavailable playback configuration".into());
        assert!(
            draft
                .build(&devices)
                .unwrap_err()
                .contains("unavailable playback configuration")
        );
        draft.send = None;
        draft.aux_to = "output".into();
        draft.aux_input = Some("missing-aux".into());
        assert!(
            draft
                .build(&devices)
                .unwrap_err()
                .contains("AUX / MR 입력 장치가 조회한 목록에 없습니다")
        );
        assert_eq!(draft.aux_input.as_deref(), Some("missing-aux"));
        draft.aux_input = None;
        let disabled = draft.build(&devices).unwrap();
        assert!(disabled.route.send.is_none());
        assert!(disabled.route.aux_input.is_none());
        assert_eq!(disabled.effects, original.effects);
    }

    #[test]
    fn startup_levels_queue_and_inactive_channel_shapes_are_validated() {
        for invalid in ["", "bad", "NaN", "inf", "-inf", "0.1", "-96.1"] {
            let mut edited = draft();
            edited.output_gain_db = invalid.into();
            assert!(edited.build(&catalog()).is_err());
            edited = draft();
            edited.send_gain_db = invalid.into();
            assert!(edited.build(&catalog()).is_err());
            edited = draft();
            edited.aux_gain_db = invalid.into();
            assert!(edited.build(&catalog()).is_err());
        }
        for invalid in ["", "bad", "-1", "9", "101", "30.5"] {
            let mut edited = draft();
            edited.queue_ms = invalid.into();
            assert!(edited.build(&catalog()).is_err());
        }
        for invalid in ["0", "1,1", "1,2,3", "", "1,"] {
            let mut edited = draft();
            edited.send_channels = invalid.into();
            assert!(edited.build(&catalog()).is_err());
            edited = draft();
            edited.aux_channels = invalid.into();
            assert!(edited.build(&catalog()).is_err());
        }
        let mut edited = draft();
        edited.output_gain_db = "-96".into();
        edited.send_gain_db = "0".into();
        edited.aux_gain_db = "-96".into();
        edited.queue_ms = "10".into();
        assert!(edited.build(&catalog()).is_ok());
        edited.queue_ms = "100".into();
        assert!(edited.build(&catalog()).is_ok());
    }

    #[test]
    fn missing_wrong_direction_and_unsupported_devices_never_fall_back() {
        let mut draft = draft();
        for id in [
            None,
            Some("missing".into()),
            Some("output-a".into()),
            Some("Same display name".into()),
        ] {
            draft.input = id;
            assert!(draft.build(&catalog()).is_err());
        }
        draft.input = Some("input-a".into());
        let mut devices = catalog();
        devices.inputs[0].config = Err("unsupported sample format".into());
        assert!(
            draft
                .build(&devices)
                .unwrap_err()
                .contains("unsupported sample format")
        );
        devices.inputs[0].config = Ok(DeviceConfig {
            channels: 1,
            sample_rate: 16_000,
            sample_format: "f32".into(),
        });
        assert!(draft.build(&devices).is_ok());
        draft.sound.preset = "natural".into();
        assert!(draft.build(&devices).unwrap_err().contains("32000"));
    }

    #[test]
    fn channels_are_validated_against_the_selected_endpoint() {
        let mut draft = draft();
        for invalid in ["0", "5", "1,2", "", "-1"] {
            draft.input_channel = invalid.into();
            assert!(draft.build(&catalog()).is_err());
        }
        draft.input_channel = "1".into();
        for invalid in ["0", "5", "1,1", "1,2,3", "", "1,"] {
            draft.output_channels = invalid.into();
            assert!(draft.build(&catalog()).is_err());
        }
        draft.output_channels = " 4, 3 ".into();
        assert_eq!(
            draft.build(&catalog()).unwrap().route.output_channels,
            [4, 3]
        );
        draft.output = Some("output-b".into());
        assert!(draft.build(&catalog()).is_err());
    }

    #[test]
    fn shared_endpoints_require_disjoint_channels_and_preserve_stereo_order() {
        let mut draft = draft();
        draft.send = draft.output.clone();
        assert!(draft.build(&catalog()).unwrap_err().contains("overlap"));
        draft.send_channels = "4,3".into();
        draft.aux_input = draft.input.clone();
        assert!(draft.build(&catalog()).unwrap_err().contains("overlap"));
        draft.aux_channels = "4,3".into();
        let scene = draft.build(&catalog()).unwrap();
        assert_eq!(scene.route.send_channels, [4, 3]);
        assert_eq!(scene.route.aux_channels, [4, 3]);
        draft.send_channels = "5".into();
        assert!(draft.build(&catalog()).is_err());
        draft.send_channels = "4,3".into();
        draft.aux_channels = "5".into();
        assert!(draft.build(&catalog()).is_err());
    }

    #[test]
    fn aux_requires_its_selected_destination_and_scene_round_trips() {
        let mut draft = draft();
        draft.aux_input = Some("input-b".into());
        draft.aux_channels = "1".into();
        assert!(draft.build(&catalog()).unwrap_err().contains("추가 송출"));
        draft.aux_to = "both".into();
        assert!(draft.build(&catalog()).is_err());
        draft.aux_to = "output".into();
        draft.sound.preset = "warm".into();
        draft.sound.input_gain_db = "3.5".into();
        draft.sound.strength = "0.375".into();
        draft
            .sound
            .append_plugins(vec!["not-installed.vst3".into()])
            .unwrap();
        let scene = draft.build(&catalog()).unwrap();
        assert_eq!(scene.route.input_gain_db, 3.5);
        assert_eq!(scene.route.vocal_strength, 0.375);
        assert_eq!(scene.effects.len(), 1);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.mixlive-scene.json");
        scene.save_new(&path).unwrap();
        let restored = AudioScene::read(&path).unwrap();
        assert_eq!(restored.route, scene.route);
        assert_eq!(restored.effects, scene.effects);
    }
}
