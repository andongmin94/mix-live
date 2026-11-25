//! Opt-in mono pitch correction, using the same normalized autocorrelation as the tuner.
//! A fixed-hop phase vocoder preserves the input clock and adds exactly one window of
//! nominal delay. The unvoiced path is the equally delayed dry signal, not a fault fallback.
//! FFT plans, scratch, analysis/synthesis and overlap-add rings are allocated before start.
use crate::{config::PitchConfig, pitch::{Estimator, analysis_size}};
use rustfft::{Fft, FftPlanner, num_complex::Complex64};
use std::{f64::consts::{PI, TAU}, sync::Arc};

pub(crate) struct PitchCorrector {
    config: PitchConfig,
    rate: u32,
    detector: Estimator,
    fft: Arc<dyn Fft<f64>>,
    inverse: Arc<dyn Fft<f64>>,
    spectrum: Vec<Complex64>,
    synthesis: Vec<Complex64>,
    scratch: Vec<Complex64>,
    history: Vec<f64>,
    frame: Vec<f64>,
    window: Vec<f64>,
    delta: Vec<f64>,
    analysis_phase: Vec<f64>,
    synthesis_phase: Vec<f64>,
    cursor: usize,
    filled: usize,
    hop: usize,
    since_hop: usize,
    phase_valid: bool,
    smoothed_shift: f64,
    retune_alpha: f64,
}

impl PitchCorrector {
    pub(crate) fn new(rate: u32, config: PitchConfig) -> Result<Option<Self>, String> {
        config.validate()?;
        if !(8000..=192_000).contains(&rate) { return Err("unsupported pitch correction sample rate".into()); }
        if config.strength == 0.0 { return Ok(None); }
        let size = analysis_size(rate as usize);
        let hop = size / 4;
        let mut planner = FftPlanner::<f64>::new();
        let fft = planner.plan_fft_forward(size);
        let inverse = planner.plan_fft_inverse(size);
        let scratch_len = fft.get_inplace_scratch_len().max(inverse.get_inplace_scratch_len());
        let window = (0..size).map(|i| (0.5 - 0.5 * (TAU * i as f64 / size as f64).cos()).sqrt()).collect();
        Ok(Some(Self {
            config, rate, detector: Estimator::new(rate as usize, size), fft, inverse,
            spectrum: vec![Complex64::default(); size], synthesis: vec![Complex64::default(); size],
            scratch: vec![Complex64::default(); scratch_len],
            history: vec![0.0; size], frame: vec![0.0; size], window, delta: vec![0.0; size],
            analysis_phase: vec![0.0; size / 2], synthesis_phase: vec![0.0; size / 2],
            cursor: 0, filled: 0, hop, since_hop: 0, phase_valid: false, smoothed_shift: 0.0,
            retune_alpha: (-(hop as f64) / (f64::from(rate) * f64::from(config.retune_ms) * 0.001)).exp(),
        }))
    }

    pub(crate) fn latency(&self) -> usize { self.history.len() }

    /// Apply an already validated, enabled setting at a processing boundary.
    /// Keep the analysis, synthesis and overlap histories; retune starts from
    /// the current shift instead of creating a new analysis-window gap.
    pub(crate) fn set_config(&mut self, config: PitchConfig) {
        self.config = config;
        self.retune_alpha = (-(self.hop as f64)
            / (f64::from(self.rate) * f64::from(config.retune_ms) * 0.001)).exp();
    }

    /// Arbitrary callback block lengths, exactly one output for each input sample.
    pub(crate) fn process(&mut self, input: &[f32], output: &mut [f32]) -> bool {
        if input.len() != output.len() || input.iter().any(|sample| !sample.is_finite()) { return false; }
        let size = self.history.len();
        for (&sample, out) in input.iter().zip(output) {
            // The history slot contains sample t-size. A completed frame is added
            // starting at the NEXT unread output slot, so latency never changes.
            *out = (self.history[self.cursor] + self.delta[self.cursor]) as f32;
            if !out.is_finite() { return false; }
            self.delta[self.cursor] = 0.0;
            self.history[self.cursor] = f64::from(sample);
            self.cursor = (self.cursor + 1) % size;
            self.filled = (self.filled + 1).min(size);
            self.since_hop += 1;
            if self.since_hop == self.hop {
                self.since_hop = 0;
                if self.filled == size { self.render_frame(); }
            }
        }
        true
    }

