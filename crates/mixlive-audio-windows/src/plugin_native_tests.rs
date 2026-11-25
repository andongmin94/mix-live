use super::*;
use crate::{bridge::channel, control::LiveControls};
use mixlive_effects::{VocalChain, VocalConfig};
use std::time::Instant;

fn fixture(name: &str) -> EffectSpec {
    EffectSpec { plugin: std::env::var_os(name).unwrap().into(), class_id: None, state: None, bypassed: false }
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
    let mut input = QueuedInput { writer, has_aux: true, app_count: 0, health: health.clone() };
    input.push(&[0.1; 3], Some(&[[0.2, -0.1]; 3]), &[]);
    assert_eq!(health.error(), Some("audio clock queue overflow; stopped instead of building unbounded latency"));
    let health = Arc::new(Health::default());
    let (writer, _reader) = RingBuffer::new(2);
    let mut input = QueuedInput { writer, has_aux: false, app_count: 0, health: health.clone() };
    input.push(&[f32::NAN], None, &[]);
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
        plugins.push((plugin, Arc::new(AtomicBool::new(false))));
    }
    let health = Arc::new(Health::default());
    let controls = Arc::new(LiveControls::new(0.0, [Some(0.0), Some(0.0), Some(0.0)], true, 2, VocalConfig::default()).unwrap());
    controls.apply("route app1 send on".parse().unwrap()).unwrap();
    controls.apply("route app2 output on".parse().unwrap()).unwrap();
    controls.apply("route app1 broadcast on".parse().unwrap()).unwrap();
    controls.apply("route app2 broadcast on".parse().unwrap()).unwrap();
    let (writer, mut reader) = channel(48000, 48000, 10, 3, health.clone()).unwrap();
    controls.apply("route aux send on".parse().unwrap()).unwrap();
    let capture = Capture::new(VocalChain::new(48000, VocalConfig::default()).unwrap(), 48000,
        vec![(writer, vec![0, 1, 2])], health.clone(), controls.clone()).unwrap();
    let (mut input, mut worker) = spawn(capture, plugins, 48000, 100, true, health.clone()).unwrap();
    assert!(worker.description.contains("Serial chain: 2 effects"));
    // The existing native gain fixture reports zero latency; it must not acquire an artificial delay.
    assert!(worker.description.contains("Outgoing AUX/app send compensation: 0 samples"));
    assert!(worker.set_editor_open(0, true).unwrap_err().contains("no native editor"));
    assert!(!worker.editor_is_open(0));
    assert!(worker.set_editor_open(2, true).is_err());
    assert!(!health.failed());
    for count in [1, 137, 256, 219, 923] {
        input.push(&vec![0.4; count], Some(&vec![[0.1, -0.2]; count]),
            &[&vec![[0.1, -0.05]; count], &vec![[0.04, 0.02]; count]]);
    }
    wait_for(&mut worker, &health, 1536);
    for _ in 0..512 { reader.next_frame([1.0; 3]); }
    for (actual, expected) in reader.next_frame([1.0; 3]).into_iter().zip([0.09, 0.07, 0.25, -0.20, 0.19, 0.02]) {
        assert!((actual - expected).abs() < 0.001, "{actual} != {expected}");
    }
    for _ in 0..4096 { reader.next_frame([1.0; 3]); }
    // Live trim reaches the same running worker. Editing the SECOND plugin
    // instance cannot alter the first or create another copy per output.
    controls.apply("vocal trim -6".parse().unwrap()).unwrap();
    worker.slots[1].plugin.lock().unwrap().set_parameter(0, 0.75).unwrap();
    assert!((worker.slots[0].plugin.lock().unwrap().get_parameter(0).unwrap() - 0.5).abs() < 1e-6);
    input.push(&[0.4; 1536], Some(&[[0.1, -0.2]; 1536]),
        &[&[[0.1, -0.05]; 1536], &[[0.04, 0.02]; 1536]]);
    wait_for(&mut worker, &health, 3072);
    for _ in 0..1024 { reader.next_frame([1.0; 3]); }
    let voice = 0.15 * 10.0_f32.powf(-6.0 / 20.0);
    for (actual, expected) in reader.next_frame([1.0; 3]).into_iter().zip([
        voice + 0.04, voice + 0.02, voice + 0.20, voice - 0.25, voice + 0.14, voice - 0.03,
    ]) {
        assert!((actual - expected).abs() < 0.001, "{actual} != {expected}");
    }
    worker.finish(None).unwrap();
    assert!(!health.failed());

    let health = Arc::new(Health::default());
    let controls = Arc::new(LiveControls::new(0.0, [Some(0.0), None, None], false, 0, VocalConfig::default()).unwrap());
    let (writer, _reader) = channel(48000, 48000, 30, 1, health.clone()).unwrap();
    let capture = Capture::new(VocalChain::new(48000, VocalConfig::default()).unwrap(), 48000,
        vec![(writer, vec![0])], health.clone(), controls).unwrap();
    let plugins = vec![(load_effect(&spec, 48000).unwrap(), Arc::new(AtomicBool::new(false))),
        (load_effect(&fixture("MIXLIVE_STATE_VST3"), 48000).unwrap(), Arc::new(AtomicBool::new(false)))];
    let error = spawn(capture, plugins, 48000, 30, false, health).err().unwrap();
    assert!(error.contains("slot 2") && error.contains("stereo"));
}

