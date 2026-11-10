//! Reuses the existing native fixtures. These tests do not claim interactive UI coverage.
#![cfg(windows)]
use mixlive_vst3::{InspectArgs, StateFile, editor::{EditArgs, edit, flush_editor_changes}, inspect};
use std::path::{Path, PathBuf};
use vst3_host::Vst3Host;

#[test]
#[ignore = "requires the existing MIXLIVE_STATE_VST3 fixture"]
fn parameter_only_flush_can_be_saved_and_restored_without_audio_devices() {
    let path = PathBuf::from(std::env::var_os("MIXLIVE_STATE_VST3").expect("build state fixture"));
    let directory = tempfile::tempdir().unwrap();
    let saved = directory.path().join("edited.json");
    let mut plugin = Vst3Host::builder().sample_rate(48_000.0).block_size(256).build().unwrap()
        .load_plugin(&path).unwrap();
    plugin.start_processing().unwrap();
    plugin.set_parameter(0, 0.29).unwrap();
    flush_editor_changes(&mut plugin).unwrap();
    plugin.stop_processing().unwrap();
    StateFile { schema: 1, class_id: plugin.info().uid.clone(), state: plugin.save_state().unwrap() }
        .save_new(&saved).unwrap();
    drop(plugin);
    let options = InspectArgs { plugin: path, class_id: None, sample_rate: 48_000,
        parameters: vec![], state_in: Some(saved), state_out: None, timeout_seconds: 30 };
    let report = inspect(Path::new(env!("CARGO_BIN_EXE_mixlive-plugins")), &options).unwrap();
    let value = report["parameters"].as_array().unwrap().iter()
        .find(|parameter| parameter["id"] == 0).unwrap()["value"].as_f64().unwrap();
    assert!((value - 0.29).abs() < 1e-6);
    assert_eq!(report["state_loaded"], true);
    assert_eq!(report["live_audio_connected"], false);
}

#[test]
#[ignore = "requires the existing MIXLIVE_GAIN_VST3 fixture, which has no editor"]
fn missing_native_editor_does_not_publish_a_state_or_change_existing_files() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("never-published.json");
    let mut options = EditArgs {
        plugin: PathBuf::from(std::env::var_os("MIXLIVE_GAIN_VST3").expect("build gain fixture")),
        class_id: None, sample_rate: 48_000, parameters: vec![], state_in: None, state_out: Some(output.clone()),
    };
    let executable = Path::new(env!("CARGO_BIN_EXE_mixlive-plugins"));
    assert!(edit(executable, &options).is_err());
    assert!(!output.exists());
    options.plugin = directory.path().join("broken.vst3");
    std::fs::write(&options.plugin, b"not a native plugin").unwrap();
    assert!(edit(executable, &options).is_err());
    assert!(!output.exists());
}
