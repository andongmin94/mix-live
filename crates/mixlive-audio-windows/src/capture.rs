use std::sync::{Arc, atomic::{AtomicU32, Ordering}};
use mixlive_core::{ChannelLayout, Engine, Input, Mix, Scene, Send, Source};
use mixlive_effects::{Meters, VocalChain};
use crate::{bridge::{Fault, Health, OutputWriter}, config::AuxDestination, control::{GainRamp, LiveCommand, LiveControls, RouteMix, RouteSource}};

pub const CAPTURE_BLOCK: usize = 256;

#[derive(Default)]
pub struct VocalStats {
    peak: AtomicU32,
    compression: AtomicU32,
    deessing: AtomicU32,
}
impl VocalStats {
    fn record(&self, meters: Meters) {
        self.peak.fetch_max(meters.output_peak.to_bits(), Ordering::Relaxed);
        self.compression.fetch_max(meters.compression_db.to_bits(), Ordering::Relaxed);
        self.deessing.fetch_max(meters.deessing_db.to_bits(), Ordering::Relaxed);
    }
    /// UI/control thread: maxima since its previous read, not a loudness measurement.
    pub fn take(&self) -> Meters {
        Meters { output_peak: f32::from_bits(self.peak.swap(0, Ordering::Relaxed)),
            compression_db: f32::from_bits(self.compression.swap(0, Ordering::Relaxed)),
            deessing_db: f32::from_bits(self.deessing.swap(0, Ordering::Relaxed)) }
    }
}