#[test]
#[ignore = "requires existing MIXLIVE_GAIN_VST3 fixture; no devices/windows"]
fn built_in_room_reaches_the_live_native_effect_as_stereo() {
    use mixlive_effects::ReverbConfig;
    let config = VocalConfig { reverb: ReverbConfig { mix: 0.5, decay_secs: 1.2, predelay_ms: 0.0 }, ..VocalConfig::default() };
    let prepare = || {
        let health = Arc::new(Health::default());
        let controls = Arc::new(LiveControls::new(0.0, [Some(0.0), None, None], false, 0, config).unwrap());
        let (writer, reader) = channel(48000, 48000, 30, 1, health.clone()).unwrap();
        let capture = Capture::new(VocalChain::new(48000, config).unwrap(), 48000,
            vec![(writer, vec![0])], health.clone(), controls).unwrap();
        (capture, reader, health)
    };
    let (capture, mut reader, health) = prepare();
    let mut plugin = load_effect(&fixture("MIXLIVE_GAIN_VST3"), 48000).unwrap();
    plugin.set_parameter(0, 0.5).unwrap();
    let (mut input, mut worker) = spawn(capture, vec![(plugin, Arc::new(AtomicBool::new(false)))], 48000, 100, false, health.clone()).unwrap();
    let mut mic = vec![0.0; 4096];
    mic[0] = 0.5;
    for part in mic.chunks(137) { input.push(part, None, &[]); }
    wait_for(&mut worker, &health, mic.len() as u64);
    worker.finish(None).unwrap();

    let (mut reference, mut expected_reader, reference_health) = prepare();
    reference.push_with_effect(&mic, None, &[], &mut |left, right| {
        for sample in left.iter_mut().chain(right.iter_mut()) { *sample *= 0.5; }
        true
    });
    let mut saw_stereo = false;
    for _ in 0..3500 {
        let actual = reader.next_frame([1.0; 3]);
        let expected = expected_reader.next_frame([1.0; 3]);
        assert!((actual[0] - expected[0]).abs() < 1e-5 && (actual[1] - expected[1]).abs() < 1e-5);
        saw_stereo |= (actual[0] - actual[1]).abs() > 0.001;
    }
    assert!(saw_stereo && !health.failed() && !reference_health.failed());
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
        let latency = plugin.latency_samples();
        assert_eq!(latency, 0);
        processors.push(Processor::new(Arc::new(Mutex::new(plugin)), buffers, Arc::new(AtomicBool::new(false)), 44100, latency));
    }
    for frames in [1, 137, 256, 219] {
        let mut left = vec![0.8; frames];
        let mut right = vec![-0.4; frames];
        assert!(process_chain(&mut processors, &mut left, &mut right));
        assert!(left.iter().all(|value| (value - 0.1).abs() < 1e-6));
        assert!(right.iter().all(|value| (value + 0.05).abs() < 1e-6));
    }
    // Bypass only slot 1; slot 2 keeps its wet gain and both instances keep running.
    processors[0].bypassed.store(true, Ordering::Relaxed);
    let mut left = [0.8; CAPTURE_BLOCK];
    let mut right = [-0.4; CAPTURE_BLOCK];
    assert!(process_chain(&mut processors, &mut left, &mut right));
    assert!((left[0] - 0.1).abs() < 0.002);
    assert!((left[255] - 0.2).abs() < 1e-6 && (right[255] + 0.1).abs() < 1e-6);
    processors[0].bypassed.store(false, Ordering::Relaxed);
    left.fill(0.8); right.fill(-0.4);
    assert!(process_chain(&mut processors, &mut left, &mut right));
    assert!((left[255] - 0.1).abs() < 1e-6 && (right[255] + 0.05).abs() < 1e-6);
    for processor in processors { processor.plugin.lock().unwrap().stop_processing().unwrap(); }
}

