use std::sync::{Arc, atomic::Ordering};
use mixlive_core::{ChannelLayout, Engine, Input, Mix, Scene, Send, Source};
use crate::bridge::{Fault, Health, OutputWriter};

pub const CAPTURE_BLOCK: usize = 256;

/// The input clock drives one source-to-mix render; each output has its own clock bridge.
pub struct Capture {
    engine: Engine,
    outputs: Vec<OutputWriter>,
    health: Arc<Health>,
}
impl Capture {
    pub fn new(gain_db: f32, outputs: Vec<(OutputWriter, f32)>, health: Arc<Health>) -> Result<Self, String> {
        let scene = Scene {
            sources: vec![Source { name: "Selected mic".into(), channels: ChannelLayout::Mono, gain_db, muted: false }],
            mixes: outputs.iter().enumerate().map(|(index, (_, gain_db))| Mix {
                name: format!("Output {index}"), gain_db: *gain_db, muted: false, excluded_sources: vec![],
            }).collect(),
            sends: (0..outputs.len()).map(|mix| Send { source: 0, mix, gain_db: 0.0, enabled: true }).collect(),
        };
        let engine = Engine::new(&scene, CAPTURE_BLOCK).map_err(|e| e.to_string())?;
        Ok(Self { engine, outputs: outputs.into_iter().map(|(writer, _)| writer).collect(), health })
    }
    pub fn push_mono(&mut self, samples: &[f32]) {
        if self.health.failed() { return; }
        for block in samples.chunks(CAPTURE_BLOCK) {
            if self.engine.render(block.len(), &[Input::Mono(block)]).is_err() {
                self.health.fail(Fault::Input); return;
            }
            let peak = block.iter().fold(0.0_f32, |p, x| p.max(x.abs()));
            self.health.input_peak.fetch_max(peak.to_bits(), Ordering::Relaxed);
            self.health.captured_frames.fetch_add(block.len() as u64, Ordering::Relaxed);
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

    #[test]
    fn one_capture_feeds_two_independent_mixes_without_dry_bypass() {
        let health = Arc::new(Health::default());
        let (first, mut monitor) = channel(48_000, 48_000, 10, health.clone()).unwrap();
        let (second, mut send) = channel(48_000, 48_000, 10, health.clone()).unwrap();
        let mut capture = Capture::new(-6.0, vec![(first, 0.0), (second, -6.0)], health.clone()).unwrap();
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
}
