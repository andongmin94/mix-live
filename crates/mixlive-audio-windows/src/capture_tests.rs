use super::*;
use crate::{bridge::{channel, QUANTUM}, config::AuxDestination, control::{LiveCommand, PlaybackGains}};
use mixlive_effects::{Preset, VocalConfig};

#[test]
fn one_capture_feeds_two_independent_mixes_without_dry_bypass() {
    let health = Arc::new(Health::default());
    let config = VocalConfig { input_gain_db: -6.0, ..VocalConfig::default() };
    let controls = Arc::new(LiveControls::new(0.0, [Some(0.0), Some(-6.0), None], false, 0, config).unwrap());
    let (first, mut monitor) = channel(48_000, 48_000, 10, 1, health.clone()).unwrap();
    let (second, mut send) = channel(48_000, 48_000, 10, 1, health.clone()).unwrap();
    let mut a_gain = PlaybackGains::new(48000, &[0], controls.clone()).unwrap();
    let mut b_gain = PlaybackGains::new(48000, &[1], controls.clone()).unwrap();
    let vocal = VocalChain::new(48000, config).unwrap();
    let mut capture = Capture::new(vocal, 48000, vec![(first, vec![0]), (second, vec![1])], health.clone(), controls).unwrap();
    let prime = (monitor.queue_ms * 48.0).round() as usize + 1024;
    capture.push(&vec![0.4; prime], None, &[]);
    for _ in 0..4 * QUANTUM { monitor.next_frame(a_gain.next_gains()); send.next_frame(b_gain.next_gains()); }
    let a = monitor.next_frame(a_gain.next_gains())[0];
    let b = send.next_frame(b_gain.next_gains())[0];
    assert!((a - 0.4 * 10.0_f32.powf(-6.0 / 20.0)).abs() < 0.001);
    assert!((b / a - 10.0_f32.powf(-6.0 / 20.0)).abs() < 0.001);
    capture.push(&[f32::NAN], None, &[]);
    assert!(health.failed());
    assert_eq!(monitor.next_frame(a_gain.next_gains()), [0.0; 6]);
    assert_eq!(send.next_frame(b_gain.next_gains()), [0.0; 6]);
}

#[test]
fn vocal_is_processed_once_before_sends_and_playback_attenuation() {
    let config = VocalConfig { preset: Preset::Bright, ..VocalConfig::default() };
    let input: Vec<f32> = (0..1024).map(|i| (i as f32 * 0.17).sin() * 0.4).collect();
    let mut expected = vec![0.0; input.len()];
    VocalChain::new(48000, config).unwrap().process(&input, &mut expected, &mut vec![0.0; input.len()]).unwrap();
    let health = Arc::new(Health::default());
    let controls = Arc::new(LiveControls::new(-18.0, [Some(0.0), Some(-6.0), None], false, 0, config).unwrap());
    let (first, _a) = channel(48000, 48000, 30, 1, health.clone()).unwrap();
    let (second, _b) = channel(48000, 48000, 30, 1, health.clone()).unwrap();
    let mut capture = Capture::new(VocalChain::new(48000, config).unwrap(), 48000,
        vec![(first, vec![0]), (second, vec![1])], health.clone(), controls).unwrap();
    capture.push(&input, None, &[]);
    assert!(!health.failed());
    let a = capture.engine.output(0).unwrap();
    let b = capture.engine.output(1).unwrap();
    for (index, sample) in expected[768..].iter().enumerate() {
        assert!((a.left[index] - sample).abs() < 1e-6);
        assert_eq!(b.left[index], a.left[index]);
        assert_eq!(a.left[index], a.right[index]);
    }
    let voice = f32::from_bits(health.voice_peak.swap(0, Ordering::Relaxed));
    let expected_peak = expected.iter().fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    assert!((voice - expected_peak).abs() < 1e-6);
    let meters = capture.vocal_stats.take();
    assert!(meters.compression_db > 0.0);
    assert!(meters.output_peak > 0.0);
    assert_eq!(capture.vocal_stats.take().output_peak, 0.0);
}

#[test]
fn shared_endpoint_applies_independent_gains_after_one_resampler() {
    use crate::config::{OutputMap, output_sample};
    let health = Arc::new(Health::default());
    let controls = Arc::new(LiveControls::new(-6.0, [Some(0.0), Some(-12.0), None], false, 0, VocalConfig::default()).unwrap());
    let (writer, mut reader) = channel(48_000, 48_000, 10, 2, health.clone()).unwrap();
    let mut gains = PlaybackGains::new(48000, &[0, 1], controls.clone()).unwrap();
    let vocal = VocalChain::new(48000, VocalConfig::default()).unwrap();
    let mut capture = Capture::new(vocal, 48000, vec![(writer, vec![0, 1])], health.clone(), controls).unwrap();
    let prime = (reader.queue_ms * 48.0).round() as usize + 1024;
    for chunk in vec![0.4; prime].chunks(137) { capture.push(chunk, None, &[]); }
    for _ in 0..4 * QUANTUM { reader.next_frame(gains.next_gains()); }
    let maps = [OutputMap::Stereo(0, 1), OutputMap::Stereo(2, 3)];
    let packed = reader.next_frame(gains.next_gains());
    let values: Vec<f32> = (0..6).map(|c| output_sample(&maps, c, &packed)).collect();
    assert!((values[0] - 0.4 * 10.0_f32.powf(-6.0 / 20.0)).abs() < 0.001);
    assert_eq!(values[0], values[1]);
    assert!((values[2] / values[0] - 10.0_f32.powf(-12.0 / 20.0)).abs() < 1e-6);
    assert_eq!(values[2], values[3]);
    assert_eq!(&values[4..], &[0.0; 2]);
    let peaks = reader.stats.take_peaks();
    assert!(peaks[0] > 0.0 && peaks[1] > 0.0);
    assert!((peaks[1] / peaks[0] - 10.0_f32.powf(-12.0 / 20.0)).abs() < 1e-5);
    assert_eq!(reader.stats.take_peaks(), [0.0; 3]);
    assert_eq!(health.captured_frames.load(Ordering::Relaxed), prime as u64);
    capture.push(&[f32::NAN], None, &[]);
    assert!(health.failed());
    assert_eq!(reader.next_frame(gains.next_gains()), [0.0; 6]);
}

