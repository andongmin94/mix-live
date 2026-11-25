//! Independent stereo feedback echo. Dry audio is not delayed.
//! The one-second ring is allocated before processing. Live time changes move its
//! fractional read position over 10 ms while preserving the feedback history.
use super::{DspError, ParameterRamp, VocalParameter};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EchoConfig {
    /// Linear wet/dry blend, independent of EQ and room reverb.
    pub mix: f32,
    /// Delay between repeats in milliseconds.
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
    delay: ParameterRamp,
    mix: ParameterRamp,
    feedback: ParameterRamp,
}

impl Echo {
    pub(super) fn new(rate: u32, config: EchoConfig) -> Self {
        let frames = (f64::from(config.time_ms) * f64::from(rate) / 1000.0).round() as usize;
        Self { history: vec![[0.0; 2]; rate as usize], cursor: 0,
            delay: ParameterRamp::new(rate, frames as f64),
            mix: ParameterRamp::new(rate, f64::from(config.mix)),
            feedback: ParameterRamp::new(rate, f64::from(config.feedback)) }
    }

    pub(super) fn set_mix(&mut self, value: f32) { self.mix.set(f64::from(value)); }
    pub(super) fn set_feedback(&mut self, value: f32) { self.feedback.set(f64::from(value)); }
    pub(super) fn set_time_frames(&mut self, frames: usize) { self.delay.set(frames as f64); }

    pub(super) fn run(&mut self, dry: [f64; 2]) -> [f64; 2] {
        let delay = self.delay.next_value();
        let whole = delay.floor() as usize;
        let fraction = delay - whole as f64;
        let newer = (self.cursor + self.history.len() - whole) % self.history.len();
        let wet = if fraction == 0.0 { self.history[newer] } else {
            let older = (newer + self.history.len() - 1) % self.history.len();
            std::array::from_fn(|channel| self.history[newer][channel] * (1.0 - fraction)
                + self.history[older][channel] * fraction)
        };
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
        self.delay.reset();
        self.mix.reset();
        self.feedback.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_echo_time_retains_stereo_history_at_endpoints_and_during_retarget() {
        for rate in [8000, 44100, 192000] {
            let maximum = rate as usize;
            let minimum = (f64::from(rate) * 0.010).round() as usize;
            let duration = rate as usize / 100;
            let mut echo = Echo::new(rate, EchoConfig { mix: 1.0, time_ms: 10.0, feedback: 0.0 });
            let mut frame = 0;
            while frame < maximum + 17 {
                let output = echo.run([frame as f64, -(frame as f64) * 0.5]);
                let delayed = frame.saturating_sub(minimum) as f64;
                assert_eq!(output, [delayed, -delayed * 0.5]);
                frame += 1;
            }
            let mut start = minimum as f64;
            // The second edit is interrupted before its ten-millisecond ramp ends.
            for (target, count) in [(maximum, duration), (minimum, duration / 3), (maximum / 2, duration)] {
                echo.set_time_frames(target);
                let mut offset = start;
                for step in 1..=count {
                    offset = start + (target as f64 - start) * step as f64 / duration as f64;
                    let expected = frame as f64 - offset;
                    let output = echo.run([frame as f64, -(frame as f64) * 0.5]);
                    assert!((output[0] - expected).abs() < 1e-7, "rate {rate}, target {target}, step {step}");
                    assert!((output[1] + expected * 0.5).abs() < 1e-7);
                    frame += 1;
                }
                start = offset;
                if count == duration {
                    assert_eq!(echo.delay.current, target as f64);
                    assert_eq!(echo.delay.remaining, 0);
                }
            }
            echo.set_time_frames(maximum);
            echo.reset();
            for frame in 0..=maximum {
                let output = echo.run(if frame == 0 { [1.0, -0.5] } else { [0.0; 2] });
                assert_eq!(output, if frame == maximum { [1.0, -0.5] } else { [0.0; 2] });
            }
        }
    }
}