#[test]
#[ignore = "requires existing MIXLIVE_STATE_VST3 editor fixture; native windows, no devices"]
fn native_editors_reopen_on_the_same_slots_and_detach_independently() {
    let health = Arc::new(Health::default());
    let spec = fixture("MIXLIVE_STATE_VST3");
    // This instrumented synth is NOT a supported stereo vocal effect. Exercise only
    // the native editor/slot lifecycle, without constructing a live capture route.
    let slots = [0.23, 0.61].into_iter().map(|gain| {
        let mut plugin = load_effect(&spec, 48000).unwrap();
        let _ = plugin.take_restart_flags();
        plugin.start_processing().unwrap();
        plugin.set_parameter(0, gain).unwrap();
        Slot { plugin: Arc::new(Mutex::new(plugin)), editor: None, bypassed: Arc::new(AtomicBool::new(gain > 0.5)) }
    }).collect();
    let mut worker = PluginWorker { thread: None, stop: Arc::new(AtomicBool::new(false)), slots,
        stopped: false, health: health.clone(), description: String::new() };
    // Upstream's documented Editor Attached instrumentation (read-only parameter 1000).
    let attached = |worker: &PluginWorker, index: usize| {
        worker.slots[index].plugin.lock().unwrap().get_parameter(1000).unwrap()
    };
    assert_eq!(attached(&worker, 0), 0.0);
    worker.set_editor_open(0, true).unwrap();
    worker.set_editor_open(0, true).unwrap(); // idempotent; no replacement view
    worker.set_editor_open(1, true).unwrap();
    assert!(worker.editor_is_open(0) && worker.editor_is_open(1));
    for _ in 0..5 {
        assert!(mixlive_vst3::editor::pump_editor_messages());
        assert!(!worker.poll().unwrap());
    }
    assert_eq!(attached(&worker, 0), 1.0);
    assert_eq!(attached(&worker, 1), 1.0);
    worker.set_editor_open(0, false).unwrap();
    assert!(!worker.editor_is_open(0) && worker.editor_is_open(1));
    assert_eq!(attached(&worker, 0), 0.0);
    assert_eq!(attached(&worker, 1), 1.0);
    worker.set_editor_open(0, true).unwrap();
    assert!(mixlive_vst3::editor::pump_editor_messages());
    worker.poll().unwrap();
    assert!(worker.editor_is_open(0));
    assert_eq!(attached(&worker, 0), 1.0);
    // A normal stop detaches both windows, then saves every slot.
    let instances: Vec<_> = worker.slots.iter().map(|slot| slot.plugin.clone()).collect();
    let snapshot = worker.snapshot().unwrap();
    assert_eq!(snapshot.effects.len(), 2);
    assert!(!snapshot.effects[0].bypassed && snapshot.effects[1].bypassed);
    for (instance, effect, expected) in instances.iter().zip(&snapshot.effects)
        .zip([0.23, 0.61]).map(|((instance, effect), expected)| (instance, effect, expected)) {
        assert_eq!(instance.lock().unwrap().get_parameter(1000).unwrap(), 0.0);
        let restored = load_effect(effect, 48000).unwrap();
        assert!((restored.get_parameter(0).unwrap() - expected).abs() < 1e-6);
    }
    assert!(!health.failed());
}