    fn render_frame(&mut self) {
        let size = self.history.len();
        let split = size - self.cursor;
        self.frame[..split].copy_from_slice(&self.history[self.cursor..]);
        self.frame[split..].copy_from_slice(&self.history[..self.cursor]);
        let Some((hz, _clarity)) = self.detector.estimate(&self.frame) else {
            self.phase_valid = false;
            return;
        };
        let midi = 69.0 + 12.0 * (hz / 440.0).log2();
        let target = f64::from(self.config.scale.nearest_note(self.config.key, midi));
        let desired = (target - midi) * f64::from(self.config.strength);
        // A tuned note within one cent needs no resynthesis.
        if desired.abs() <= 0.01 && !self.phase_valid { return; }
        self.smoothed_shift = if self.phase_valid {
            self.retune_alpha * self.smoothed_shift + (1.0 - self.retune_alpha) * desired
        } else { desired };
        let ratio = 2.0_f64.powf(self.smoothed_shift / 12.0);
        let phase_step = TAU * self.hop as f64 / size as f64;

        for ((bin, &sample), &window) in self.spectrum.iter_mut().zip(&self.frame).zip(&self.window) {
            *bin = Complex64::new(sample * window, 0.0);
        }
        self.fft.process_with_scratch(&mut self.spectrum, &mut self.scratch);
        self.synthesis.fill(Complex64::default());
        self.synthesis[0] = Complex64::new(self.spectrum[0].re, 0.0);
        let half = size / 2;
        for k in 1..half {
            let input = self.spectrum[k];
            let phase = input.arg();
            let expected = phase_step * k as f64;
            // Instantaneous frequency from the phase deviation across input hops.
            // Maintain synthesis phase by input bin, including when its destination moves.
            self.synthesis_phase[k] = if self.phase_valid {
                wrap_phase(self.synthesis_phase[k] + (expected + wrap_phase(phase - self.analysis_phase[k] - expected)) * ratio)
            } else { phase };
            self.analysis_phase[k] = phase;
            let destination = (k as f64 * ratio).round() as usize;
            if destination > 0 && destination < half {
                self.synthesis[destination] += Complex64::from_polar(input.norm(), self.synthesis_phase[k]);
            }
        }
        self.phase_valid = true;
        for k in 1..half { self.synthesis[size - k] = self.synthesis[k].conj(); }
        self.inverse.process_with_scratch(&mut self.synthesis, &mut self.scratch);
        // sqrt-Hann analysis/synthesis, four overlaps: sum Hann = 2.
        // Accumulate only the difference from the aligned dry frame. When a frame
        // is unvoiced no difference is added, and previous frames fade naturally.
        for (i, ((bin, &window), &dry)) in self.synthesis.iter().zip(&self.window).zip(&self.frame).enumerate() {
            self.delta[(self.cursor + i) % size] += 0.5 * window * (bin.re / size as f64 - dry * window);
        }
    }
}

