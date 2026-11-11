use super::*;
use crate::{bridge::{channel, QUANTUM}, control::PlaybackGains};
use mixlive_effects::{Preset, VocalConfig};

#[test]
fn one_capture_feeds_two_independent_mixes_without_dry_bypass() {
    let health = Arc::new(Health::default());
    let controls = Arc::new(LiveControls::new(0.0, Some(-6.0), false).unwrap());
    let (first, mut monitor) = channel(48_000, 48_000, 10, 1, health.clone()).unwrap();
    let (second, mut send) = channel(48_000, 48_000, 10, 1, health.clone()).unwrap();
    let mut a_gain = PlaybackGains::new(48000, &[0], controls.clone()).unwrap();
    let mut b_gain = PlaybackGains::new(48000, &[1], controls.clone()).unwrap();
    let vocal = VocalChain::new(48000, VocalConfig { input_gain_db: -6.0, ..VocalConfig::default() }).unwrap();
    let mut capture = Capture::new(vocal, 48000, None, vec![(first, vec![0]), (second, vec![1])], health.clone(), controls).unwrap();
    let prime = (monitor.queue_ms * 48.0).round() as usize + 1024;
    capture.push(&vec![0.4; prime], None);
    for _ in 0..4 * QUANTUM { monitor.next_frame(a_gain.next_gains()); send.next_frame(b_gain.next_gains()); }
    let a = monitor.next_frame(a_gain.next_gains())[0];
    let b = send.next_frame(b_gain.next_gains())[0];
    assert!((a - 0.4 * 10.0_f32.powf(-6.0 / 20.0)).abs() < 0.001);
    assert!((b / a - 10.0_f32.powf(-6.0 / 20.0)).abs() < 0.001);
    capture.push(&[f32::NAN], None);
    assert!(health.failed());
    assert_eq!(monitor.next_frame(a_gain.next_gains()), [0.0; 4]);
    assert_eq!(send.next_frame(b_gain.next_gains()), [0.0; 4]);
}

#[test]
fn vocal_is_processed_once_before_sends_and_playback_attenuation() {
    let config = VocalConfig { preset: Preset::Bright, ..VocalConfig::default() };
    let input: Vec<f32> = (0..1024).map(|i| (i as f32 * 0.17).sin() * 0.4).collect();
    let mut expected = input.clone();
    VocalChain::new(48000, config).unwrap().process(&mut expected).unwrap();
    let health = Arc::new(Health::default());
    let controls = Arc::new(LiveControls::new(-18.0, Some(-6.0), false).unwrap());
    let (first, _a) = channel(48000, 48000, 30, 1, health.clone()).unwrap();
    let (second, _b) = channel(48000, 48000, 30, 1, health.clone()).unwrap();
    let mut capture = Capture::new(VocalChain::new(48000, config).unwrap(), 48000, None,
        vec![(first, vec![0]), (second, vec![1])], health.clone(), controls).unwrap();
    capture.push(&input, None);
    assert!(!health.failed());
    let a = capture.engine.output(0).unwrap();
    let b = capture.engine.output(1).unwrap();
    for (index, sample) in expected[768..].iter().enumerate() {
        assert!((a.left[index] - sample).abs() < 1e-6);
        assert_eq!(b.left[index], a.left[index]);
        assert_eq!(a.left[index], a.right[index]);
    }
    let meters = capture.vocal_stats.take();
    assert!(meters.compression_db > 0.0);
    assert!(meters.output_peak > 0.0);
    assert_eq!(capture.vocal_stats.take().output_peak, 0.0);
}

#[test]
fn shared_endpoint_applies_independent_gains_after_one_resampler() {
    use crate::config::{OutputMap, output_sample};
    let health = Arc::new(Health::default());
    let controls = Arc::new(LiveControls::new(-6.0, Some(-12.0), false).unwrap());
    let (writer, mut reader) = channel(48_000, 48_000, 10, 2, health.clone()).unwrap();
    let mut gains = PlaybackGains::new(48000, &[0, 1], controls.clone()).unwrap();
    let vocal = VocalChain::new(48000, VocalConfig::default()).unwrap();
    let mut capture = Capture::new(vocal, 48000, None, vec![(writer, vec![0, 1])], health.clone(), controls).unwrap();
    let prime = (reader.queue_ms * 48.0).round() as usize + 1024;
    for chunk in vec![0.4; prime].chunks(137) { capture.push(chunk, None); }
    for _ in 0..4 * QUANTUM { reader.next_frame(gains.next_gains()); }
    let maps = [OutputMap::Stereo(0, 1), OutputMap::Stereo(2, 3)];
    let packed = reader.next_frame(gains.next_gains());
    let values: Vec<f32> = (0..6).map(|c| output_sample(&maps, c, &packed)).collect();
    assert!((values[0] - 0.4 * 10.0_f32.powf(-6.0 / 20.0)).abs() < 0.001);
    assert_eq!(values[0], values[1]);
    assert!((values[2] / values[0] - 10.0_f32.powf(-12.0 / 20.0)).abs() < 1e-6);
    assert_eq!(values[2], values[3]);
    assert_eq!(&values[4..], &[0.0; 2]);
    assert_eq!(health.captured_frames.load(Ordering::Relaxed), prime as u64);
    capture.push(&[f32::NAN], None);
    assert!(health.failed());
    assert_eq!(reader.next_frame(gains.next_gains()), [0.0; 4]);
}