#[test]
#[ignore = "requires existing MIXLIVE_STATE_VST3 fixture; no devices/windows"]
fn native_finish_saves_all_slot_states_or_nothing_and_preserves_existing_files() {
    let directory = tempfile::tempdir().unwrap();
    let spec = fixture("MIXLIVE_STATE_VST3");
    for case in ["save", "scene-snapshot", "fault", "snapshot-fault", "discard", "exists", "partial-failure", "snapshot-partial"] {
        let health = Arc::new(Health::default());
        let mut slots = Vec::new();
        for gain in [0.23, 0.61] {
            let mut plugin = load_effect(&spec, 48000).unwrap();
            let _ = plugin.take_restart_flags();
            plugin.start_processing().unwrap();
            // A last edit with no subsequent audio block must survive in its own snapshot.
            plugin.set_parameter(0, gain).unwrap();
            slots.push(Slot { plugin: Arc::new(Mutex::new(plugin)), editor: None, bypassed: Arc::new(AtomicBool::new(gain > 0.5)) });
        }
        if matches!(case, "partial-failure" | "snapshot-partial") { slots[1].plugin.lock().unwrap().stop_processing().unwrap(); }
        let worker = PluginWorker { thread: None, stop: Arc::new(AtomicBool::new(false)), slots,
            stopped: false, health: health.clone(), description: String::new() };
        let path = directory.path().join(format!("{case}.json"));
        if case == "exists" { std::fs::write(&path, b"keep").unwrap(); }
        if matches!(case, "fault" | "snapshot-fault") { health.fail(Fault::Device); }
        match case {
            "discard" => { drop(worker); assert!(!path.exists()); },
            "fault" | "partial-failure" => { assert!(worker.finish(Some(&path)).is_err()); assert!(!path.exists()); },
            "snapshot-fault" | "snapshot-partial" => { assert!(worker.snapshot().is_err()); assert!(!path.exists()); },
            "exists" => { assert!(worker.finish(Some(&path)).is_err()); assert_eq!(std::fs::read(&path).unwrap(), b"keep"); },
            _ => {
                use clap::Parser;
                use crate::{config::{Cli, Command}, plugin::PluginArgs, scene::AudioScene};
                let restored = if case == "scene-snapshot" {
                    worker.snapshot().unwrap()
                } else {
                    worker.finish(Some(&path)).unwrap();
                    ChainFile::read(&path).unwrap()
                };
                assert_eq!(restored.effects.len(), 2);
                assert!(!restored.effects[0].bypassed && restored.effects[1].bypassed);
                // Reuse the real native snapshots through the same scene/launch path as the CLI.
                let Command::Check(route) = Cli::parse_from(["audio", "check", "--input", "fake-mic", "--output", "fake-phones"]).command
                    else { panic!("expected check") };
                let scene_path = directory.path().join(format!("{case}.mixlive-scene.json"));
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
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 4);
}

#[test]
fn dry_comparison_preserves_reported_latency_and_crossfades_across_variable_blocks() {
    const LATENCY: usize = 5;
    let mut dry = DryBlend::new(8000, LATENCY, false);
    // 40-frame ramp. Wet is a synthetic plugin reporting the same latency, with a 0.5 gain.
    let mut pos = 0;
    let mut transitioned = 0_u32;
    for (count, bypassed) in [(13, false), (7, true), (11, true), (22, true), (3, true), (17, false), (23, false)] {
        let left: Vec<_> = (pos..pos + count).map(|i| (i + 1) as f32 * 0.01).collect();
        let right: Vec<_> = left.iter().map(|x| -2.0 * x).collect();
        dry.prepare(&left, &right);
        let delayed: Vec<_> = (pos..pos + count).map(|i| if i < LATENCY { 0.0 } else { (i - LATENCY + 1) as f32 * 0.01 }).collect();
        let mut wet_l: Vec<_> = delayed.iter().map(|x| 0.5 * x).collect();
        let mut wet_r: Vec<_> = delayed.iter().map(|x| -*x).collect();
        dry.mix(&mut wet_l, &mut wet_r, bypassed);
        for i in 0..count {
            if bypassed { transitioned = (transitioned + 1).min(40); }
            else { transitioned = transitioned.saturating_sub(1); }
            let blend = transitioned as f32 / 40.0;
            let expected = delayed[i] * (0.5 + 0.5 * blend);
            assert!((wet_l[i] - expected).abs() < 1e-6);
            assert!((wet_r[i] + 2.0 * expected).abs() < 1e-6);
        }
        pos += count;
    }
    let mut bypassed = DryBlend::new(48000, LATENCY, true);
    let mut l = [1.0; 8];
    let mut r = [-0.5; 8];
    bypassed.prepare(&l, &r);
    l.fill(0.0); r.fill(0.0);
    bypassed.mix(&mut l, &mut r, true);
    assert_eq!(&l[..LATENCY], &[0.0; LATENCY]);
    assert_eq!(&l[LATENCY..], &[1.0; 3]);
    assert_eq!(&r[LATENCY..], &[-0.5; 3]);
}
