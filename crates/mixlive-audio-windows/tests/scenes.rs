//! CLI wiring tests: fake endpoint IDs and nonexistent plugin paths never open hardware/code.
use mixlive_audio_windows::scene::AudioScene;
use mixlive_effects::Preset;
use std::process::Command;

fn command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_mixlive-audio"))
}

#[test]
fn save_and_show_work_without_audio_hardware_and_do_not_overwrite() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("노래 설정.mixlive-scene.json");
    let saved = command().args(["scene", "save"]).arg(&path).args([
        "--input", "not-a-real-microphone", "--output", "not-a-real-playback-device",
        "--vocal", "bright", "--input-gain-db", "3", "--output-gain-db", "-12",
        "--send", "not-a-real-cable", "--send-gain-db", "-6",
        "--aux-input", "not-a-real-mr-capture", "--aux-channels", "2,1", "--aux-gain-db", "-9",
    ]).output().unwrap();
    assert!(saved.status.success(), "{}", String::from_utf8_lossy(&saved.stderr));
    let scene = AudioScene::read(&path).unwrap();
    assert_eq!(scene.route.vocal, Preset::Bright);
    assert_eq!(scene.route.input_gain_db, 3.0);
    assert_eq!(scene.route.send_gain_db, -6.0);
    assert_eq!(scene.route.aux_input.as_deref(), Some("not-a-real-mr-capture"));
    assert_eq!(scene.route.aux_channels, [2, 1]);
    assert_eq!(scene.route.aux_gain_db, -9.0);
    assert_eq!(scene.route.aux_to, mixlive_audio_windows::config::AuxDestination::Send);
    assert!(scene.effects.is_empty());
    let original = std::fs::read(&path).unwrap();
    let shown = command().args(["scene", "show"]).arg(&path).output().unwrap();
    assert!(shown.status.success(), "{}", String::from_utf8_lossy(&shown.stderr));
    assert_eq!(serde_json::from_slice::<serde_json::Value>(&shown.stdout).unwrap(),
        serde_json::from_slice::<serde_json::Value>(&original).unwrap());
    let repeated = command().args(["scene", "save"]).arg(&path)
        .args(["--input", "other-mic", "--output", "other-phones"]).output().unwrap();
    assert!(!repeated.status.success());
    assert_eq!(std::fs::read(&path).unwrap(), original);
}

#[test]
fn multiple_apps_persist_distinct_executable_paths_and_disconnected_defaults() {
    use mixlive_audio_windows::config::AuxDestination;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("apps.json");
    let browser = r"C:\Program Files\Browser\browser.exe";
    let call = r"C:\Program Files\Call\call.exe";
    let saved = command().args(["scene", "save"]).arg(&path)
        .args(["--input", "mic", "--output", "phones", "--send", "cable",
            "--aux-input", "mr-device", "--app", browser, "--app", call])
        .output().unwrap();
    assert!(saved.status.success(), "{}", String::from_utf8_lossy(&saved.stderr));
    let restored = AudioScene::read(&path).unwrap();
    assert_eq!(restored.route.apps.len(), 2);
    assert_eq!(restored.route.apps[0].path, browser);
    assert_eq!(restored.route.apps[1].path, call);
    assert!(restored.route.has_aux());
    assert!(restored.route.apps.iter().all(|app| app.to == AuxDestination::Off));
    let mut route = restored.route;
    route.apps[0].to = AuxDestination::Send;
    route.apps[1].to = AuxDestination::Output;
    assert!(route.validate().is_ok());
    route.apps[0].channels = vec![3];
    assert!(route.validate().is_err());
    route.apps[0].channels = vec![1, 2];
    route.apps[1].path = browser.into();
    assert!(route.validate().is_err());
    assert!(!String::from_utf8(std::fs::read(&path).unwrap()).unwrap().contains("\"pid\""));
}

#[test]
fn invalid_scene_never_reaches_device_check_or_start_and_run_requires_valid_duration() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("invalid.mixlive-scene.json");
    let mut invalid = command().args(["scene", "save"]).arg(&path)
        .args(["--input", "test-mic", "--output", "test-phones", "--output-channels", "1,1"])
        .output().unwrap();
    assert!(!invalid.status.success());
    assert!(!path.exists());
    invalid = command().args(["scene", "run"]).arg(&path).args(["--seconds", "0"])
        .output().unwrap();
    assert!(!invalid.status.success());
    let saved = command().args(["scene", "save"]).arg(&path)
        .args(["--input", "test-mic", "--output", "test-phones"]).output().unwrap();
    assert!(saved.status.success());
    let mut document: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    document["schema"] = 2.into();
    std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
    for action in ["show", "check", "run"] {
        let result = command().args(["scene", action]).arg(&path).output().unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("unsupported MixLive scene schema"));
    }
}

