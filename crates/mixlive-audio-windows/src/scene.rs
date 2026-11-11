//! Device/vocal settings and the ordered VST3 chain in one explicit startup scene.
//! Reading/saving a scene never loads native plugins or opens an audio device.

use crate::{config::RouteOptions, plugin::{EffectSpec, validate_effects}};
use serde::{Deserialize, Serialize};
use std::{fs::File, io::{BufReader, Write}, path::Path};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioScene {
    schema: u32,
    pub route: RouteOptions,
    /// Required even when empty. State bytes are embedded, not references to another file.
    pub effects: Vec<EffectSpec>,
}

impl AudioScene {
    pub fn new(route: RouteOptions, mut effects: Vec<EffectSpec>) -> Result<Self> {
        route.validate()?;
        validate_effects(&effects)?;
        // CLI paths refer to the current working directory, not a future scene directory.
        // absolute() does not require the plugin to be present or execute its native code.
        for effect in &mut effects { effect.plugin = std::path::absolute(&effect.plugin)?; }
        Ok(Self { schema: 3, route, effects })
    }

    /// Relative paths in a hand-authored scene are anchored to that scene, never the caller's cwd.
    pub fn read(path: &Path) -> Result<Self> {
        let path = path.canonicalize()?;
        let mut scene: Self = serde_json::from_reader(BufReader::new(File::open(&path)?))?;
        scene.validate()?;
        let parent = path.parent().ok_or("scene file has no parent directory")?;
        for effect in &mut scene.effects {
            if effect.plugin.is_relative() { effect.plugin = parent.join(&effect.plugin); }
        }
        Ok(scene)
    }

    fn validate(&self) -> Result<()> {
        if self.schema != 3 { return Err("unsupported MixLive scene schema; create a schema 3 scene".into()); }
        self.route.validate()?;
        validate_effects(&self.effects)?;
        Ok(())
    }

    /// Publish one complete NEW document. Endpoint IDs and plugin state are private local data.
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
    use crate::{config::{Cli, Command}, plugin::{ChainFile, PluginArgs}};
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
            if index >= 2 {
                options.aux_input = Some(if index == 2 { options.input.clone() } else { "test-aux".into() });
                options.aux_channels = vec![4, 3];
                options.aux_gain_db = -9.0;
                options.aux_to = crate::config::AuxDestination::Both;
            }
            let scene = AudioScene::new(options.clone(), vec![]).unwrap();
            let path = directory.path().join(format!("{index}.mixlive-scene.json"));
            scene.save_new(&path).unwrap();
            let restored = AudioScene::read(&path).unwrap();
            assert_eq!(restored.route, options);
            assert!(restored.effects.is_empty());
            assert_eq!(serde_json::to_value(&restored).unwrap()["route"]["vocal"], preset.to_string());
        }
    }

    #[test]
    fn scene_embeds_all_states_and_does_not_depend_on_the_imported_chain_file() {
        let directory = tempfile::tempdir().unwrap();
        let chain_path = directory.path().join("original.chain.json");
        let first = EffectSpec { plugin: "not-installed.vst3".into(),
            class_id: Some("0123456789ABCDEF0123456789abcdef".into()), state: Some(vec![1, 2]) };
        ChainFile { schema: 1, effects: vec![first.clone(), EffectSpec { state: Some(vec![3, 4]), ..first }] }
            .save_new(&chain_path).unwrap();
        let effects = PluginArgs { vst3_chain: Some(chain_path.clone()), ..PluginArgs::default() }.resolve().unwrap().effects;
        let folder = directory.path().join("다른 폴더");
        std::fs::create_dir(&folder).unwrap();
        let scene_path = folder.join("노래 설정.mixlive-scene.json");
        AudioScene::new(route(), effects.clone()).unwrap().save_new(&scene_path).unwrap();
        std::fs::remove_file(&chain_path).unwrap();
        let scene = AudioScene::read(&scene_path).unwrap();
        assert_eq!(scene.effects, effects);
        let setup = PluginArgs { vst3_editor: vec![2], ..PluginArgs::default() }.resolve_scene(scene.effects).unwrap();
        assert_eq!(setup.effects[0].state.as_deref(), Some([1, 2].as_slice()));
        assert_eq!(setup.effects[1].state.as_deref(), Some([3, 4].as_slice()));
        assert_eq!(setup.effects[0].plugin, chain_path.parent().unwrap().canonicalize().unwrap().join("not-installed.vst3"));
        assert!(!setup.effects[0].plugin.exists()); // save/show/resolve did not load or require a DLL
        let mut document = serde_json::to_value(AudioScene::new(route(), effects).unwrap()).unwrap();
        document["effects"][0]["plugin"] = json!("relative.vst3");
        std::fs::write(&scene_path, serde_json::to_vec(&document).unwrap()).unwrap();
        let scene = AudioScene::read(&scene_path).unwrap();
        assert_eq!(scene.effects[0].plugin, scene_path.canonicalize().unwrap().parent().unwrap().join("relative.vst3"));
    }

    #[test]
    fn scene_save_preserves_existing_files_and_rejects_invalid_settings_before_writing() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("voice.mixlive-scene.json");
        let mut scene = AudioScene::new(route(), vec![]).unwrap();
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
        let original = serde_json::to_value(AudioScene::new(route(), vec![]).unwrap()).unwrap();
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
            ("aux_input", json!(" ")), ("aux_channels", json!([0])),
            ("aux_gain_db", json!(1)), ("aux_to", json!("magic")),
            ("unknown", json!(true)),
        ] {
            let mut document = original.clone();
            document["route"][key] = value;
            cases.push(document);
        }
        for effects in [json!([{"plugin": ""}]), json!([{"plugin": "x", "state": [1]}]),
            json!([{"plugin": "x", "class_id": "wrong"}]), json!([{"plugin": "x", "typo": true}]), json!(null)] {
            let mut document = original.clone();
            document["effects"] = effects;
            cases.push(document);
        }
        let mut missing_effects = original.clone();
        missing_effects.as_object_mut().unwrap().remove("effects");
        cases.push(missing_effects);
        let mut overlap = original.clone();
        overlap["route"]["send"] = json!("test-phones");
        overlap["route"]["send_channels"] = json!([4]);
        cases.push(overlap);
        let mut missing = original;
        missing["route"].as_object_mut().unwrap().remove("output_gain_db");
        cases.push(missing);
        for document in cases {
            std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
            assert!(AudioScene::read(&path).is_err(), "accepted invalid scene: {document}");
        }
        std::fs::write(&path, b"{\"schema\": 3,").unwrap();
        assert!(AudioScene::read(&path).is_err());
        assert!(AudioScene::read(&directory.path().join("missing.json")).is_err());
    }
}