/// The input clock processes the vocal ONCE, then renders independent destination mixes.
pub struct Capture {
    engine: Engine,
    vocal: VocalChain,
    scratch: [f32; CAPTURE_BLOCK],
    aux_enabled: bool,
    aux_left: [f32; CAPTURE_BLOCK],
    aux_right: [f32; CAPTURE_BLOCK],
    controls: Arc<LiveControls>,
    source_mutes: [GainRamp; 2],
    route_count: usize,
    route_ramps: [GainRamp; 4],
    route_envelopes: [[f32; CAPTURE_BLOCK]; 4],
    // A writer belongs to a physical endpoint, not necessarily to a single mix.
    outputs: Vec<(OutputWriter, Vec<usize>)>,
    health: Arc<Health>,
    pub vocal_stats: Arc<VocalStats>,
}
impl Capture {
    pub fn new(vocal: VocalChain, sample_rate: u32, aux: Option<(f32, AuxDestination)>, outputs: Vec<(OutputWriter, Vec<usize>)>, health: Arc<Health>, controls: Arc<LiveControls>) -> Result<Self, String> {
        let mix_count = controls.mix_count();
        if !(8_000..=192_000).contains(&sample_rate) || aux.is_some() != controls.has_aux() {
            return Err("source controls do not match the capture configuration".into());
        }
        if outputs.is_empty() || outputs.iter().any(|(writer, mixes)| {
            mixes.len() != writer.mix_count() || mixes.iter().any(|&mix| mix >= mix_count)
        }) { return Err("endpoint mix bindings do not match their clock bridges".into()); }
        let mut scene = Scene {
            // Input trim remains in VocalChain. Final master/send attenuation is applied
            // after each output queue, so live output mute never waits for that queue to drain.
            sources: vec![Source { name: "Processed mic".into(), channels: ChannelLayout::Mono, gain_db: 0.0, muted: false }],
            mixes: (0..mix_count).map(|index| Mix {
                name: format!("Output {index}"), gain_db: 0.0, muted: false, excluded_sources: vec![],
            }).collect(),
            sends: (0..mix_count).map(|mix| Send { source: 0, mix, gain_db: 0.0, enabled: true }).collect(),
        };
        if let Some((gain_db, destination)) = aux {
            if destination.includes(1) && mix_count != 2 {
                return Err("AUX send destination needs a second mix".into());
            }
            scene.sources.push(Source { name: "AUX".into(), channels: ChannelLayout::Stereo, gain_db, muted: false });
            // Compile every existing source/mix pair once. Initial off routes have zero
            // envelopes, not disabled core routes that runtime controls could accidentally bypass.
            scene.sends.extend((0..mix_count)
                .map(|mix| Send { source: 1, mix, gain_db: 0.0, enabled: true }));
        }
        let engine = Engine::new(&scene, CAPTURE_BLOCK).map_err(|e| e.to_string())?;
        // Initialize AUX routing on the setup thread, before any capture/playback starts.
        if let Some((_, destination)) = aux {
            for (mix, target) in [RouteMix::Output, RouteMix::Send].into_iter().take(mix_count).enumerate() {
                controls.apply(LiveCommand::Route { source: RouteSource::Aux, mix: target, enabled: destination.includes(mix) })?;
            }
        }
        let route_count = scene.sends.len();
        let targets = controls.route_targets();
        let route_ramps = std::array::from_fn(|index| GainRamp::new(sample_rate,
            if index < route_count { targets[index / mix_count * 2 + index % mix_count] } else { 0.0 }));
        let source_mutes = controls.source_targets().map(|gain| GainRamp::new(sample_rate, gain));
        Ok(Self { engine, vocal, scratch: [0.0; CAPTURE_BLOCK], aux_enabled: aux.is_some(),
            aux_left: [0.0; CAPTURE_BLOCK], aux_right: [0.0; CAPTURE_BLOCK], outputs,
            controls, source_mutes, route_count, route_ramps, route_envelopes: [[0.0; CAPTURE_BLOCK]; 4], health, vocal_stats: Arc::new(VocalStats::default()) })
    }
    /// AUX frames must already be on the mic's processing clock. Only the mic enters VocalChain.
    pub fn push(&mut self, samples: &[f32], aux: Option<&[[f32; 2]]>) {
        if self.health.failed() { return; }
        if aux.is_some() != self.aux_enabled || aux.is_some_and(|frames| frames.len() != samples.len()) {
            self.health.fail(Fault::Input); return;
        }
        for (block_index, block) in samples.chunks(CAPTURE_BLOCK).enumerate() {
            for (ramp, target) in self.source_mutes.iter_mut().zip(self.controls.source_targets()) { ramp.set(target); }
            let targets = self.controls.route_targets();
            let mix_count = self.controls.mix_count();
            for (index, ramp) in self.route_ramps[..self.route_count].iter_mut().enumerate() {
                ramp.set(targets[index / mix_count * 2 + index % mix_count]);
                for gain in &mut self.route_envelopes[index][..block.len()] { *gain = ramp.next_gain(); }
            }
            let envelopes: [&[f32]; 4] = std::array::from_fn(|index| &self.route_envelopes[index][..block.len()]);
            let envelopes = &envelopes[..self.route_count];
            let processed = &mut self.scratch[..block.len()];
            processed.copy_from_slice(block);
            let Ok(meters) = self.vocal.process(processed) else { self.health.fail(Fault::Input); return; };
            // Keep the voice detectors running while muted; do not reset or bypass its DSP.
            for sample in processed.iter_mut() { *sample *= self.source_mutes[0].next_gain(); }
            let rendered = if let Some(aux) = aux {
                let start = block_index * CAPTURE_BLOCK;
                let mut peak = 0.0_f32;
                for (index, frame) in aux[start..start + block.len()].iter().enumerate() {
                    let gain = self.source_mutes[1].next_gain();
                    self.aux_left[index] = frame[0] * gain;
                    self.aux_right[index] = frame[1] * gain;
                    peak = peak.max(frame[0].abs()).max(frame[1].abs());
                }
                self.health.aux_peak.fetch_max(peak.to_bits(), Ordering::Relaxed);
                self.engine.render_with_send_gains(block.len(), &[Input::Mono(processed), Input::Stereo {
                    left: &self.aux_left[..block.len()], right: &self.aux_right[..block.len()],
                }], envelopes)
            } else { self.engine.render_with_send_gains(block.len(), &[Input::Mono(processed)], envelopes) };
            if rendered.is_err() {
                self.health.fail(Fault::Input); return;
            }
            let peak = block.iter().fold(0.0_f32, |p, x| p.max(x.abs()));
            self.health.input_peak.fetch_max(peak.to_bits(), Ordering::Relaxed);
            self.health.captured_frames.fetch_add(block.len() as u64, Ordering::Relaxed);
            self.vocal_stats.record(meters);
            for (writer, mixes) in &mut self.outputs {
                for frame in 0..block.len() {
                    let mut packed = [0.0; 4];
                    for (lane, mix) in mixes.iter().enumerate() {
                        let Some(output) = self.engine.output(*mix) else { self.health.fail(Fault::Routing); return; };
                        packed[lane * 2] = output.left[frame];
                        packed[lane * 2 + 1] = output.right[frame];
                    }
                    // Publish both mixes atomically as one clocked frame, never separate queues.
                    writer.push(packed);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
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
            // Muting cannot hide invalid input or turn it into a silent successful route.
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
        // Remove MR only from the call; monitor MR remains. DSP keeps its continuous history.
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
        // Exercise the compact two-source/one-mix layout as well as ordinary mono-only routing.
        for has_aux in [false, true] {
            let run = |chunks: &[usize]| {
                let health = Arc::new(Health::default());
                let controls = Arc::new(LiveControls::new(0.0, None, has_aux).unwrap());
                let (writer, _reader) = channel(48_000, 48_000, 30, 1, health.clone()).unwrap();
                let mut capture = Capture::new(VocalChain::new(48000, VocalConfig::default()).unwrap(), 48000,
                    has_aux.then_some((0.0, AuxDestination::Output)), vec![(writer, vec![0])], health.clone(), controls.clone()).unwrap();
                controls.apply("route mic output off".parse().unwrap()).unwrap();
                controls.apply("level mic output -6".parse().unwrap()).unwrap(); // must stay off
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

}
