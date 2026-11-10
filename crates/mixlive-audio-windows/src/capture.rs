use std::sync::{Arc, atomic::{AtomicU32, Ordering}};
use mixlive_core::{ChannelLayout, Engine, Input, Mix, Scene, Send, Source};
use mixlive_effects::{Meters, VocalChain};
use crate::bridge::{Fault, Health, OutputWriter};

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
    outputs: Vec<OutputWriter>,
    health: Arc<Health>,
    pub vocal_stats: Arc<VocalStats>,
}
impl Capture {
    pub fn new(vocal: VocalChain, output_gain_db: f32, outputs: Vec<(OutputWriter, f32)>, health: Arc<Health>) -> Result<Self, String> {
        let scene = Scene {
            // Input trim belongs before the vocal detectors; final attenuation belongs after DSP.
            sources: vec![Source { name: "Processed mic".into(), channels: ChannelLayout::Mono, gain_db: 0.0, muted: false }],
            mixes: outputs.iter().enumerate().map(|(index, _)| Mix {
                name: format!("Output {index}"), gain_db: output_gain_db, muted: false, excluded_sources: vec![],
            }).collect(),
            sends: outputs.iter().enumerate().map(|(mix, (_, gain_db))| Send { source: 0, mix, gain_db: *gain_db, enabled: true }).collect(),
        };
        let engine = Engine::new(&scene, CAPTURE_BLOCK).map_err(|e| e.to_string())?;
        Ok(Self { engine, vocal, scratch: [0.0; CAPTURE_BLOCK], outputs: outputs.into_iter().map(|(writer, _)| writer).collect(),
            health, vocal_stats: Arc::new(VocalStats::default()) })
    }
    pub fn push_mono(&mut self, samples: &[f32]) {
        if self.health.failed() { return; }
        for block in samples.chunks(CAPTURE_BLOCK) {
            let processed = &mut self.scratch[..block.len()];
            processed.copy_from_slice(block);
            let Ok(meters) = self.vocal.process(processed) else { self.health.fail(Fault::Input); return; };
            if self.engine.render(block.len(), &[Input::Mono(processed)]).is_err() {
                self.health.fail(Fault::Input); return;
            }
            let peak = block.iter().fold(0.0_f32, |p, x| p.max(x.abs()));
            self.health.input_peak.fetch_max(peak.to_bits(), Ordering::Relaxed);
            self.health.captured_frames.fetch_add(block.len() as u64, Ordering::Relaxed);
            self.vocal_stats.record(meters);
            for (mix, writer) in self.outputs.iter_mut().enumerate() {
                let Some(output) = self.engine.output(mix) else { self.health.fail(Fault::Routing); return; };
                for (&left, &right) in output.left.iter().zip(output.right) { writer.push([left, right]); }
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
        let (first, mut monitor) = channel(48_000, 48_000, 10, health.clone()).unwrap();
        let (second, mut send) = channel(48_000, 48_000, 10, health.clone()).unwrap();
        let vocal = VocalChain::new(48000, VocalConfig { input_gain_db: -6.0, ..VocalConfig::default() }).unwrap();
        let mut capture = Capture::new(vocal, 0.0, vec![(first, 0.0), (second, -6.0)], health.clone()).unwrap();
        let prime = (monitor.queue_ms * 48.0).round() as usize + 1024;
        capture.push_mono(&vec![0.4; prime]);
        for _ in 0..4 * QUANTUM { monitor.next_frame(); send.next_frame(); }
        let a = monitor.next_frame()[0];
        let b = send.next_frame()[0];
        assert!((a - 0.4 * 10.0_f32.powf(-6.0 / 20.0)).abs() < 0.001);
        assert!((b / a - 10.0_f32.powf(-6.0 / 20.0)).abs() < 0.001);
        capture.push_mono(&[f32::NAN]);
        assert!(health.failed());
        assert_eq!(monitor.next_frame(), [0.0; 2]);
        assert_eq!(send.next_frame(), [0.0; 2]);
    }

    #[test]
    fn vocal_is_processed_once_before_sends_and_output_attenuation() {
        let config = VocalConfig { preset: Preset::Bright, ..VocalConfig::default() };
        let input: Vec<f32> = (0..1024).map(|i| (i as f32 * 0.17).sin() * 0.4).collect();
        let mut expected = input.clone();
        let mut reference = VocalChain::new(48000, config).unwrap();
        reference.process(&mut expected).unwrap();
        let health = Arc::new(Health::default());
        let (first, _a) = channel(48000, 48000, 30, health.clone()).unwrap();
        let (second, _b) = channel(48000, 48000, 30, health.clone()).unwrap();
        let mut capture = Capture::new(VocalChain::new(48000, config).unwrap(), -18.0, vec![(first, 0.0), (second, -6.0)], health.clone()).unwrap();
        capture.push_mono(&input);
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
}