#[test]
fn cli_packs_chain_states_and_rejects_overrides_before_device_access() {
    use serde_json::json;
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("original.chain.json");
    let id = "0123456789ABCDEF0123456789ABCDEF";
    std::fs::write(&source, serde_json::to_vec(&json!({"schema": 2, "effects": [
        {"plugin": "absent.vst3", "class_id": id, "state": [1,2], "bypassed": false},
        {"plugin": "absent.vst3", "class_id": id, "state": [3,4], "bypassed": true}
    ]})).unwrap()).unwrap();
    let folder = directory.path().join("다른 저장 폴더");
    std::fs::create_dir(&folder).unwrap();
    let path = folder.join("노래.mixlive-scene.json");
    let saved = command().args(["scene", "save"]).arg(&path)
        .args(["--input", "not-a-device", "--output", "not-an-output", "--vst3-chain"])
        .arg(&source).output().unwrap();
    assert!(saved.status.success(), "{}", String::from_utf8_lossy(&saved.stderr));
    std::fs::remove_file(&source).unwrap();
    let scene = AudioScene::read(&path).unwrap();
    assert_eq!(scene.effects.len(), 2);
    assert_eq!(scene.effects[0].state.as_deref(), Some([1,2].as_slice()));
    assert_eq!(scene.effects[1].state.as_deref(), Some([3,4].as_slice()));
    assert!(!scene.effects[0].bypassed && scene.effects[1].bypassed);
    assert_eq!(scene.effects[0].plugin, directory.path().canonicalize().unwrap().join("absent.vst3"));
    let shown = command().args(["scene", "show"]).arg(&path).output().unwrap();
    assert!(shown.status.success());
    let original = std::fs::read(&path).unwrap();
    for options in [vec!["--vst3", "other.vst3"], vec!["--vst3-chain", "not-read.json"], vec!["--vst3-editor", "3"]] {
        let result = command().args(["scene", "run"]).arg(&path).args(options).output().unwrap();
        assert!(!result.status.success());
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(error.contains("embedded") || error.contains("inside this VST3 chain"), "{error}");
    }
    assert_eq!(std::fs::read(&path).unwrap(), original);
    let rejected = directory.path().join("must-not-exist.json");
    let result = command().args(["scene", "save"]).arg(&rejected)
        .args(["--input", "mic", "--output", "phones", "--vst3", "absent.vst3", "--vst3-editor", "1"])
        .output().unwrap();
    assert!(!result.status.success());
    assert!(!rejected.exists());
    let listed = directory.path().join("listed.json");
    let result = command().current_dir(directory.path()).args(["scene", "save"]).arg(&listed)
        .args(["--input", "mic", "--output", "phones", "--vst3", "eq.vst3", "--vst3", "reverb.vst3"])
        .output().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let scene = AudioScene::read(&listed).unwrap();
    assert_eq!(scene.effects.iter().map(|effect| effect.plugin.file_name().unwrap().to_str().unwrap()).collect::<Vec<_>>(), ["eq.vst3", "reverb.vst3"]);
    assert!(scene.effects.iter().all(|effect| effect.plugin.is_absolute() && effect.state.is_none() && !effect.bypassed));
}

#[test]
fn scene_output_is_explicit_and_rejects_collisions_before_start() {
    let directory = tempfile::tempdir().unwrap();
    let original = directory.path().join("original.mixlive-scene.json");
    let fresh = directory.path().join("snapshot.mixlive-scene.json");
    let chain = directory.path().join("chain.json");
    let saved = command().args(["scene", "save"]).arg(&original)
        .args(["--input", "not-a-real-mic", "--output", "not-a-real-output"]).output().unwrap();
    assert!(saved.status.success());
    let before = std::fs::read(&original).unwrap();
    for saved_route in [false, true] {
        let run = || {
            let mut process = command();
            if saved_route { process.args(["scene", "run"]).arg(&original); }
            else { process.args(["run", "--input", "not-a-real-mic", "--output", "not-a-real-output"]); }
            process
        };
        let result = run().arg("--scene-out").arg(&original).output().unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("Scene output already exists"));
        let result = run().arg("--scene-out").arg(&fresh).arg("--vst3-chain-out").arg(&chain).output().unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("choose --scene-out OR --vst3-chain-out"));
        let result = run().arg("--scene-out").arg(&fresh).output().unwrap();
        assert!(!result.status.success(), "a nonexistent device must not start");
        assert!(!fresh.exists());
        assert!(!chain.exists());
        assert_eq!(std::fs::read(&original).unwrap(), before);
    }
    let result = command().args(["scene", "show"]).arg(&original).arg("--scene-out").arg(&fresh).output().unwrap();
    assert!(!result.status.success());
    assert!(!fresh.exists());
}