#[test]
fn aux_is_stereo_selective_and_never_passes_through_vocal_processing() {
    for destination in [AuxDestination::Output, AuxDestination::Send, AuxDestination::Both] {
        let health = Arc::new(Health::default());
        let controls = Arc::new(LiveControls::new(-18.0, Some(-6.0), true).unwrap());
        let (writer, _reader) = channel(48_000, 48_000, 30, 2, health.clone()).unwrap();
        let config = VocalConfig { preset: Preset::Bright, input_gain_db: 6.0, ..VocalConfig::default() };
        let input: Vec<f32> = (0..613).map(|i| (i as f32 * 0.17).sin() * 0.4).collect();
        let aux = vec![[2.0, -1.0]; input.len()];
        let mut expected = input.clone();
        VocalChain::new(48000, config).unwrap().process(&mut expected).unwrap();
        let mut capture = Capture::new(VocalChain::new(48000, config).unwrap(), 48000,
            Some((-12.0, destination)), vec![(writer, vec![0, 1])], health.clone(), controls.clone()).unwrap();
        let mut start = 0;
        for count in [1, 137, 256, 219] {
            capture.push(&input[start..start + count], Some(&aux[start..start + count]));
            start += count;
        }
        assert!(!health.failed());
        for mix in 0..2 {
            let output = capture.engine.output(mix).unwrap();
            for (index, expected) in expected[394..].iter().enumerate() {
                let aux_gain = if destination.includes(mix) { 10.0_f32.powf(-12.0 / 20.0) } else { 0.0 };
                assert!((output.left[index] - (expected + 2.0 * aux_gain)).abs() < 1e-6);
                assert!((output.right[index] - (expected - aux_gain)).abs() < 1e-6);
            }
        }
        assert_eq!(f32::from_bits(health.aux_peak.load(Ordering::Relaxed)), 2.0);
        controls.apply("mute mic".parse().unwrap()).unwrap();
        capture.push(&[0.1; 256], Some(&[[0.2, -0.1]; 256]));
        for mix in 0..2 {
            let output = capture.engine.output(mix).unwrap();
            let aux_gain = if destination.includes(mix) { 10.0_f32.powf(-12.0 / 20.0) } else { 0.0 };
            assert!((output.left[255] - 0.2 * aux_gain).abs() < 1e-6);
            assert!((output.right[255] + 0.1 * aux_gain).abs() < 1e-6);
        }
        controls.apply("mute aux".parse().unwrap()).unwrap();
        capture.push(&[0.1; 256], Some(&[[0.2, -0.1]; 256]));
        for mix in 0..2 {
            let output = capture.engine.output(mix).unwrap();
            assert_eq!((output.left[255], output.right[255]), (0.0, 0.0));
        }
        capture.push(&[0.1], Some(&[[f32::NAN, 0.0]]));
        assert!(health.failed());
    }
}

