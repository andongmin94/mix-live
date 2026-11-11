//! Convert the new-Scene form into the existing schema. This layer has no device I/O.

use mixlive_audio_windows::{
    config::{AuxDestination, InputMap, OutputMap, RouteOptions},
    devices::{DeviceCatalog, DeviceConfig, DeviceInfo},
    scene::AudioScene,
};
use mixlive_effects::{Preset, VocalChain, VocalConfig};

pub struct SceneDraft {
    pub input: Option<String>,
    pub input_channel: String,
    pub output: Option<String>,
    pub output_channels: String,
    pub send: Option<String>,
    pub send_channels: String,
    pub aux_input: Option<String>,
    pub aux_channels: String,
    pub aux_to: String,
    pub vocal: String,
}

impl SceneDraft {
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
        let send_channels = if self.send.is_some() {
            let send = selected(&self.send, &catalog.outputs, "추가 송출")?;
            let channels = channels(&self.send_channels)?;
            OutputMap::new(&channels, configuration(send)?.channels)?;
            channels
        } else {
            vec![1, 2]
        };
        let aux_channels = if self.aux_input.is_some() {
            let aux = selected(&self.aux_input, &catalog.inputs, "AUX / MR 입력")?;
            let channels = channels(&self.aux_channels)?;
            InputMap::new(&channels, configuration(aux)?.channels)?;
            channels
        } else {
            vec![1, 2]
        };
        let aux_to = match self.aux_to.as_str() {
            "output" => AuxDestination::Output,
            "send" => AuxDestination::Send,
            "both" => AuxDestination::Both,
            _ => return Err("AUX를 보낼 목적지를 선택하세요.".into()),
        };
        if self.aux_input.is_some() && aux_to.includes(1) && self.send.is_none() {
            return Err("AUX를 추가 송출로 보내려면 추가 송출 장치를 선택하세요. 주 출력만 사용하려면 AUX 목적지를 주 출력으로 바꾸세요.".into());
        }
        let vocal = self
            .vocal
            .parse::<Preset>()
            .map_err(|error| error.to_string())?;
        let defaults = VocalConfig::default();
        let route = RouteOptions {
            input: input.id.clone(),
            input_channel,
            aux_input: self.aux_input.clone(),
            aux_channels,
            aux_gain_db: -12.0,
            aux_to,
            output: output.id.clone(),
            output_channels,
            send: self.send.clone(),
            send_channels,
            input_gain_db: defaults.input_gain_db,
            output_gain_db: -18.0,
            send_gain_db: 0.0,
            vocal,
            vocal_strength: defaults.strength,
            vocal_threshold_db: defaults.compressor_threshold_db,
            deess_threshold_db: defaults.deess_threshold_db,
            queue_ms: 30,
        };
        // Use the same coefficient/sample-rate validation as Prepared without opening devices.
        // Start rechecks the live endpoint configuration; the catalog is only a point-in-time view.
        VocalChain::new(input_config.sample_rate, route.vocal_config())
            .map_err(|error| error.to_string())?;
        AudioScene::new(route, Vec::new()).map_err(|error| error.to_string())
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
                "{label} 장치가 조회한 목록에 없습니다. 새 Scene을 다시 열어 장치를 확인하세요."
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use mixlive_audio_windows::config::{Cli, Command};

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
        }
    }

    fn draft() -> SceneDraft {
        SceneDraft {
            input: Some("input-a".into()),
            input_channel: "1".into(),
            output: Some("output-a".into()),
            output_channels: "1,2".into(),
            send: None,
            send_channels: "1,2".into(),
            aux_input: None,
            aux_channels: "1,2".into(),
            aux_to: "send".into(),
            vocal: "off".into(),
        }
    }

    #[test]
    fn new_scene_uses_exact_ids_and_existing_cli_defaults() {
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
        draft.vocal = "natural".into();
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
        draft.vocal = "warm".into();
        let scene = draft.build(&catalog()).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.mixlive-scene.json");
        scene.save_new(&path).unwrap();
        let restored = AudioScene::read(&path).unwrap();
        assert_eq!(restored.route, scene.route);
        assert!(restored.effects.is_empty());
    }
}
