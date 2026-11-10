//! Ordinary end-to-end tests against existing, source-built VST3 fixtures.
//! CI supplies the paths; no installed or commercial plugin is used.
#![cfg(windows)]

use mixlive_vst3::{InspectArgs, ParameterChange, StateFile, inspect};
use serde_json::Value;
use std::path::PathBuf;

fn options(variable: &str) -> InspectArgs {
    InspectArgs {
        plugin: PathBuf::from(std::env::var_os(variable).expect("build the documented CI fixture and set its path")),
        class_id: None, sample_rate: 48_000, parameters: vec![], state_in: None, state_out: None, timeout_seconds: 30,
    }
}
fn run(options: &InspectArgs) -> Value {
    inspect(std::path::Path::new(env!("CARGO_BIN_EXE_mixlive-plugins")), options).unwrap()
}
fn value(report: &Value, id: u32) -> f64 {
    report["parameters"].as_array().unwrap().iter()
        .find(|parameter| parameter["id"].as_u64() == Some(u64::from(id))).unwrap()["value"].as_f64().unwrap()
}

#[test]
#[ignore = "requires the source-built vst3-rs gain example; CI supplies MIXLIVE_GAIN_VST3"]
fn native_gain_processes_real_buffers_and_bad_modules_fail_in_the_child() {
    let mut args = options("MIXLIVE_GAIN_VST3");
    args.parameters.push(ParameterChange { id: 0, normalized: 0.25 });
    let report = run(&args);
    assert_eq!(report["synthetic_processing"]["frames"], 4096);
    assert_eq!(report["synthetic_processing"]["finite"], true);
    assert_eq!(report["live_audio_connected"], false);
    let ratio = report["synthetic_processing"]["output_input_rms_ratio"].as_f64().unwrap();
    assert!((ratio - 0.25).abs() < 1e-6, "gain did not reach the processor: {ratio}");
    let directory = tempfile::tempdir().unwrap();
    args.plugin = directory.path().join("invalid.vst3");
    std::fs::write(&args.plugin, b"not a dynamic library").unwrap();
    assert!(inspect(std::path::Path::new(env!("CARGO_BIN_EXE_mixlive-plugins")), &args).is_err());
}

#[test]
#[ignore = "requires vst3-host-testplug; CI supplies MIXLIVE_STATE_VST3"]
fn plugin_state_restores_in_a_fresh_process_and_never_crosses_class_ids() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("voice.mixlive-vst3.json");
    let mut args = options("MIXLIVE_STATE_VST3");
    args.parameters.push(ParameterChange { id: 0, normalized: 0.37 });
    args.state_out = Some(path.clone());
    let first = run(&args);
    assert!((value(&first, 0) - 0.37).abs() < 1e-6);
    assert!(first["state_saved_bytes"].as_u64().unwrap() > 0);
    args.state_out = None;
    args.parameters.clear();
    args.state_in = Some(path.clone());
    let restored = run(&args);
    assert_eq!(restored["state_loaded"], true);
    assert_eq!(restored["plugin"]["uid"], first["plugin"]["uid"]);
    assert!((value(&restored, 0) - 0.37).abs() < 1e-6);
    let bytes = std::fs::read(&path).unwrap();
    args.state_out = Some(path.clone());
    assert!(inspect(std::path::Path::new(env!("CARGO_BIN_EXE_mixlive-plugins")), &args).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    args.state_out = None;
    args.class_id = Some("00000000000000000000000000000000".into());
    assert!(inspect(std::path::Path::new(env!("CARGO_BIN_EXE_mixlive-plugins")), &args).is_err());
    assert_eq!(StateFile::read(&path).unwrap().schema, 1);
}
