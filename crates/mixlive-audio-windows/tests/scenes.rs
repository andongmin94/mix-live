//! CLI wiring tests: use fake endpoint IDs only for file operations, never open a device.
use mixlive_audio_windows::scene::DeviceScene;
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
    ]).output().unwrap();
    assert!(saved.status.success(), "{}", String::from_utf8_lossy(&saved.stderr));
    let scene = DeviceScene::read(&path).unwrap();
    assert_eq!(scene.route.vocal, Preset::Bright);
    assert_eq!(scene.route.input_gain_db, 3.0);
    assert_eq!(scene.route.send_gain_db, -6.0);
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
    // A complete valid document with a deliberately unsupported version must fail
    // during loading, before either device command can resolve fake IDs or start audio.
    let saved = command().args(["scene", "save"]).arg(&path)
        .args(["--input", "test-mic", "--output", "test-phones"]).output().unwrap();
    assert!(saved.status.success());
    let mut document: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    document["schema"] = 99.into();
    std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
    for action in ["show", "check", "run"] {
        let result = command().args(["scene", action]).arg(&path).output().unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("unsupported MixLive device scene schema"));
    }
}
