//! Streaming mono vocal chain: trim -> HPF/tone -> RMS compression -> split-band de-essing.
//! No automatic makeup gain, lookahead buffer, audio I/O or UI dependencies.
//! Filters use biquad's RBJ coefficient implementation; presets are unvalidated starting points,
//! not a claim of production sound quality. IIR phase/group delay is not zero.
use std::{fmt, str::FromStr};
use biquad::{Biquad, Coefficients, DirectForm2Transposed, Type, Q_BUTTERWORTH_F64};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Preset {
    #[default]
    Off,
    Natural,
    Warm,
    Bright,
}
impl fmt::Display for Preset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self { Self::Off => "off", Self::Natural => "natural", Self::Warm => "warm", Self::Bright => "bright" })
    }
}
impl FromStr for Preset {
    type Err = DspError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "off" => Ok(Self::Off), "natural" => Ok(Self::Natural),
            "warm" => Ok(Self::Warm), "bright" => Ok(Self::Bright),
            _ => Err(DspError::Preset),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct VocalConfig {
    pub preset: Preset,
    /// Digital trim before the detectors. Cannot repair clipping at the interface input.
    pub input_gain_db: f32,
    /// Dry/processed blend; zero retains only trimmed input. Not loudness matching.
    pub strength: f32,
    pub compressor_threshold_db: f32,
    pub deess_threshold_db: f32,
}
impl Default for VocalConfig {
    fn default() -> Self {
        Self { preset: Preset::Off, input_gain_db: 0.0, strength: 1.0, compressor_threshold_db: -24.0, deess_threshold_db: -30.0 }
    }
}
impl VocalConfig {
    pub fn validate(self) -> Result<(), DspError> {
        if !self.input_gain_db.is_finite() || !(-36.0..=18.0).contains(&self.input_gain_db) { return Err(DspError::InputGain); }
        if !self.strength.is_finite() || !(0.0..=1.0).contains(&self.strength) { return Err(DspError::Strength); }
        if [self.compressor_threshold_db, self.deess_threshold_db].iter().any(|v| !v.is_finite() || !(-60.0..=0.0).contains(v)) {
            return Err(DspError::Threshold);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DspError { Preset, InputGain, Strength, Threshold, SampleRate, Filter, NonFiniteAudio }
impl fmt::Display for DspError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Preset => "vocal preset must be off, natural, warm or bright",
            Self::InputGain => "input-gain-db must be finite and in -36..=18",
            Self::Strength => "vocal-strength must be finite and in 0..=1",
            Self::Threshold => "vocal thresholds must be finite and in -60..=0 dBFS",
            Self::SampleRate => "vocal presets require 32000..=192000 Hz; off supports 8000..=192000 Hz",
            Self::Filter => "could not prepare vocal filter coefficients",
            Self::NonFiniteAudio => "non-finite audio in vocal processing; block silenced",
        })
    }
}
impl std::error::Error for DspError {}

/// Maximum values over this call, before the output mix's attenuation.
#[derive(Clone, Copy, Debug, Default)]
pub struct Meters {
    pub output_peak: f32,
    pub compression_db: f32,
    pub deessing_db: f32,
}

struct Filter(DirectForm2Transposed<f64>);
impl Filter {
    fn new(kind: Type<f64>, hz: f64, q: f64, rate: f64) -> Result<Self, DspError> {
        let coefficients = Coefficients::<f64>::from_normalized_params(kind, 2.0 * hz / rate, q).map_err(|_| DspError::Filter)?;
        Ok(Self(DirectForm2Transposed::new(coefficients)))
    }
    fn run(&mut self, value: f64) -> f64 {
        let output = self.0.run(value);
        // Flush inaudible state before it becomes a subnormal in long silent passages.
        if self.0.s1.abs() < 1e-100 { self.0.s1 = 0.0; }
        if self.0.s2.abs() < 1e-100 { self.0.s2 = 0.0; }
        output
    }
    fn reset(&mut self) { self.0.reset_state(); }
}

fn coefficient(ms: f64, rate: f64) -> f64 { (-1.0 / (ms * 0.001 * rate)).exp() }
fn gain(db: f64) -> f64 { 10.0_f64.powf(db / 20.0) }
fn smooth(current: &mut f64, target: f64, alpha: f64) {
    *current = target + alpha * (*current - target);
    if current.abs() < 1e-100 { *current = 0.0; }
}

/// Positive attenuation in dB; the quadratic knee meets both straight sections continuously.
fn reduction(level: f64, threshold: f64, ratio: f64, knee: f64) -> f64 {
    let over = level - threshold;
    let slope = 1.0 - 1.0 / ratio;
    if over <= -knee * 0.5 { 0.0 }
    else if over >= knee * 0.5 { over * slope }
    else { slope * (over + knee * 0.5).powi(2) / (2.0 * knee) }
}

struct Compressor {
    power: f64, reduction: f64,
    detector: f64, attack: f64, release: f64, threshold: f64, ratio: f64,
}
impl Compressor {
    fn new(rate: f64, threshold: f64, ratio: f64) -> Self {
        Self { power: 0.0, reduction: 0.0, detector: coefficient(10.0, rate), attack: coefficient(8.0, rate), release: coefficient(140.0, rate), threshold, ratio }
    }
    fn run(&mut self, sample: f64) -> f64 {
        smooth(&mut self.power, sample * sample, self.detector);
        let target = reduction(10.0 * self.power.max(1e-20).log10(), self.threshold, self.ratio, 6.0);
        let alpha = if target > self.reduction { self.attack } else { self.release };
        smooth(&mut self.reduction, target, alpha);
        sample * gain(-self.reduction)
    }
    fn reset(&mut self) { self.power = 0.0; self.reduction = 0.0; }
}