#[test]
fn live_routes_keep_call_vocals_while_removing_only_monitor_vocals() {
    let health = Arc::new(Health::default());
    let controls = Arc::new(LiveControls::new(0.0, Some(0.0), true).unwrap());
    let (writer, _reader) = channel(48_000, 48_000, 30, 2, health.clone()).unwrap();
    let config = VocalConfig { preset: Preset::Natural, ..VocalConfig::default() };
    let mut reference = VocalChain::new(48000, config).unwrap();
    let mut capture = Capture::new(VocalChain::new(48000, config).unwrap(), 48000,
        Some((0.0, AuxDestination::Send)), vec![(writer, vec![0, 1])], health.clone(), controls.clone()).unwrap();
    assert_eq!(controls.route_targets(), [1.0, 1.0, 0.0, 1.0]);
    controls.apply("route mic output off".parse().unwrap()).unwrap();
    controls.apply("route aux output on".parse().unwrap()).unwrap();
    controls.apply("level aux output -6".parse().unwrap()).unwrap();
    let mic: Vec<f32> = (0..613).map(|i| (i as f32 * 0.13).sin() * 0.3).collect();
    let mut wet = mic.clone();
    reference.process(&mut wet).unwrap();
    let aux = vec![[0.2, -0.1]; 613];
    let mut cursor = 0;
    for count in [1, 137, 256, 219] {
        capture.push(&mic[cursor..cursor + count], Some(&aux[cursor..cursor + count]));
        cursor += count;
    }
    let monitor = capture.engine.output(0).unwrap();
    let call = capture.engine.output(1).unwrap();
    let gain = 10.0_f32.powf(-6.0 / 20.0);
    for (i, wet) in wet[394..].iter().enumerate() {
        assert!((monitor.left[i] - 0.2 * gain).abs() < 1e-6);
        assert!((monitor.right[i] + 0.1 * gain).abs() < 1e-6);
        assert!((call.left[i] - (wet + 0.2)).abs() < 1e-6);
        assert!((call.right[i] - (wet - 0.1)).abs() < 1e-6);
    }
    controls.apply("route aux send off".parse().unwrap()).unwrap();
    controls.apply("level mic send -9".parse().unwrap()).unwrap();
    let mut next = [0.1; 256];
    reference.process(&mut next).unwrap();
    capture.push(&[0.1; 256], Some(&[[0.2, -0.1]; 256]));
    let call = capture.engine.output(1).unwrap();
    let expected = next[255] * 10.0_f32.powf(-9.0 / 20.0);
    assert!((call.left[255] - expected).abs() < 1e-6);
    assert_eq!(call.left[255], call.right[255]);
    assert!((capture.engine.output(0).unwrap().left[255] - 0.2 * gain).abs() < 1e-6);
    assert!(!health.failed());
}

#[test]
fn single_output_route_fades_are_block_invariant_and_keep_inactive_routes_off() {
    for has_aux in [false, true] {
        let run = |chunks: &[usize]| {
            let health = Arc::new(Health::default());
            let controls = Arc::new(LiveControls::new(0.0, None, has_aux).unwrap());
            let (writer, _reader) = channel(48_000, 48_000, 30, 1, health.clone()).unwrap();
            let mut capture = Capture::new(VocalChain::new(48000, VocalConfig::default()).unwrap(), 48000,
                has_aux.then_some((0.0, AuxDestination::Output)), vec![(writer, vec![0])], health.clone(), controls.clone()).unwrap();
            controls.apply("route mic output off".parse().unwrap()).unwrap();
            controls.apply("level mic output -6".parse().unwrap()).unwrap();
            let mut result = Vec::new();
            for &count in chunks {
                let mic = vec![0.4; count];
                let aux = vec![[0.2, -0.1]; count];
                capture.push(&mic, has_aux.then_some(aux.as_slice()));
                let output = capture.engine.output(0).unwrap();
                result.extend(output.left.iter().zip(output.right).map(|(&left, &right)| [left, right]));
            }
            assert!(!health.failed());
            result
        };
        let whole = run(&[240]);
        let split = run(&[1, 37, 99, 103]);
        assert_eq!(whole, split);
        let final_frame = if has_aux { [0.2, -0.1] } else { [0.0; 2] };
        assert_eq!(whole[239], final_frame);
        for pair in whole.windows(2) {
            assert!(pair[0][0] >= pair[1][0]);
            assert!((pair[0][0] - pair[1][0]).abs() <= 0.4 / 240.0 + 1e-6);
        }
    }
}

#[test]
fn external_effect_preserves_stereo_and_never_processes_aux_or_falls_back_to_dry() {
    let health = Arc::new(Health::default());
    let controls = Arc::new(LiveControls::new(0.0, Some(0.0), true).unwrap());
    let (writer, mut reader) = channel(48000, 48000, 30, 2, health.clone()).unwrap();
    let mut capture = Capture::new(VocalChain::new(48000, VocalConfig::default()).unwrap(), 48000,
        Some((0.0, AuxDestination::Send)), vec![(writer, vec![0, 1])], health.clone(), controls).unwrap();
    let mut calls = 0;
    capture.push_with_effect(&[0.4; 613], Some(&[[0.1, -0.2]; 613]), &mut |mic, left, right| {
        calls += 1;
        for (i, &sample) in mic.iter().enumerate() { left[i] = sample * 0.5; right[i] = -sample * 0.25; }
        true
    });
    assert_eq!(calls, 3);
    let voice = capture.engine.output(0).unwrap();
    let mixed = capture.engine.output(1).unwrap();
    assert_eq!((voice.left[0], voice.right[0]), (0.2, -0.1));
    assert!((mixed.left[0] - 0.3).abs() < 1e-6 && (mixed.right[0] + 0.3).abs() < 1e-6);
    capture.push_with_effect(&[0.4], Some(&[[0.1, -0.2]]), &mut |_, _, _| false);
    assert!(health.failed());
    assert_eq!(reader.next_frame([1.0; 2]), [0.0; 4]);
}
