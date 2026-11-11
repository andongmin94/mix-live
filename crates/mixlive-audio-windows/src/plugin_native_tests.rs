use super::*;
use crate::{bridge::channel, config::AuxDestination, control::LiveControls};
use mixlive_effects::{VocalChain, VocalConfig};
use std::time::Instant;

fn fixture(name: &str) -> EffectSpec {
    EffectSpec { plugin: std::env::var_os(name).unwrap().into(), class_id: None, state: None }
}

fn wait_for(worker: &mut PluginWorker, health: &Health, frames: u64) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while health.captured_frames.load(Ordering::Relaxed) < frames && !health.failed() && Instant::now() < deadline {
        worker.poll().unwrap(); thread::sleep(Duration::from_millis(1));
    }
    assert!(!health.failed(), "{:?}", health.error());
    assert_eq!(health.captured_frames.load(Ordering::Relaxed), frames);
}

#[test]
fn capture_queue_rejects_overflow_and_invalid_input_without_waiting() {
    let health = Arc::new(Health::default());
    let (writer, _reader) = RingBuffer::new(2);
    let mut input = QueuedInput { writer, has_aux: true, health: health.clone() };
    input.push(&[0.1; 3], Some(&[[0.2, -0.1]; 3]));
    assert_eq!(health.error(), Some("audio clock queue overflow; stopped instead of building unbounded latency"));
    let health = Arc::new(Health::default());
    let (writer, _reader) = RingBuffer::new(2);
    let mut input = QueuedInput { writer, has_aux: false, health: health.clone() };
    input.push(&[f32::NAN], None);
    assert!(health.failed());
}

#[test]
#[ignore = "requires existing MIXLIVE_GAIN_VST3 and MIXLIVE_STATE_VST3 fixtures; no devices/windows"]
fn native_chain_processes_voice_once_and_keeps_aux_and_instances_independent() {
    let spec = fixture("MIXLIVE_GAIN_VST3");
    let mut plugins = Vec::new();
    for value in [0.5, 0.25] {
        let mut plugin = load_effect(&spec, 48000).unwrap();
        plugin.set_parameter(0, value).unwrap();
        plugins.push(plugin);
    }
    let health = Arc::new(Health::default());
    let controls = Arc::new(LiveControls::new(0.0, Some(0.0), true).unwrap());
    let (writer, mut reader) = channel(48000, 48000, 10, 2, health.clone()).unwrap();
    let capture = Capture::new(VocalChain::new(48000, VocalConfig::default()).unwrap(), 48000,
        Some((0.0, AuxDestination::Send)), vec![(writer, vec![0, 1])], health.clone(), controls).unwrap();
    let (mut input, mut worker) = spawn(capture, plugins, 48000, 100, true, health.clone()).unwrap();
    assert!(worker.description.contains("Serial chain: 2 effects"));
    assert!(open_editor(worker.slots[0].plugin.clone()).err().unwrap().contains("no native editor"));
    for count in [1, 137, 256, 219, 923] { input.push(&vec![0.4; count], Some(&vec![[0.1, -0.2]; count])); }
    wait_for(&mut worker, &health, 1536);
    for _ in 0..512 { reader.next_frame([1.0; 2]); }
    for (actual, expected) in reader.next_frame([1.0; 2]).into_iter().zip([0.05, 0.05, 0.15, -0.15]) {
        assert!((actual - expected).abs() < 0.001, "{actual} != {expected}");
    }
    for _ in 0..4096 { reader.next_frame([1.0; 2]); }
    // Editing the SECOND instance cannot alter the first or create another copy per output.
    worker.slots[1].plugin.lock().unwrap().set_parameter(0, 0.75).unwrap();
    assert!((worker.slots[0].plugin.lock().unwrap().get_parameter(0).unwrap() - 0.5).abs() < 1e-6);
    input.push(&[0.4; 1536], Some(&[[0.1, -0.2]; 1536]));
    wait_for(&mut worker, &health, 3072);
    for _ in 0..512 { reader.next_frame([1.0; 2]); }
    for (actual, expected) in reader.next_frame([1.0; 2]).into_iter().zip([0.15, 0.15, 0.25, -0.05]) {
        assert!((actual - expected).abs() < 0.001, "{actual} != {expected}");
    }
    worker.finish(None).unwrap();
    assert!(!health.failed());

    let health = Arc::new(Health::default());
    let controls = Arc::new(LiveControls::new(0.0, None, false).unwrap());
    let (writer, _reader) = channel(48000, 48000, 30, 1, health.clone()).unwrap();
    let capture = Capture::new(VocalChain::new(48000, VocalConfig::default()).unwrap(), 48000,
        None, vec![(writer, vec![0])], health.clone(), controls).unwrap();
    let plugins = vec![load_effect(&spec, 48000).unwrap(), load_effect(&fixture("MIXLIVE_STATE_VST3"), 48000).unwrap()];
    let error = spawn(capture, plugins, 48000, 30, false, health).err().unwrap();
    assert!(error.contains("slot 2") && error.contains("stereo"));
}