struct DeEsser {
    low: Filter,
    power: f64, high_power: f64, reduction: f64,
    detector: f64, attack: f64, release: f64, threshold: f64, maximum: f64,
}
impl DeEsser {
    fn new(rate: f64, threshold: f64, maximum: f64) -> Result<Self, DspError> {
        Ok(Self { low: Filter::new(Type::SinglePoleLowPass, 5500.0, Q_BUTTERWORTH_F64, rate)?,
            power: 0.0, high_power: 0.0, reduction: 0.0, detector: coefficient(3.0, rate),
            attack: coefficient(1.0, rate), release: coefficient(65.0, rate), threshold, maximum })
    }
    fn run(&mut self, sample: f64) -> f64 {
        let high = sample - self.low.run(sample);
        smooth(&mut self.power, sample * sample, self.detector);
        smooth(&mut self.high_power, high * high, self.detector);
        // Level AND high-band dominance, not merely every loud vowel. This is not speech recognition.
        let target = if self.high_power > self.power * 0.20 {
            reduction(10.0 * self.high_power.max(1e-20).log10(), self.threshold, 4.0, 3.0).min(self.maximum)
        } else { 0.0 };
        let alpha = if target > self.reduction { self.attack } else { self.release };
        smooth(&mut self.reduction, target, alpha);
        // Complementary residual: exactly the original when no attenuation is requested.
        sample + (gain(-self.reduction) - 1.0) * high
    }
    fn reset(&mut self) { self.low.reset(); self.power = 0.0; self.high_power = 0.0; self.reduction = 0.0; }
}

struct Processing {
    filters: [Filter; 4],
    compressor: Compressor,
    deesser: DeEsser,
}
impl Processing {
    fn new(rate: f64, config: VocalConfig) -> Result<Self, DspError> {
        let (highpass, body, presence, air, ratio, deess) = match config.preset {
            Preset::Warm => (65.0, 1.5, 0.0, -1.0, 2.5, 4.0),
            Preset::Bright => (85.0, -1.5, 2.0, 1.5, 3.0, 6.0),
            Preset::Natural | Preset::Off => (75.0, 0.0, 1.0, 0.0, 2.5, 4.0),
        };
        Ok(Self {
            filters: [
                Filter::new(Type::HighPass, highpass, Q_BUTTERWORTH_F64, rate)?,
                Filter::new(Type::LowShelf(body), 180.0, Q_BUTTERWORTH_F64, rate)?,
                Filter::new(Type::PeakingEQ(presence), 2800.0, 0.8, rate)?,
                Filter::new(Type::HighShelf(air), 7500.0, Q_BUTTERWORTH_F64, rate)?,
            ],
            compressor: Compressor::new(rate, f64::from(config.compressor_threshold_db), ratio),
            deesser: DeEsser::new(rate, f64::from(config.deess_threshold_db), deess)?,
        })
    }
    fn reset(&mut self) {
        for filter in &mut self.filters { filter.reset(); }
        self.compressor.reset(); self.deesser.reset();
    }
}

pub struct VocalChain {
    config: VocalConfig,
    input_gain: f64,
    processing: Option<Processing>,
}
impl VocalChain {
    /// Construct on the control thread, before opening audio streams.
    pub fn new(sample_rate: u32, config: VocalConfig) -> Result<Self, DspError> {
        config.validate()?;
        let minimum = if config.preset == Preset::Off { 8000 } else { 32000 };
        if !(minimum..=192_000).contains(&sample_rate) { return Err(DspError::SampleRate); }
        let processing = if config.preset == Preset::Off || config.strength == 0.0 { None }
            else { Some(Processing::new(f64::from(sample_rate), config)?) };
        Ok(Self { config, input_gain: gain(f64::from(config.input_gain_db)), processing })
    }
    pub fn config(&self) -> VocalConfig { self.config }
    pub fn reset(&mut self) { if let Some(processing) = &mut self.processing { processing.reset(); } }

    /// Process any block length without allocation, locks, logging, or I/O.
    /// Input/output remain mono; routing duplicates the processed source, not the DSP state.
    /// An invalid block is silenced in full and returns an error; callers must stop that route.
    pub fn process(&mut self, samples: &mut [f32]) -> Result<Meters, DspError> {
        if samples.iter().any(|sample| !sample.is_finite()) {
            samples.fill(0.0); self.reset(); return Err(DspError::NonFiniteAudio);
        }
        let mut meters = Meters::default();
        for sample in samples.iter_mut() {
            let dry = f64::from(*sample) * self.input_gain;
            let value = if let Some(processing) = &mut self.processing {
                let mut wet = dry;
                for filter in &mut processing.filters { wet = filter.run(wet); }
                wet = processing.compressor.run(wet);
                wet = processing.deesser.run(wet);
                meters.compression_db = meters.compression_db.max(processing.compressor.reduction as f32);
                meters.deessing_db = meters.deessing_db.max(processing.deesser.reduction as f32);
                dry + f64::from(self.config.strength) * (wet - dry)
            } else { dry };
            *sample = value as f32;
            meters.output_peak = meters.output_peak.max(sample.abs());
        }
        if samples.iter().any(|sample| !sample.is_finite()) {
            samples.fill(0.0); self.reset(); return Err(DspError::NonFiniteAudio);
        }
        Ok(meters)
    }
}

#[cfg(test)]
mod tests;
