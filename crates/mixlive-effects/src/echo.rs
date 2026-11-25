//! Independent stereo feedback echo. Dry audio is not delayed.
//! Buffer allocation and delay-length selection happen before processing.
use super::{DspError, ParameterRamp, VocalParameter};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EchoConfig {
    /// Linear wet/dry blend, independent of EQ and room reverb.
    pub mix: f32,
    /// Delay between repeats in milliseconds, fixed for the session.
    pub time_ms: f32,
    /// Linear feedback coefficient, bounded below unity.
    pub feedback: f32,
}

impl Default for EchoConfig {
    fn default() -> Self { Self { mix: 0.0, time_ms: 250.0, feedback: 0.35 } }
}

impl EchoConfig {
    pub fn validate(self) -> Result<(), DspError> {
        VocalParameter::EchoMix.validate(self.mix)?;
        VocalParameter::EchoFeedback.validate(self.feedback)?;
        if !(10.0..=1000.0).contains(&self.time_ms) { return Err(DspError::EchoTime); }
        Ok(())
    }
}

pub(super) struct Echo {
    history: Vec<[f64; 2]>,
    cursor: usize,
    mix: ParameterRamp,
    feedback: ParameterRamp,
}

impl Echo {
    pub(super) fn new(rate: u32, config: EchoConfig) -> Self {
        let frames = (f64::from(config.time_ms) * f64::from(rate) / 1000.0).round() as usize;
        Self { history: vec![[0.0; 2]; frames], cursor: 0,
            mix: ParameterRamp::new(rate, f64::from(config.mix)),
            feedback: ParameterRamp::new(rate, f64::from(config.feedback)) }
    }

    pub(super) fn set_mix(&mut self, value: f32) { self.mix.set(f64::from(value)); }
    pub(super) fn set_feedback(&mut self, value: f32) { self.feedback.set(f64::from(value)); }

    pub(super) fn run(&mut self, dry: [f64; 2]) -> [f64; 2] {
        let wet = self.history[self.cursor];
        let feedback = self.feedback.next_value();
        let next = std::array::from_fn(|channel| {
            let sample = dry[channel] + feedback * wet[channel];
            if sample.abs() < 1e-100 { 0.0 } else { sample }
        });
        self.history[self.cursor] = next;
        self.cursor += 1;
        if self.cursor == self.history.len() { self.cursor = 0; }
        let mix = self.mix.next_value();
        if mix == 0.0 { dry }
        else { std::array::from_fn(|channel| dry[channel] * (1.0 - mix) + wet[channel] * mix) }
    }

    pub(super) fn reset(&mut self) {
        self.history.fill([0.0; 2]);
        self.cursor = 0;
        self.mix.reset();
        self.feedback.reset();
    }
}