fn wrap_phase(phase: f64) -> f64 { (phase + PI).rem_euclid(TAU) - PI }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PitchScale;

    fn render(rate: u32, input: &[f32], config: PitchConfig, block: usize) -> (Vec<f32>, usize) {
        let mut correction = PitchCorrector::new(rate, config).unwrap().unwrap();
        let latency = correction.latency();
        let mut output = vec![0.0; input.len()];
        for (part, result) in input.chunks(block).zip(output.chunks_mut(block)) {
            assert!(correction.process(part, result));
        }
        (output, latency)
    }

    #[test]
    fn correction_moves_frequency_not_duration_and_respects_strength_and_scale() {
        for rate in [44_100, 48_000, 96_000] {
            for (hz, target, strength, scale) in [
                (445.0, 440.0, 1.0, PitchScale::Chromatic),
                (430.0, 440.0, 1.0, PitchScale::Chromatic),
                (113.0, 110.0, 1.0, PitchScale::Chromatic),
                (278.8, 293.664_767_917, 1.0, PitchScale::Major),
                (445.0, 440.0, 0.5, PitchScale::Chromatic),
            ] {
                let input: Vec<_> = (0..rate).map(|i| (0.2 * (TAU * hz * f64::from(i) / f64::from(rate)).sin()) as f32).collect();
                let config = PitchConfig { strength, scale, ..PitchConfig::default() };
                let (output, latency) = render(rate, &input, config, 137);
                assert_eq!(output.len(), input.len());
                assert!(output[..latency].iter().all(|&x| x == 0.0));
                assert!(output.iter().all(|x| x.is_finite()));
                let tail: Vec<_> = output[output.len() - latency..].iter().copied().map(f64::from).collect();
                let (measured, clarity) = Estimator::new(rate as usize, latency).estimate(&tail).expect("shifted tone");
                let expected = hz * (target / hz).powf(f64::from(strength));
                let error = 1200.0 * (measured / expected).log2();
                assert!(error.abs() < 8.0, "rate {rate}, {hz} -> {expected}, measured {measured}: {error} cents");
                let rms = (tail.iter().map(|x| x * x).sum::<f64>() / tail.len() as f64).sqrt();
                assert!((0.07..0.23).contains(&rms));
                assert!(clarity >= 0.8);
            }
        }
    }

    #[test]
    fn disabled_and_unvoiced_paths_have_explicit_latency_and_preserve_samples() {
        assert!(PitchCorrector::new(48000, PitchConfig::default()).unwrap().is_none());
        for invalid in [f32::NAN, -0.1, 1.1] {
            assert!(PitchCorrector::new(48000, PitchConfig { strength: invalid, ..PitchConfig::default() }).is_err());
        }
        assert!(PitchCorrector::new(48000, PitchConfig { retune_ms: 201.0, ..PitchConfig::default() }).is_err());
        let mut input = vec![0.0; 8192];
        input[137] = 0.625;
        input[5120] = -0.25;
        let config = PitchConfig { strength: 1.0, ..PitchConfig::default() };
        let (output, latency) = render(48000, &input, config, 256);
        assert_eq!(latency, 2048);
        assert!(output[..latency].iter().all(|&x| x == 0.0));
        assert_eq!(&output[latency..], &input[..input.len() - latency]);
    }

    #[test]
    fn correction_is_independent_of_callback_block_sizes() {
        let rate = 48000;
        let input: Vec<_> = (0..12000).map(|i| (0.2 * (TAU * 445.0 * f64::from(i) / f64::from(rate)).sin()) as f32).collect();
        let config = PitchConfig { strength: 0.8, ..PitchConfig::default() };
        let (reference, _) = render(rate, &input, config, input.len());
        for block in [1, 17, 256, 1024] {
            assert_eq!(render(rate, &input, config, block).0, reference);
        }
    }

    #[test]
    fn live_settings_retarget_without_resetting_history_or_block_alignment() {
        const RATE: u32 = 48000;
        let hz = 278.8;
        let configs = [
            PitchConfig { key: 0, scale: PitchScale::Major, strength: 1.0, retune_ms: 20.0 },
            PitchConfig { key: 2, scale: PitchScale::Major, strength: 0.75, retune_ms: 5.0 },
            PitchConfig { key: 0, scale: PitchScale::Chromatic, strength: 0.5, retune_ms: 200.0 },
        ];
        let input: Vec<_> = (0..RATE as usize * configs.len())
            .map(|i| (0.2 * (TAU * hz * i as f64 / f64::from(RATE)).sin()) as f32).collect();
        let render_changes = |block: usize| {
            let mut corrector = PitchCorrector::new(RATE, configs[0]).unwrap().unwrap();
            let mut output = vec![0.0; input.len()];
            for ((input, output), config) in input.chunks(RATE as usize)
                .zip(output.chunks_mut(RATE as usize)).zip(configs) {
                corrector.set_config(config);
                for (part, result) in input.chunks(block).zip(output.chunks_mut(block)) {
                    assert!(corrector.process(part, result));
                }
            }
            (output, corrector.latency())
        };
        let (output, latency) = render_changes(137);
        assert!(output.iter().all(|sample| sample.is_finite() && sample.abs() < 1.0));
        for (index, config) in configs.into_iter().enumerate() {
            let end = (index + 1) * RATE as usize;
            let tail: Vec<_> = output[end - latency..end].iter().copied().map(f64::from).collect();
            let measured = Estimator::new(RATE as usize, latency).estimate(&tail).unwrap().0;
            let note = 69.0 + 12.0 * (hz / 440.0).log2();
            let target = config.scale.nearest_note(config.key, note);
            let target_hz = 440.0 * 2.0_f64.powf((f64::from(target) - 69.0) / 12.0);
            let expected = hz * (target_hz / hz).powf(f64::from(config.strength));
            assert!((1200.0 * (measured / expected).log2()).abs() < 8.0, "{config:?}: {measured} != {expected}");
            if index > 0 {
                let boundary = index * RATE as usize;
                let energy = output[boundary..boundary + latency].iter().map(|sample| sample * sample).sum::<f32>();
                assert!(energy / latency as f32 > 0.005, "a setting change restarted the analysis window");
            }
        }
        for block in [1, 17, 256] { assert_eq!(render_changes(block).0, output); }

        // A setting change while an unvoiced impulse is in flight must not erase
        // the delayed sample or move it to a different output frame.
        let mut corrector = PitchCorrector::new(RATE, configs[0]).unwrap().unwrap();
        let mut impulse = vec![0.0; latency * 2];
        impulse[137] = 0.5;
        let mut delayed = vec![0.0; impulse.len()];
        assert!(corrector.process(&impulse[..1000], &mut delayed[..1000]));
        corrector.set_config(configs[2]);
        assert!(corrector.process(&impulse[1000..], &mut delayed[1000..]));
        assert!(delayed[..latency].iter().all(|&sample| sample == 0.0));
        assert_eq!(&delayed[latency..], &impulse[..latency]);
    }
}
