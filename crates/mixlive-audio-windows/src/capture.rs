use std::sync::{Arc, atomic::{AtomicU32, Ordering}};
use mixlive_core::{ChannelLayout, Engine, Input, Mix, Scene, Send, Source};
use mixlive_effects::{Meters, VocalChain};
use crate::{bridge::{Fault, Health, OutputWriter}, config::AuxDestination};

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
    // A writer belongs to a physical endpoint, not necessarily to a single mix.
    outputs: Vec<(OutputWriter, Vec<usize>)>,
    health: Arc<Health>,
    pub vocal_stats: Arc<VocalStats>,
}
impl Capture {
    pub fn new(vocal: VocalChain, output_gain_db: f32, send_gain_db: Option<f32>, aux: Option<(f32, AuxDestination)>, outputs: Vec<(OutputWriter, Vec<usize>)>, health: Arc<Health>) -> Result<Self, String> {
        let gains: Vec<f32> = std::iter::once(0.0).chain(send_gain_db).collect();
        if outputs.is_empty() || outputs.iter().any(|(writer, mixes)| {
            mixes.len() != writer.mix_count() || mixes.iter().any(|&mix| mix >= gains.len())
        }) { return Err("endpoint mix bindings do not match their clock bridges".into()); }
        let mut scene = Scene {
            // Input trim belongs before the vocal detectors; final attenuation belongs after DSP.
            sources: vec![Source { name: "Processed mic".into(), channels: ChannelLayout::Mono, gain_db: 0.0, muted: false }],
            mixes: gains.iter().enumerate().map(|(index, _)| Mix {
                name: format!("Output {index}"), gain_db: output_gain_db, muted: false, excluded_sources: vec![],
            }).collect(),
            sends: gains.iter().enumerate().map(|(mix, gain_db)| Send { source: 0, mix, gain_db: *gain_db, enabled: true }).collect(),
        };
        if let Some((gain_db, destination)) = aux {
            if destination.includes(1) && gains.len() != 2 {
                return Err("AUX send destination needs a second mix".into());
            }
            scene.sources.push(Source { name: "AUX".into(), channels: ChannelLayout::Stereo, gain_db, muted: false });
            scene.sends.extend(gains.iter().enumerate().filter(|(mix, _)| destination.includes(*mix))
                .map(|(mix, gain_db)| Send { source: 1, mix, gain_db: *gain_db, enabled: true }));
        }
        let engine = Engine::new(&scene, CAPTURE_BLOCK).map_err(|e| e.to_string())?;
        Ok(Self { engine, vocal, scratch: [0.0; CAPTURE_BLOCK], aux_enabled: aux.is_some(),
            aux_left: [0.0; CAPTURE_BLOCK], aux_right: [0.0; CAPTURE_BLOCK], outputs,
            health, vocal_stats: Arc::new(VocalStats::default()) })
    }
    /// AUX frames must already be on the mic's processing clock. Only the mic enters VocalChain.
    pub fn push(&mut self, samples: &[f32], aux: Option<&[[f32; 2]]>) {
        if self.health.failed() { return; }
        if aux.is_some() != self.aux_enabled || aux.is_some_and(|frames| frames.len() != samples.len()) {
            self.health.fail(Fault::Input); return;
        }
        for (block_index, block) in samples.chunks(CAPTURE_BLOCK).enumerate() {
            let processed = &mut self.scratch[..block.len()];
            processed.copy_from_slice(block);
            let Ok(meters) = self.vocal.process(processed) else { self.health.fail(Fault::Input); return; };
            let rendered = if let Some(aux) = aux {
                let start = block_index * CAPTURE_BLOCK;
                let mut peak = 0.0_f32;
                for (index, frame) in aux[start..start + block.len()].iter().enumerate() {
                    self.aux_left[index] = frame[0];
                    self.aux_right[index] = frame[1];
                    peak = peak.max(frame[0].abs()).max(frame[1].abs());
                }
                self.health.aux_peak.fetch_max(peak.to_bits(), Ordering::Relaxed);
                self.engine.render(block.len(), &[Input::Mono(processed), Input::Stereo {
                    left: &self.aux_left[..block.len()], right: &self.aux_right[..block.len()],
                }])
            } else { self.engine.render(block.len(), &[Input::Mono(processed)]) };
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
    use crate::bridge::{channel, QUANTUM};
    use mixlive_effects::{Preset, VocalConfig};

    #[test]
    fn one_capture_feeds_two_independent_mixes_without_dry_bypass() {
        let health = Arc::new(Health::default());
        let (first, mut monitor) = channel(48_000, 48_000, 10, 1, health.clone()).unwrap();
        let (second, mut send) = channel(48_000, 48_000, 10, 1, health.clone()).unwrap();
        let vocal = VocalChain::new(48000, VocalConfig { input_gain_db: -6.0, ..VocalConfig::default() }).unwrap();
        let mut capture = Capture::new(vocal, 0.0, Some(-6.0), None, vec![(first, vec![0]), (second, vec![1])], health.clone()).unwrap();
        let prime = (monitor.queue_ms * 48.0).round() as usize + 1024;
        capture.push(&vec![0.4; prime], None);
        for _ in 0..4 * QUANTUM { monitor.next_frame(); send.next_frame(); }
        let a = monitor.next_frame()[0];
        let b = send.next_frame()[0];
        assert!((a - 0.4 * 10.0_f32.powf(-6.0 / 20.0)).abs() < 0.001);
        assert!((b / a - 10.0_f32.powf(-6.0 / 20.0)).abs() < 0.001);
        capture.push(&[f32::NAN], None);
        assert!(health.failed());
        assert_eq!(monitor.next_frame(), [0.0; 4]);
        assert_eq!(send.next_frame(), [0.0; 4]);
    }

    #[test]
    fn vocal_is_processed_once_before_sends_and_output_attenuation() {
        let config = VocalConfig { preset: Preset::Bright, ..VocalConfig::default() };
        let input: Vec<f32> = (0..1024).map(|i| (i as f32 * 0.17).sin() * 0.4).collect();
        let mut expected = input.clone();
        let mut reference = VocalChain::new(48000, config).unwrap();
        reference.process(&mut expected).unwrap();
        let health = Arc::new(Health::default());
        let (first, _a) = channel(48000, 48000, 30, 1, health.clone()).unwrap();
        let (second, _b) = channel(48000, 48000, 30, 1, health.clone()).unwrap();
        let mut capture = Capture::new(VocalChain::new(48000, config).unwrap(), -18.0, Some(-6.0), None, vec![(first, vec![0]), (second, vec![1])], health.clone()).unwrap();
        capture.push(&input, None);
        assert!(!health.failed());
        let a = capture.engine.output(0).unwrap();
        let b = capture.engine.output(1).unwrap();
        let final_gain = 10.0_f32.powf(-18.0 / 20.0);
        let send_gain = 10.0_f32.powf(-6.0 / 20.0);
        for (index, sample) in expected[768..].iter().enumerate() {
            assert!((a.left[index] - sample * final_gain).abs() < 1e-6);
            assert!((b.left[index] - a.left[index] * send_gain).abs() < 1e-6);
            assert_eq!(a.left[index], a.right[index]);
        }
        let meters = capture.vocal_stats.take();
        assert!(meters.compression_db > 0.0);
        assert!(meters.output_peak > 0.0);
        assert_eq!(capture.vocal_stats.take().output_peak, 0.0);
    }

    #[test]
    fn shared_endpoint_packs_two_mixes_with_independent_gains_before_resampling() {
        use crate::config::{OutputMap, output_sample};
        let health = Arc::new(Health::default());
        let (writer, mut reader) = channel(48_000, 48_000, 10, 2, health.clone()).unwrap();
        let vocal = VocalChain::new(48000, VocalConfig::default()).unwrap();
        let mut capture = Capture::new(vocal, -6.0, Some(-12.0), None, vec![(writer, vec![0, 1])], health.clone()).unwrap();
        let prime = (reader.queue_ms * 48.0).round() as usize + 1024;
        for chunk in vec![0.4; prime].chunks(137) { capture.push(chunk, None); }
        for _ in 0..4 * QUANTUM { reader.next_frame(); }
        let maps = [OutputMap::Stereo(0, 1), OutputMap::Stereo(2, 3)];
        let packed = reader.next_frame();
        let values: Vec<f32> = (0..6).map(|c| output_sample(&maps, c, &packed)).collect();
        assert!((values[0] - 0.4 * 10.0_f32.powf(-6.0 / 20.0)).abs() < 0.001);
        assert_eq!(values[0], values[1]);
        assert!((values[2] / values[0] - 10.0_f32.powf(-12.0 / 20.0)).abs() < 1e-6);
        assert_eq!(values[2], values[3]);
        assert_eq!(&values[4..], &[0.0; 2]);
        assert_eq!(health.captured_frames.load(Ordering::Relaxed), prime as u64);
        capture.push(&[f32::NAN], None);
        assert!(health.failed());
        assert_eq!(reader.next_frame(), [0.0; 4]);
    }
    #[test]
    fn aux_is_stereo_selective_and_never_passes_through_vocal_processing() {
        for destination in [AuxDestination::Output, AuxDestination::Send, AuxDestination::Both] {
            let health = Arc::new(Health::default());
            let (writer, _reader) = channel(48_000, 48_000, 30, 2, health.clone()).unwrap();
            let config = VocalConfig { preset: Preset::Bright, input_gain_db: 6.0, ..VocalConfig::default() };
            let input: Vec<f32> = (0..613).map(|i| (i as f32 * 0.17).sin() * 0.4).collect();
            let aux = vec![[2.0, -1.0]; input.len()]; // float headroom is reduced only by the source gain
            let mut expected = input.clone();
            VocalChain::new(48000, config).unwrap().process(&mut expected).unwrap();
            let mut capture = Capture::new(VocalChain::new(48000, config).unwrap(), -18.0, Some(-6.0),
                Some((-12.0, destination)), vec![(writer, vec![0, 1])], health.clone()).unwrap();
            // Irregular callback lengths, including a final partial block.
            let mut start = 0;
            for count in [1, 137, 256, 219] {
                capture.push(&input[start..start + count], Some(&aux[start..start + count]));
                start += count;
            }
            assert!(!health.failed());
            for mix in 0..2 {
                let output = capture.engine.output(mix).unwrap();
                let gain = 10.0_f32.powf((-18.0 - 6.0 * mix as f32) / 20.0);
                for (index, expected) in expected[394..].iter().enumerate() {
                    let aux_gain = if destination.includes(mix) { 10.0_f32.powf(-12.0 / 20.0) } else { 0.0 };
                    assert!((output.left[index] - (expected + 2.0 * aux_gain) * gain).abs() < 1e-6);
                    assert!((output.right[index] - (expected - aux_gain) * gain).abs() < 1e-6);
                }
            }
            assert_eq!(f32::from_bits(health.aux_peak.load(Ordering::Relaxed)), 2.0);
            capture.push(&[0.1], Some(&[[f32::NAN, 0.0]]));
            assert!(health.failed());
        }
    }

}