#[test]
fn aux_is_stereo_selective_and_never_passes_through_vocal_processing() {
    for destination in [AuxDestination::Output, AuxDestination::Send, AuxDestination::Both] {
        let health = Arc::new(Health::default());
        let config = VocalConfig { preset: Preset::Bright, input_gain_db: 6.0, ..VocalConfig::default() };
        let controls = Arc::new(LiveControls::new(-18.0, [Some(0.0), Some(-6.0), None], true, 0, config).unwrap());
        let (writer, _reader) = channel(48_000, 48_000, 30, 2, health.clone()).unwrap();
        let input: Vec<f32> = (0..613).map(|i| (i as f32 * 0.17).sin() * 0.4).collect();
        let aux = vec![[2.0, -1.0]; input.len()];
        let mut expected = vec![0.0; input.len()];
        VocalChain::new(48000, config).unwrap().process(&input, &mut expected, &mut vec![0.0; input.len()]).unwrap();
        controls.apply(LiveCommand::SourceTrim { source: RouteSource::Aux, db: -12.0 }).unwrap();
        for mix in controls.mixes() {
            controls.apply(LiveCommand::Route { source: RouteSource::Aux, mix, enabled: destination.includes(mix.index()) }).unwrap();
        }
        let mut capture = Capture::new(VocalChain::new(48000, config).unwrap(), 48000,
            vec![(writer, vec![0, 1])], health.clone(), controls.clone()).unwrap();
        let mut start = 0;
        for count in [1, 137, 256, 219] {
            capture.push(&input[start..start + count], Some(&aux[start..start + count]), &[]);
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
        capture.push(&[0.1; 256], Some(&[[0.2, -0.1]; 256]), &[]);
        for mix in 0..2 {
            let output = capture.engine.output(mix).unwrap();
            let aux_gain = if destination.includes(mix) { 10.0_f32.powf(-12.0 / 20.0) } else { 0.0 };
            assert!((output.left[255] - 0.2 * aux_gain).abs() < 1e-6);
            assert!((output.right[255] + 0.1 * aux_gain).abs() < 1e-6);
        }
        controls.apply("mute aux".parse().unwrap()).unwrap();
        capture.push(&[0.1; 256], Some(&[[0.2, -0.1]; 256]), &[]);
        for mix in 0..2 {
            let output = capture.engine.output(mix).unwrap();
            assert_eq!((output.left[255], output.right[255]), (0.0, 0.0));
        }
        capture.push(&[0.1], Some(&[[f32::NAN, 0.0]]), &[]);
        assert!(health.failed());
    }
}

#[test]
fn live_routes_keep_call_vocals_while_removing_only_monitor_vocals() {
    let health = Arc::new(Health::default());
    let config = VocalConfig { preset: Preset::Natural, ..VocalConfig::default() };
    let controls = Arc::new(LiveControls::new(0.0, [Some(0.0), Some(0.0), None], true, 0, config).unwrap());
    let (writer, _reader) = channel(48_000, 48_000, 30, 2, health.clone()).unwrap();
    let mut reference = VocalChain::new(48000, config).unwrap();
    controls.apply("route aux send on".parse().unwrap()).unwrap();
    let mut capture = Capture::new(VocalChain::new(48000, config).unwrap(), 48000,
        vec![(writer, vec![0, 1])], health.clone(), controls.clone()).unwrap();
    assert_eq!(controls.route_targets()[..6], [1.0, 1.0, 0.0, 0.0, 1.0, 0.0]);
    controls.apply("route mic output off".parse().unwrap()).unwrap();
    controls.apply("route aux output on".parse().unwrap()).unwrap();
    controls.apply("level aux output -6".parse().unwrap()).unwrap();
    let mic: Vec<f32> = (0..613).map(|i| (i as f32 * 0.13).sin() * 0.3).collect();
    let mut wet = vec![0.0; mic.len()];
    reference.process(&mic, &mut wet, &mut vec![0.0; mic.len()]).unwrap();
    let aux = vec![[0.2, -0.1]; 613];
    let mut cursor = 0;
    for count in [1, 137, 256, 219] {
        capture.push(&mic[cursor..cursor + count], Some(&aux[cursor..cursor + count]), &[]);
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
    let mut next = [0.0; 256];
    reference.process(&[0.1; 256], &mut next, &mut [0.0; 256]).unwrap();
    capture.push(&[0.1; 256], Some(&[[0.2, -0.1]; 256]), &[]);
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
            let controls = Arc::new(LiveControls::new(0.0, [Some(0.0), None, None], has_aux, 0, VocalConfig::default()).unwrap());
            let (writer, _reader) = channel(48_000, 48_000, 30, 1, health.clone()).unwrap();
            if has_aux {
                controls.apply("route aux output on".parse().unwrap()).unwrap();
            }
            let mut capture = Capture::new(VocalChain::new(48000, VocalConfig::default()).unwrap(), 48000,
                vec![(writer, vec![0])], health.clone(), controls.clone()).unwrap();
            controls.apply("route mic output off".parse().unwrap()).unwrap();
            controls.apply("level mic output -6".parse().unwrap()).unwrap();
            let mut result = Vec::new();
            for &count in chunks {
                let mic = vec![0.4; count];
                let aux = vec![[0.2, -0.1]; count];
                capture.push(&mic, has_aux.then_some(aux.as_slice()), &[]);
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
    let controls = Arc::new(LiveControls::new(0.0, [Some(0.0), Some(0.0), None], true, 0, VocalConfig::default()).unwrap());
    let (writer, mut reader) = channel(48000, 48000, 30, 2, health.clone()).unwrap();
    controls.apply("route aux send on".parse().unwrap()).unwrap();
    let mut capture = Capture::new(VocalChain::new(48000, VocalConfig::default()).unwrap(), 48000,
        vec![(writer, vec![0, 1])], health.clone(), controls).unwrap();
    let mut calls = 0;
    capture.push_with_effect(&[0.4; 613], Some(&[[0.1, -0.2]; 613]), &[], &mut |left, right| {
        calls += 1;
        for (left, right) in left.iter_mut().zip(right) { *left *= 0.5; *right *= -0.25; }
        true
    });
    assert_eq!(calls, 3);
    let voice = capture.engine.output(0).unwrap();
    let mixed = capture.engine.output(1).unwrap();
    assert_eq!((voice.left[0], voice.right[0]), (0.2, -0.1));
    assert!((mixed.left[0] - 0.3).abs() < 1e-6 && (mixed.right[0] + 0.3).abs() < 1e-6);
    capture.push_with_effect(&[0.4], Some(&[[0.1, -0.2]]), &[], &mut |_, _| false);
    assert!(health.failed());
    assert_eq!(reader.next_frame([1.0; 3]), [0.0; 6]);
}

#[test]
fn separate_app_sources_keep_stereo_and_mix_minus_with_and_without_physical_aux() {
    for has_aux in [false, true] {
        let health = Arc::new(Health::default());
        let controls = Arc::new(LiveControls::new(0.0, [Some(0.0), Some(0.0), None], has_aux, 2, VocalConfig::default()).unwrap());
        controls.apply("route app1 send on".parse().unwrap()).unwrap();
        controls.apply("route app2 output on".parse().unwrap()).unwrap();
        controls.apply("level app2 output -6".parse().unwrap()).unwrap();
        let (writer, _reader) = channel(48000, 48000, 30, 2, health.clone()).unwrap();
        if has_aux {
            for mix in controls.mixes() {
                controls.apply(LiveCommand::Route { source: RouteSource::Aux, mix, enabled: AuxDestination::Both.includes(mix.index()) }).unwrap();
            }
        }
        let mut capture = Capture::new(VocalChain::new(48000, VocalConfig::default()).unwrap(), 48000,
            vec![(writer, vec![0, 1])], health.clone(), controls.clone()).unwrap();
        let mic = [0.1; 256];
        let aux = [[0.05, 0.02]; 256];
        let browser = [[0.3, -0.1]; 256];
        let call = [[-0.2, 0.4]; 256];
        capture.push_with_effect(&mic, has_aux.then_some(aux.as_slice()), &[&browser, &call],
            &mut |left, right| {
                for (left, right) in left.iter_mut().zip(right) {
                    *left *= 2.0; *right *= 2.0;
                }
                true
            });
        let aux = if has_aux { aux[0] } else { [0.0; 2] };
        let monitor = capture.engine.output(0).unwrap();
        let send = capture.engine.output(1).unwrap();
        let attenuation = 10.0_f32.powf(-6.0 / 20.0);
        assert!((monitor.left[255] - (0.2 + aux[0] - 0.2 * attenuation)).abs() < 1e-6);
        assert!((monitor.right[255] - (0.2 + aux[1] + 0.4 * attenuation)).abs() < 1e-6);
        assert!((send.left[255] - (0.5 + aux[0])).abs() < 1e-6);
        assert!((send.right[255] - (0.1 + aux[1])).abs() < 1e-6);
        assert_eq!(f32::from_bits(health.app_peaks[0].load(Ordering::Relaxed)), 0.3);
        assert_eq!(f32::from_bits(health.app_peaks[1].load(Ordering::Relaxed)), 0.4);
        controls.apply("mute app2".parse().unwrap()).unwrap();
        controls.apply("route app1 send off".parse().unwrap()).unwrap();
        let aux_frames = [aux; 256];
        capture.push(&mic, has_aux.then_some(aux_frames.as_slice()), &[&browser, &call]);
        for mix in 0..2 {
            let output = capture.engine.output(mix).unwrap();
            assert!((output.left[255] - (0.1 + aux[0])).abs() < 1e-6);
            assert!((output.right[255] - (0.1 + aux[1])).abs() < 1e-6);
        }
        assert!(!health.failed());
    }
}

#[test]
fn broadcast_and_monitor_route_independently_with_or_without_call() {
    for call in [false, true] {
        let health = Arc::new(Health::default());
        let controls = Arc::new(LiveControls::new(0.0, [Some(0.0), call.then_some(0.0), Some(0.0)], false, 2, VocalConfig::default()).unwrap());
        controls.apply("route app1 broadcast on".parse().unwrap()).unwrap();
        controls.apply("route app2 output on".parse().unwrap()).unwrap();
        if call { controls.apply("route app1 send on".parse().unwrap()).unwrap(); }
        controls.apply("level app1 broadcast -6".parse().unwrap()).unwrap();
        let bindings: Vec<_> = controls.mixes().map(RouteMix::index).collect();
        let (writer, mut reader) = channel(48000, 48000, 10, bindings.len(), health.clone()).unwrap();
        let mut gains = PlaybackGains::new(48000, &bindings, controls.clone()).unwrap();
        let mut capture = Capture::new(VocalChain::new(48000, VocalConfig::default()).unwrap(), 48000,
            vec![(writer, bindings)], health.clone(), controls.clone()).unwrap();
        let mic = vec![0.1; 1536];
        let music = vec![[0.2, -0.1]; 1536];
        let return_audio = vec![[-0.05, 0.3]; 1536];
        capture.push(&mic, None, &[&music, &return_audio]);
        let expected_monitor = [0.05, 0.4];
        let attenuation = 10.0_f32.powf(-6.0 / 20.0);
        let expected_broadcast = [0.1 + 0.2 * attenuation, 0.1 - 0.1 * attenuation];
        let broadcast_lane = if call { 2 } else { 1 };
        for _ in 0..512 { reader.next_frame(gains.next_gains()); }
        let frame = reader.next_frame(gains.next_gains());
        for (actual, expected) in frame[..2].iter().zip(expected_monitor) {
            assert!((*actual - expected).abs() < 0.001);
        }
        if call {
            assert!((frame[2] - 0.3).abs() < 0.001 && frame[3].abs() < 0.001);
        }
        for (actual, expected) in frame[broadcast_lane * 2..broadcast_lane * 2 + 2].iter().zip(expected_broadcast) {
            assert!((*actual - expected).abs() < 0.001);
        }
        controls.apply("mute broadcast".parse().unwrap()).unwrap();
        gains.update();
        let _ = reader.stats.take_peaks();
        for _ in 0..240 { reader.next_frame(gains.next_gains()); }
        let _ = reader.stats.take_peaks();
        let frame = reader.next_frame(gains.next_gains());
        assert_eq!(&frame[broadcast_lane * 2..broadcast_lane * 2 + 2], &[0.0; 2]);
        assert!((frame[1] - 0.4).abs() < 0.001);
        assert_eq!(reader.stats.take_peaks()[broadcast_lane], 0.0);
        assert!(!health.failed());
    }
}

#[test]
fn reported_vst3_delay_aligns_stereo_sends_across_blocks_without_delaying_monitor() {
    const LATENCY: usize = 301;
    const IMPULSE: usize = 7;
    for call in [false, true] {
        let health = Arc::new(Health::default());
        let controls = Arc::new(LiveControls::new(0.0, [Some(0.0), call.then_some(0.0), Some(0.0)], true, 2, VocalConfig::default()).unwrap());
        for command in ["route app1 output on", "route app1 broadcast on", "route app2 output on"] {
            controls.apply(command.parse().unwrap()).unwrap();
        }
        if call { controls.apply("route app1 send on".parse().unwrap()).unwrap(); }
        let bindings: Vec<_> = controls.mixes().map(RouteMix::index).collect();
        let (writer, _reader) = channel(48000, 48000, 30, bindings.len(), health.clone()).unwrap();
        let aux_to = if call { AuxDestination::All } else { AuxDestination::OutputBroadcast };
        for mix in controls.mixes() {
            controls.apply(LiveCommand::Route { source: RouteSource::Aux, mix, enabled: aux_to.includes(mix.index()) }).unwrap();
        }
        let mut capture = Capture::new(VocalChain::new(48000, VocalConfig::default()).unwrap(), 48000,
            vec![(writer, bindings.clone())], health.clone(), controls).unwrap();
        assert!(capture.prepare_voice_delay(96_001, 48000).is_err());
        assert_eq!(capture.prepare_voice_delay(LATENCY as u64, 48000).unwrap(), LATENCY);

        let mut mic = [0.0; 640];
        let mut aux = [[0.0; 2]; 640];
        let mut app1 = [[0.0; 2]; 640];
        let mut app2 = [[0.0; 2]; 640];
        mic[IMPULSE] = 0.5;
        aux[IMPULSE] = [0.125, -0.0625];
        app1[IMPULSE] = [0.25, 0.125];
        app2[IMPULSE] = [-0.0625, 0.25];

        // Synthetic delayed stereo effect: the same reported integer latency as the host's PDC.
        let mut history = [0.0; LATENCY];
        let mut cursor = 0;
        let mut position = 0;
        for count in [13, 113, 256, 129, 129] {
            let end = position + count;
            capture.push_with_effect(&mic[position..end], Some(&aux[position..end]),
                &[&app1[position..end], &app2[position..end]], &mut |left, right| {
                    for (left, right) in left.iter_mut().zip(right) {
                        let delayed = std::mem::replace(&mut history[cursor], *left);
                        cursor = (cursor + 1) % LATENCY;
                        *left = delayed;
                        *right = -0.5 * delayed;
                    }
                    true
                });
            for (core_mix, &logical_mix) in bindings.iter().enumerate() {
                let output = capture.engine.output(core_mix).unwrap();
                for (index, (&left, &right)) in output.left.iter().zip(output.right).enumerate() {
                    let frame = position + index;
                    let expected = match (logical_mix, frame) {
                        (0, IMPULSE) => [0.3125, 0.3125], // monitor keeps the original supplemental signals
                        (0, frame) if frame == IMPULSE + LATENCY => [0.5, -0.25],
                        (_, frame) if logical_mix != 0 && frame == IMPULSE + LATENCY => [0.875, -0.1875],
                        _ => [0.0; 2],
                    };
                    assert_eq!([left, right], expected, "mix {logical_mix}, sample {frame}");
                }
            }
            position = end;
        }
        assert_eq!(position, mic.len());
        assert_eq!(f32::from_bits(health.aux_peak.load(Ordering::Relaxed)), 0.125);
        assert_eq!(f32::from_bits(health.app_peaks[0].load(Ordering::Relaxed)), 0.25);
        assert!(!health.failed());
    }
}

#[test]
fn live_chain_delay_swap_preserves_equal_latency_and_removes_the_last_delay() {
    let health = Arc::new(Health::default());
    let controls = Arc::new(LiveControls::new(0.0, [Some(0.0), None, Some(0.0)], true, 1, VocalConfig::default()).unwrap());
    for command in ["route aux output on", "route aux broadcast on", "route app1 output on", "route app1 broadcast on"] {
        controls.apply(command.parse().unwrap()).unwrap();
    }
    let (writer, _reader) = channel(48000, 48000, 30, 2, health.clone()).unwrap();
    let mut capture = Capture::new(VocalChain::new(48000, VocalConfig::default()).unwrap(), 48000,
        vec![(writer, vec![0, 2])], health.clone(), controls).unwrap();
    let layout = capture.delay_layout();
    let mut prepared = layout.prepare(3, 48000).unwrap();
    assert_eq!(prepared.latency(), 3);
    assert_eq!(prepared.compensation(), 3);
    capture.swap_voice_delay(&mut prepared);
    assert_eq!(prepared.latency(), 0); // returned allocation belongs to the caller
    capture.push(&[0.0; 2], Some(&[[0.125, -0.25]; 2]), &[&[[0.25, 0.125]; 2]]);
    assert_eq!(capture.engine.output(1).unwrap().left, &[0.0; 2]);
    assert_eq!(capture.engine.output(0).unwrap().left, &[0.375; 2]);

    // Reordering a chain with the same latency keeps the first two queued samples.
    capture.swap_voice_delay(&mut layout.prepare(3, 48000).unwrap());
    assert!(layout.prepare(96_001, 48000).is_err());
    assert!(layout.prepare(u64::MAX, 48000).is_err());
    capture.push(&[0.0; 2], Some(&[[0.5, -0.5]; 2]), &[&[[0.25, 0.25]; 2]]);
    assert_eq!(capture.engine.output(1).unwrap().left, &[0.0, 0.375]);
    assert_eq!(capture.engine.output(1).unwrap().right, &[0.0, -0.125]);

    // Removing the last latency-bearing effect immediately restores undelayed sends.
    let mut zero = layout.prepare(0, 48000).unwrap();
    capture.swap_voice_delay(&mut zero);
    assert_eq!(zero.latency(), 3);
    assert_eq!(zero.compensation(), 3);
    capture.push(&[0.0; 2], Some(&[[0.5, -0.5]; 2]), &[&[[0.25, 0.25]; 2]]);
    assert_eq!(capture.engine.output(1).unwrap().left, &[0.75; 2]);
    assert_eq!(capture.engine.output(1).unwrap().right, &[-0.25; 2]);

    let mut longer = layout.prepare(5, 48000).unwrap();
    capture.swap_voice_delay(&mut longer);
    assert_eq!(longer.latency(), 0);
    capture.push(&[0.0; 5], Some(&[[0.125, -0.25]; 5]), &[&[[0.25, 0.125]; 5]]);
    assert_eq!(capture.engine.output(1).unwrap().left, &[0.0; 5]);
    assert_eq!(capture.engine.output(0).unwrap().left, &[0.375; 5]);
    capture.push(&[0.0], Some(&[[0.0; 2]]), &[&[[0.0; 2]]]);
    assert_eq!(capture.engine.output(1).unwrap().left, &[0.375]);
    assert!(!health.failed());
}

#[test]
fn source_mute_applies_after_compensation_and_route_reenable_keeps_current_history() {
    let health = Arc::new(Health::default());
    let controls = Arc::new(LiveControls::new(0.0, [Some(0.0), None, Some(0.0)], true, 1, VocalConfig::default()).unwrap());
    controls.apply("route app1 broadcast on".parse().unwrap()).unwrap();
    let (writer, _reader) = channel(48000, 48000, 30, 2, health.clone()).unwrap();
    controls.apply("route aux broadcast on".parse().unwrap()).unwrap();
    let mut capture = Capture::new(VocalChain::new(48000, VocalConfig::default()).unwrap(), 48000,
        vec![(writer, vec![0, 2])], health.clone(), controls.clone()).unwrap();
    assert_eq!(capture.prepare_voice_delay(512, 48000).unwrap(), 512);
    let silence = [0.0; 256];
    let aux = [[0.125, -0.25]; 256];
    let first_app = [[0.5, 0.25]; 256];
    let next_app = [[0.25, -0.125]; 256];
    capture.push(&silence, Some(&aux), &[&first_app]);
    assert_eq!(capture.engine.output(1).unwrap().sample_peak(), 0.0);
    controls.apply("mute aux".parse().unwrap()).unwrap();
    controls.apply("route app1 broadcast off".parse().unwrap()).unwrap();
    capture.push(&silence, Some(&aux), &[&next_app]);
    // At frame 512 both original sources arrive, but their ramps already reached zero.
    capture.push(&silence, Some(&aux), &[&next_app]);
    assert_eq!(capture.engine.output(1).unwrap().sample_peak(), 0.0);
    controls.apply("route app1 broadcast on".parse().unwrap()).unwrap();
    capture.push(&silence, Some(&aux), &[&next_app]);
    let broadcast = capture.engine.output(1).unwrap();
    assert_eq!([broadcast.left[255], broadcast.right[255]], [0.25, -0.125]);
    assert_eq!(capture.engine.output(0).unwrap().sample_peak(), 0.0);
    assert!(!health.failed());
}

#[test]
fn room_is_processed_once_before_vst3_and_never_affects_aux_or_apps() {
    use mixlive_effects::ReverbConfig;
    let config = VocalConfig { reverb: ReverbConfig { mix: 0.5, decay_secs: 1.2, predelay_ms: 0.0 }, ..VocalConfig::default() };
    let mut mic = vec![0.0; 4096];
    mic[0] = 0.5;
    let mut expected_left = vec![0.0; mic.len()];
    let mut expected_right = vec![0.0; mic.len()];
    VocalChain::new(48000, config).unwrap().process(&mic, &mut expected_left, &mut expected_right).unwrap();
    let health = Arc::new(Health::default());
    let controls = Arc::new(LiveControls::new(0.0, [Some(0.0); 3], true, 1, config).unwrap());
    controls.apply("route app1 broadcast on".parse().unwrap()).unwrap();
    let (writer, _reader) = channel(48000, 48000, 30, 3, health.clone()).unwrap();
    controls.apply("route aux send on".parse().unwrap()).unwrap();
    let mut capture = Capture::new(VocalChain::new(48000, config).unwrap(), 48000,
        vec![(writer, vec![0, 1, 2])], health.clone(), controls).unwrap();
    let mut position = 0;
    let mut saw_stereo = false;
    for block in mic.chunks(137) {
        let end = position + block.len();
        let aux = vec![[0.125, -0.25]; block.len()];
        let app = vec![[-0.0625, 0.25]; block.len()];
        capture.push_with_effect(block, Some(&aux), &[&app], &mut |left, right| {
            assert_eq!(&*left, &expected_left[position..end]);
            assert_eq!(&*right, &expected_right[position..end]);
            saw_stereo |= left.iter().zip(right.iter()).any(|(l, r)| (l - r).abs() > 0.001);
            for (l, r) in left.iter_mut().zip(right) { *l *= 0.5; *r *= 0.25; }
            true
        });
        for mix in 0..3 {
            let output = capture.engine.output(mix).unwrap();
            let extra = match mix { 1 => aux[0], 2 => app[0], _ => [0.0; 2] };
            for index in 0..block.len() {
                assert!((output.left[index] - (expected_left[position + index] * 0.5 + extra[0])).abs() < 1e-6);
                assert!((output.right[index] - (expected_right[position + index] * 0.25 + extra[1])).abs() < 1e-6);
            }
        }
        position = end;
    }
    assert!(saw_stereo && !health.failed());
}

#[test]
fn live_vocal_controls_reach_the_audio_thread_without_touching_other_sources() {
    use mixlive_effects::VocalParameter;
    let config = VocalConfig::default();
    let health = Arc::new(Health::default());
    let controls = Arc::new(LiveControls::new(0.0, [Some(0.0), None, Some(0.0)], true, 1, config).unwrap());
    controls.apply("route app1 broadcast on".parse().unwrap()).unwrap();
    let (writer, _reader) = channel(48000, 48000, 30, 2, health.clone()).unwrap();
    controls.apply("route aux broadcast on".parse().unwrap()).unwrap();
    let mut capture = Capture::new(VocalChain::new(48000, config).unwrap(), 48000,
        vec![(writer, vec![0, 2])], health.clone(), controls.clone()).unwrap();
    let mic = [0.25; 256];
    let aux = [[0.125, -0.25]; 256];
    let app = [[-0.0625, 0.125]; 256];
    capture.push(&mic, Some(&aux), &[&app]);
    assert_eq!(capture.engine.output(0).unwrap().left[255], 0.25);
    controls.apply("vocal trim -6".parse().unwrap()).unwrap();
    capture.push(&mic, Some(&aux), &[&app]);
    let middle = capture.engine.output(0).unwrap().left[255];
    assert!(middle < 0.25 && middle > 0.25 * 10.0_f32.powf(-6.0 / 20.0));
    capture.push(&mic, Some(&aux), &[&app]);
    let expected = 0.25 * 10.0_f32.powf(-6.0 / 20.0);
    assert!((capture.engine.output(0).unwrap().left[255] - expected).abs() < 1e-6);
    let broadcast = capture.engine.output(1).unwrap();
    assert!((broadcast.left[255] - (expected + aux[255][0] + app[255][0])).abs() < 1e-6);
    assert!((broadcast.right[255] - (expected + aux[255][1] + app[255][1])).abs() < 1e-6);
    assert_eq!(capture.vocal.config().parameter(VocalParameter::Trim), -6.0);
    assert!(!health.failed());
}

#[test]
fn live_eq_reaches_both_voice_sends_and_leaves_aux_and_app_audio_unchanged() {
    let config = VocalConfig { preset: Preset::Warm, compressor_threshold_db: 0.0,
        deess_threshold_db: 0.0, ..VocalConfig::default() };
    let health = Arc::new(Health::default());
    let controls = Arc::new(LiveControls::new(0.0, [Some(0.0), None, Some(0.0)], true, 1, config).unwrap());
    for command in ["route aux broadcast on", "route app1 broadcast on"] {
        controls.apply(command.parse().unwrap()).unwrap();
    }
    let (writer, _reader) = channel(48000, 48000, 100, 2, health.clone()).unwrap();
    let mut capture = Capture::new(VocalChain::new(48000, config).unwrap(), 48000,
        vec![(writer, vec![0, 2])], health.clone(), controls.clone()).unwrap();
    let input: Vec<f32> = (0..8192).map(|i| (std::f32::consts::TAU * 180.0 * i as f32 / 48000.0).sin() * 0.05).collect();
    let aux = vec![[0.125, -0.25]; input.len()];
    let app = vec![[-0.0625, 0.125]; input.len()];
    capture.push(&input[..4096], Some(&aux[..4096]), &[&app[..4096]]);
    let before = capture.engine.output(0).unwrap().sample_peak();
    for command in ["vocal eq-hpf 140", "vocal eq-body -6", "vocal eq-presence 3", "vocal eq-air 2"] {
        controls.apply(command.parse().unwrap()).unwrap();
    }
    for start in (4096..input.len()).step_by(137) {
        let end = (start + 137).min(input.len());
        capture.push(&input[start..end], Some(&aux[start..end]), &[&app[start..end]]);
    }
    assert!(!health.failed());
    assert_eq!(capture.vocal.config().eq, controls.vocal_config().eq);
    let mut expected = vec![0.0; input.len()];
    VocalChain::new(48000, controls.vocal_config()).unwrap()
        .process(&input, &mut expected, &mut vec![0.0; input.len()]).unwrap();
    let monitor = capture.engine.output(0).unwrap();
    let broadcast = capture.engine.output(1).unwrap();
    assert!(monitor.sample_peak() < before * 0.7);
    for (i, &voice) in expected[input.len() - monitor.left.len()..].iter().enumerate() {
        assert!((monitor.left[i] - voice).abs() < 1e-5);
        assert_eq!(monitor.left[i], monitor.right[i]);
        assert!((broadcast.left[i] - voice - 0.0625).abs() < 1e-5);
        assert!((broadcast.right[i] - voice + 0.125).abs() < 1e-5);
    }
}

#[test]
fn supplemental_trim_ramps_once_before_send_delay_and_keeps_input_meters_raw() {
    let health = Arc::new(Health::default());
    let controls = Arc::new(LiveControls::new(0.0, [Some(0.0), None, Some(0.0)], true, 2, VocalConfig::default()).unwrap());
    for command in ["route aux output on", "route aux broadcast on", "route app1 broadcast on",
        "route app2 output on", "trim aux -6", "trim app1 -12", "trim app2 -18"] {
        controls.apply(command.parse().unwrap()).unwrap();
    }
    let (writer, _reader) = channel(48000, 48000, 30, 2, health.clone()).unwrap();
    let mut capture = Capture::new(VocalChain::new(48000, VocalConfig::default()).unwrap(), 48000,
        vec![(writer, vec![0, 2])], health.clone(), controls.clone()).unwrap();
    assert_eq!(capture.prepare_voice_delay(256, 48000).unwrap(), 256);
    let mic = [0.0; 256];
    let aux = [[0.25, -0.125]; 256];
    let app1 = [[0.125, 0.25]; 256];
    let app2 = [[-0.0625, 0.125]; 256];
    let gain = |db: f32| 10.0_f32.powf(db / 20.0);
    capture.push(&mic, Some(&aux), &[&app1, &app2]);
    let monitor = capture.engine.output(0).unwrap();
    assert!((monitor.left[0] - (0.25 * gain(-6.0) - 0.0625 * gain(-18.0))).abs() < 1e-6);
    assert_eq!(capture.engine.output(1).unwrap().sample_peak(), 0.0);
    controls.apply("trim aux -20".parse().unwrap()).unwrap();
    controls.apply("trim app1 0".parse().unwrap()).unwrap();
    capture.push(&mic, Some(&aux), &[&app1, &app2]);
    // Delayed samples retain their original source trim, unlike post-delay mute.
    let broadcast = capture.engine.output(1).unwrap();
    assert!((broadcast.left[255] - (0.25 * gain(-6.0) + 0.125 * gain(-12.0))).abs() < 1e-6);
    let monitor = capture.engine.output(0).unwrap();
    assert!(monitor.left[0] > monitor.left[239]);
    assert!((monitor.left[239] - (0.25 * gain(-20.0) - 0.0625 * gain(-18.0))).abs() < 1e-6);
    capture.push(&mic, Some(&aux), &[&app1, &app2]);
    let broadcast = capture.engine.output(1).unwrap();
    assert!(broadcast.right[0] < broadcast.right[239]);
    assert!((broadcast.left[239] - (0.25 * gain(-20.0) + 0.125)).abs() < 1e-6);
    assert!((broadcast.right[239] - (-0.125 * gain(-20.0) + 0.25)).abs() < 1e-6);
    assert_eq!(f32::from_bits(health.aux_peak.load(Ordering::Relaxed)), 0.25);
    assert_eq!(f32::from_bits(health.app_peaks[0].load(Ordering::Relaxed)), 0.25);
    assert_eq!(f32::from_bits(health.app_peaks[1].load(Ordering::Relaxed)), 0.125);
    assert!(!health.failed());
}

#[test]
fn live_echo_reaches_voice_chain_only_and_mute_covers_its_tail() {
    let config = VocalConfig { echo: mixlive_effects::EchoConfig { mix: 0.0, time_ms: 10.0, feedback: 0.0 }, ..VocalConfig::default() };
    let health = Arc::new(Health::default());
    let controls = Arc::new(LiveControls::new(0.0, [Some(0.0), None, Some(0.0)], true, 1, config).unwrap());
    for command in ["route aux broadcast on", "route app1 broadcast on"] {
        controls.apply(command.parse().unwrap()).unwrap();
    }
    let (writer, _reader) = channel(48000, 48000, 30, 2, health.clone()).unwrap();
    let mut capture = Capture::new(VocalChain::new(48000, config).unwrap(), 48000,
        vec![(writer, vec![0, 2])], health.clone(), controls.clone()).unwrap();
    let mic = [0.5; 240];
    let silence = [0.0; 240];
    let aux = [[0.125, -0.25]; 240];
    let app = [[-0.0625, 0.125]; 240];
    // First 480 samples fill the 10 ms echo buffer.
    for _ in 0..2 { capture.push(&mic, Some(&aux), &[&app]); }
    controls.apply("vocal echo 1".parse().unwrap()).unwrap();
    for block in 0..2 {
        capture.push_with_effect(&silence, Some(&aux), &[&app], &mut |left, right| {
            assert_eq!(left, right);
            if block == 1 { assert_eq!(left[239], 0.5); }
            for (l, r) in left.iter_mut().zip(right) { *l *= 0.5; *r *= -0.25; }
            true
        });
    }
    let monitor = capture.engine.output(0).unwrap();
    let broadcast = capture.engine.output(1).unwrap();
    assert_eq!([monitor.left[239], monitor.right[239]], [0.25, -0.125]);
    assert_eq!([broadcast.left[239], broadcast.right[239]], [0.3125, -0.25]);
    // Refill the echo and mute voice before the delayed signal appears.
    for _ in 0..2 { capture.push(&mic, Some(&aux), &[&app]); }
    controls.apply("mute mic".parse().unwrap()).unwrap();
    capture.push(&silence, Some(&aux), &[&app]);
    assert!(capture.engine.output(0).unwrap().sample_peak() > 0.0);
    let monitor = capture.engine.output(0).unwrap();
    assert_eq!([monitor.left[239], monitor.right[239]], [0.0; 2]);
    let broadcast = capture.engine.output(1).unwrap();
    assert_eq!([broadcast.left[239], broadcast.right[239]], [0.0625, -0.125]);
    assert!(!health.failed());
}

#[test]
fn pitch_delay_and_reported_vst3_delay_align_outgoing_aux_and_apps_once() {
    const PLUGIN_DELAY: usize = 71;
    for plugin_delay in [0, PLUGIN_DELAY] {
        let health = Arc::new(Health::default());
        let controls = Arc::new(LiveControls::new(0.0, [Some(0.0), None, Some(0.0)], true, 1, VocalConfig::default()).unwrap());
        for command in ["route aux output on", "route aux broadcast on", "route app1 output on", "route app1 broadcast on"] {
            controls.apply(command.parse().unwrap()).unwrap();
        }
        let (writer, _reader) = channel(48000, 48000, 100, 2, health.clone()).unwrap();
        let mut capture = Capture::new(VocalChain::new(48000, VocalConfig::default()).unwrap(), 48000,
            vec![(writer, vec![0, 2])], health.clone(), controls).unwrap();
        let pitch = capture.prepare_pitch(48000, PitchConfig { strength: 1.0, ..PitchConfig::default() }).unwrap();
        assert_eq!(pitch, 2048);
        let total = pitch + plugin_delay;
        assert!(capture.prepare_voice_delay(96000, 48000).is_err());
        assert_eq!(capture.prepare_voice_delay(plugin_delay as u64, 48000).unwrap(), total);
        let mut history = [0.0; PLUGIN_DELAY];
        let mut cursor = 0;
        let count = total + 384;
        let mut mic = vec![0.0; count];
        let mut aux = vec![[0.0; 2]; count];
        let mut app = aux.clone();
        mic[17] = 0.5;
        aux[17] = [0.125, -0.25];
        app[17] = [-0.0625, 0.125];
        let mut start = 0;
        for block in mic.chunks(137) {
            let end = start + block.len();
            capture.push_with_effect(block, Some(&aux[start..end]), &[&app[start..end]], &mut |left, right| {
                if plugin_delay != 0 {
                    for (l, r) in left.iter_mut().zip(right) {
                        let delayed = std::mem::replace(&mut history[cursor], *l);
                        cursor = (cursor + 1) % PLUGIN_DELAY;
                        *l = delayed; *r = delayed;
                    }
                }
                true
            });
            for mix in [0, 1] {
                let output = capture.engine.output(mix).unwrap();
                for (i, (&l, &r)) in output.left.iter().zip(output.right).enumerate() {
                    let expected = match (mix, start + i) {
                        (0, 17) => [0.0625, -0.125],
                        (0, frame) if frame == 17 + total => [0.5, 0.5],
                        (1, frame) if frame == 17 + total => [0.5625, 0.375],
                        _ => [0.0; 2],
                    };
                    assert_eq!([l, r], expected, "mix {mix}, frame {}", start + i);
                }
            }
            start = end;
        }
        assert_eq!(f32::from_bits(health.input_peak.load(Ordering::Relaxed)), 0.5);
        assert!(!health.failed());
    }
}
