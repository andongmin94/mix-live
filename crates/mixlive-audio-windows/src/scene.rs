//! Explicit device/vocal scene persistence on the control thread, never in an audio callback.
//! This saves the existing one-microphone route, not an unimplemented app/VST3 routing graph.

use crate::config::RouteOptions;
use serde::{Deserialize, Serialize};
use std::{fs::File, io::{BufReader, Write}, path::Path};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceScene {
    schema: u32,
    pub route: RouteOptions,
}

impl DeviceScene {
    pub fn new(route: RouteOptions) -> Result<Self> {
        route.validate()?;
        Ok(Self { schema: 1, route })
    }

    /// Reject unsupported files and invalid settings. No default device substitution or migration.
    pub fn read(path: &Path) -> Result<Self> {
        let scene: Self = serde_json::from_reader(BufReader::new(File::open(path)?))?;
        scene.validate()?;
        Ok(scene)
    }

    fn validate(&self) -> Result<()> {
        if self.schema != 1 { return Err("unsupported MixLive device scene schema".into()); }
        self.route.validate()?;
        Ok(())
    }

    /// Finish writing beside the destination, then publish a NEW file without overwriting it.
    /// Saved endpoint IDs remain private local configuration; no audio or plugin state is stored.
    pub fn save_new(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let bytes = serde_json::to_vec_pretty(self)?;
        let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(&bytes)?;
        temporary.write_all(b"\n")?;
        temporary.as_file().sync_all()?;
        temporary.persist_noclobber(path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use crate::config::{Cli, Command};
    use mixlive_effects::Preset;
    use serde_json::json;

    fn route() -> RouteOptions {
        let cli = Cli::try_parse_from([
            "mixlive-audio", "check", "--input", "test-mic", "--input-channel", "2",
            "--output", "test-phones", "--output-channels", "3,4",
            "--send", "test-cable", "--send-channels", "1", "--send-gain-db", "-6",
            "--input-gain-db", "3", "--output-gain-db", "-12", "--vocal", "warm",
            "--vocal-strength", "0.75", "--vocal-threshold-db", "-20",
            "--deess-threshold-db", "-28", "--queue-ms", "20",
        ]).unwrap();
        let Command::Check(route) = cli.command else { panic!("expected check") };
        route
    }

    #[test]
    fn complete_route_roundtrips_including_all_preset_names_and_optional_send() {
        let directory = tempfile::tempdir().unwrap();
        for (index, preset) in [Preset::Off, Preset::Natural, Preset::Warm, Preset::Bright].into_iter().enumerate() {
            let mut options = route();
            options.vocal = preset;
            if index == 0 { options.send = None; }
            if index == 1 { options.send = Some(options.output.clone()); }
            let scene = DeviceScene::new(options.clone()).unwrap();
            let path = directory.path().join(format!("{index}.mixlive-scene.json"));
            scene.save_new(&path).unwrap();
            let restored = DeviceScene::read(&path).unwrap();
            assert_eq!(restored.route, options);
            assert_eq!(serde_json::to_value(&restored).unwrap()["route"]["vocal"], preset.to_string());
        }
    }

    #[test]
    fn scene_save_preserves_existing_files_and_rejects_invalid_settings_before_writing() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("voice.mixlive-scene.json");
        let mut scene = DeviceScene::new(route()).unwrap();
        scene.save_new(&path).unwrap();
        let original = std::fs::read(&path).unwrap();
        scene.route.output_gain_db = -30.0;
        assert!(scene.save_new(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        scene.route.vocal_strength = f32::NAN;
        let absent = directory.path().join("invalid.mixlive-scene.json");
        assert!(scene.save_new(&absent).is_err());
        assert!(!absent.exists());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn files_reject_unsupported_schema_unknown_fields_missing_values_and_bad_routes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("invalid.mixlive-scene.json");
        let original = serde_json::to_value(DeviceScene::new(route()).unwrap()).unwrap();
        let mut cases = Vec::new();
        for (key, value) in [("schema", json!(2)), ("unknown", json!(true))] {
            let mut document = original.clone();
            document[key] = value;
            cases.push(document);
        }
        for (key, value) in [
            ("input", json!(" ")), ("output", json!("")), ("send", json!(" ")),
            ("input_channel", json!(0)),
            ("output_channels", json!([])), ("output_channels", json!([1, 1])),
            ("send_channels", json!([0])), ("queue_ms", json!(0)),
            ("input_gain_db", json!(100)), ("output_gain_db", json!(1)),
            ("vocal_strength", json!(2)), ("vocal", json!("magic")),
            ("vocal_threshold_db", json!(-100)), ("deess_threshold_db", json!(1)),
            ("unknown", json!(true)),
        ] {
            let mut document = original.clone();
            document["route"][key] = value;
            cases.push(document);
        }
        let mut overlap = original.clone();
        overlap["route"]["send"] = json!("test-phones");
        overlap["route"]["send_channels"] = json!([4]);
        cases.push(overlap);
        let mut missing = original;
        missing["route"].as_object_mut().unwrap().remove("output_gain_db");
        cases.push(missing);
        for document in cases {
            std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
            assert!(DeviceScene::read(&path).is_err(), "accepted invalid scene: {document}");
        }
        std::fs::write(&path, b"{\"schema\": 1,").unwrap();
        assert!(DeviceScene::read(&path).is_err());
        assert!(DeviceScene::read(&directory.path().join("missing.json")).is_err());
    }
}
