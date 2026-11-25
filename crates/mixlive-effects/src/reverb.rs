//! Streaming stereo room effect. Original implementation of an eight-line feedback delay
//! network with an orthogonal Walsh-Hadamard feedback matrix, per-line decay and damping.
//! Design reference: J. O. Smith, Physical Audio Signal Processing, FDN Reverberation:
//! https://www.dsprelated.com/freebooks/pasp/FDN_Reverberation.html
//! Dry audio is never delayed; pre-delay and room reflections affect only the wet signal.
use super::{DspError, Filter, ParameterRamp, VocalParameter};
use biquad::{Type, Q_BUTTERWORTH_F64};

const LINES: usize = 8;
const DELAY_MS: [f64; LINES] = [19.3, 23.7, 29.1, 31.7, 37.9, 43.1, 47.3, 53.9];
const NORM: f64 = std::f64::consts::FRAC_1_SQRT_2 * 0.5;
const IN: [f64; LINES] = [1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 1.0, -1.0];
const RIGHT: [f64; LINES] = [1.0, 1.0, -1.0, -1.0, 1.0, 1.0, -1.0, -1.0];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReverbConfig {
    /// Linear wet/dry blend, independently of the vocal preset/strength.
    pub mix: f32,
    /// Nominal low-frequency RT60 (seconds); damping shortens high-frequency decay.
    pub decay_secs: f32,
    /// Extra wet-only pre-delay, milliseconds.
    pub predelay_ms: f32,
}

impl Default for ReverbConfig {
    fn default() -> Self { Self { mix: 0.0, decay_secs: 1.2, predelay_ms: 15.0 } }
}

impl ReverbConfig {
    pub fn validate(self) -> Result<(), DspError> {
        VocalParameter::ReverbMix.validate(self.mix)?;
        VocalParameter::ReverbDecay.validate(self.decay_secs)?;
        if !self.predelay_ms.is_finite() || !(0.0..=80.0).contains(&self.predelay_ms) { return Err(DspError::ReverbPredelay); }
        Ok(())
    }
}

struct Delay {
    samples: Vec<f64>,
    cursor: usize,
}

impl Delay {
    fn new(frames: usize) -> Self { Self { samples: vec![0.0; frames], cursor: 0 } }
    fn read(&self) -> f64 { self.samples[self.cursor] }
    fn push(&mut self, value: f64) -> f64 {
        if self.samples.is_empty() { return value; }
        let previous = std::mem::replace(&mut self.samples[self.cursor], value);
        self.cursor += 1;
        if self.cursor == self.samples.len() { self.cursor = 0; }
        previous
    }
    fn reset(&mut self) { self.samples.fill(0.0); self.cursor = 0; }
}

struct Line {
    delay: Delay,
    damping: Filter,
    feedback_gain: ParameterRamp,
}

pub(super) struct Reverb {
    lines: Vec<Line>,
    predelay: Delay,
    mix: ParameterRamp,
    rate: u32,
}

impl Reverb {
    /// Config and rate have already been validated by VocalChain::new.
    pub(super) fn new(rate: u32, config: ReverbConfig) -> Result<Self, DspError> {
        let frames = |ms: f64| (ms * f64::from(rate) / 1000.0).round() as usize;
        let lines = DELAY_MS.into_iter().map(|ms| {
            let length = frames(ms);
            // A loop traversed for decay_secs seconds attenuates by 60 dB at DC.
            let feedback_gain = Self::feedback_gain(length, rate, config.decay_secs);
            Ok(Line {
                delay: Delay::new(length),
                damping: Filter::new(Type::SinglePoleLowPass, 4500.0_f64.min(f64::from(rate) * 0.4), Q_BUTTERWORTH_F64, f64::from(rate))?,
                feedback_gain: ParameterRamp::new(rate, feedback_gain),
            })
        }).collect::<Result<Vec<_>, DspError>>()?;
        Ok(Self { lines, predelay: Delay::new(frames(f64::from(config.predelay_ms))),
            mix: ParameterRamp::new(rate, f64::from(config.mix)), rate })
    }

    fn feedback_gain(length: usize, rate: u32, decay: f32) -> f64 {
        10.0_f64.powf(-3.0 * length as f64 / (f64::from(rate) * f64::from(decay)))
    }

    pub(super) fn set_mix(&mut self, mix: f32) { self.mix.set(f64::from(mix)); }

    pub(super) fn set_decay(&mut self, decay: f32) {
        for line in &mut self.lines {
            line.feedback_gain.set(Self::feedback_gain(line.delay.samples.len(), self.rate, decay));
        }
    }

    pub(super) fn run(&mut self, dry: f64) -> [f64; 2] {
        let input = self.predelay.push(dry) * NORM;
        let mut feedback = [0.0; LINES];
        let mut wet = [0.0; 2];
        for (index, line) in self.lines.iter_mut().enumerate() {
            let tap = line.delay.read();
            wet[0] += tap;
            wet[1] += tap * RIGHT[index];
            feedback[index] = line.damping.run(tap) * line.feedback_gain.next_value();
        }
        // Three butterfly stages implement a unitary 8x8 transform after scaling.
        // Each loop has gain < 1 and a non-amplifying low-pass damping filter.
        for stride in [1, 2, 4] {
            for group in feedback.chunks_exact_mut(stride * 2) {
                let (a, b) = group.split_at_mut(stride);
                for (a, b) in a.iter_mut().zip(b) {
                    let (x, y) = (*a, *b);
                    *a = x + y;
                    *b = x - y;
                }
            }
        }
        for (index, line) in self.lines.iter_mut().enumerate() {
            let next = input * IN[index] + NORM * feedback[index];
            // Avoid subnormal states after a long silent tail.
            line.delay.push(if next.abs() < 1e-100 { 0.0 } else { next });
        }
        let mix = self.mix.next_value();
        // Keep the room history moving during bypass so live edits never allocate
        // or resume a frozen tail. Zero blend remains an exact dry path.
        if mix == 0.0 { [dry; 2] }
        else { wet.map(|value| dry * (1.0 - mix) + mix * NORM * value) }
    }

    pub(super) fn reset(&mut self) {
        self.predelay.reset();
        self.mix.reset();
        for line in &mut self.lines { line.delay.reset(); line.damping.reset(); line.feedback_gain.reset(); }
    }
}