#[test]
#[ignore = "requires existing MIXLIVE_GAIN_VST3 fixture; no devices/windows"]
fn native_serial_stages_preserve_distinct_stereo_and_partial_blocks() {
    let mut processors = Vec::new();
    for gain in [0.5, 0.25] {
        let mut plugin = load_effect(&fixture("MIXLIVE_GAIN_VST3"), 44100).unwrap();
        let buffers = BusAudioBuffers::new(&plugin.audio_bus_layout().unwrap(), CAPTURE_BLOCK, 44100.0);
        plugin.set_parameter(0, gain).unwrap();
        plugin.start_processing().unwrap();
        processors.push(Processor { plugin: Arc::new(Mutex::new(plugin)), buffers });
    }
    for frames in [1, 137, 256, 219] {
        let mut left = vec![0.8; frames];
        let mut right = vec![-0.4; frames];
        assert!(process_chain(&mut processors, &mut left, &mut right));
        assert!(left.iter().all(|value| (value - 0.1).abs() < 1e-6));
        assert!(right.iter().all(|value| (value + 0.05).abs() < 1e-6));
    }
    for processor in processors { processor.plugin.lock().unwrap().stop_processing().unwrap(); }
}

#[test]
#[ignore = "requires existing MIXLIVE_STATE_VST3 fixture; no devices/windows"]
fn native_finish_saves_all_slot_states_or_nothing_and_preserves_existing_files() {
    let directory = tempfile::tempdir().unwrap();
    let spec = fixture("MIXLIVE_STATE_VST3");
    for case in ["save", "fault", "discard", "exists", "partial-failure"] {
        let health = Arc::new(Health::default());
        let mut slots = Vec::new();
        for gain in [0.23, 0.61] {
            let mut plugin = load_effect(&spec, 48000).unwrap();
            let _ = plugin.take_restart_flags();
            plugin.start_processing().unwrap();
            // A last edit with no subsequent audio block must survive in its own snapshot.
            plugin.set_parameter(0, gain).unwrap();
            slots.push(Slot { plugin: Arc::new(Mutex::new(plugin)), editor: None });
        }
        if case == "partial-failure" { slots[1].plugin.lock().unwrap().stop_processing().unwrap(); }
        let worker = PluginWorker { thread: None, stop: Arc::new(AtomicBool::new(false)), slots,
            stopped: false, health: health.clone(), description: String::new() };
        let path = directory.path().join(format!("{case}.json"));
        if case == "exists" { std::fs::write(&path, b"keep").unwrap(); }
        if case == "fault" { health.fail(Fault::Device); }
        match case {
            "discard" => { drop(worker); assert!(!path.exists()); },
            "fault" | "partial-failure" => { assert!(worker.finish(Some(&path)).is_err()); assert!(!path.exists()); },
            "exists" => { assert!(worker.finish(Some(&path)).is_err()); assert_eq!(std::fs::read(&path).unwrap(), b"keep"); },
            _ => {
                use clap::Parser;
                use crate::{config::{Cli, Command}, plugin::PluginArgs, scene::AudioScene};
                worker.finish(Some(&path)).unwrap();
                let restored = ChainFile::read(&path).unwrap();
                assert_eq!(restored.effects.len(), 2);
                // Reuse the real native snapshots through the same scene/launch path as the CLI.
                let Command::Check(route) = Cli::parse_from(["audio", "check", "--input", "fake-mic", "--output", "fake-phones"]).command
                    else { panic!("expected check") };
                let scene_path = directory.path().join("voice.mixlive-scene.json");
                AudioScene::new(route, restored.effects).unwrap().save_new(&scene_path).unwrap();
                let scene = AudioScene::read(&scene_path).unwrap();
                let setup = PluginArgs::default().resolve_scene(scene.effects).unwrap();
                for (spec, expected) in setup.effects.iter().zip([0.23, 0.61]) {
                    assert!(spec.plugin.is_absolute());
                    let plugin = load_effect(spec, 48000).unwrap();
                    assert!((plugin.get_parameter(0).unwrap() - expected).abs() < 1e-6);
                }
                let wrong_class = EffectSpec { plugin: fixture("MIXLIVE_GAIN_VST3").plugin, ..setup.effects[0].clone() };
                assert!(load_effect(&wrong_class, 48000).is_err());
            }
        }
    }
    // Complete chain and embedded scene snapshots plus the pre-existing user file remain.
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 3);
}
