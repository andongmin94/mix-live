//! Streaming vocal chain: mono trim/EQ/dynamics/de-essing -> stereo room reverb.
//! No automatic makeup gain, lookahead buffer, audio I/O or UI dependencies.
//! Filters use biquad's RBJ coefficient implementation; presets are unvalidated starting points,
//! not a claim of production sound quality. IIR phase/group delay is not zero.
use std::{fmt, str::FromStr};
use biquad::{Biquad, Coefficients, DirectForm2Transposed, Type, Q_BUTTERWORTH_F64};

mod reverb;
pub use reverb::ReverbConfig;
use reverb::Reverb;

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

/// Parameters that can change without changing processing topology or a delay length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum VocalParameter { Trim, Strength, Compressor, Deesser, ReverbMix, ReverbDecay }

impl VocalParameter {
    pub const ALL: [Self; 6] = [Self::Trim, Self::Strength, Self::Compressor, Self::Deesser, Self::ReverbMix, Self::ReverbDecay];

    pub fn name(self) -> &'static str {
        match self {
            Self::Trim => "trim", Self::Strength => "strength", Self::Compressor => "compressor",
            Self::Deesser => "deesser", Self::ReverbMix => "reverb", Self::ReverbDecay => "decay",
        }
    }

    pub fn range(self) -> std::ops::RangeInclusive<f32> {
        match self {
            Self::Trim => -36.0..=18.0,
            Self::Strength | Self::ReverbMix => 0.0..=1.0,
            Self::Compressor | Self::Deesser => -60.0..=0.0,
            Self::ReverbDecay => 0.2..=5.0,
        }
    }

    pub fn available_for(self, preset: Preset) -> bool {
        preset != Preset::Off || matches!(self, Self::Trim | Self::ReverbMix | Self::ReverbDecay)
    }

    pub fn validate(self, value: f32) -> Result<(), DspError> {
        if self.range().contains(&value) { return Ok(()); }
        Err(match self {
            Self::Trim => DspError::InputGain,
            Self::Strength => DspError::Strength,
            Self::Compressor | Self::Deesser => DspError::Threshold,
            Self::ReverbMix => DspError::ReverbMix,
            Self::ReverbDecay => DspError::ReverbDecay,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VocalConfig {
    pub preset: Preset,
    /// Digital trim before the detectors. Cannot repair clipping at the interface input.
    pub input_gain_db: f32,
    /// Dry/processed blend; zero retains only trimmed input. Not loudness matching.
    pub strength: f32,
    pub compressor_threshold_db: f32,
    pub deess_threshold_db: f32,
    pub reverb: ReverbConfig,
}
impl Default for VocalConfig {
    fn default() -> Self {
        Self { preset: Preset::Off, input_gain_db: 0.0, strength: 1.0, compressor_threshold_db: -24.0, deess_threshold_db: -30.0, reverb: ReverbConfig::default() }
    }
}
impl VocalConfig {
    pub fn parameter(self, parameter: VocalParameter) -> f32 {
        match parameter {
            VocalParameter::Trim => self.input_gain_db,
            VocalParameter::Strength => self.strength,
            VocalParameter::Compressor => self.compressor_threshold_db,
            VocalParameter::Deesser => self.deess_threshold_db,
            VocalParameter::ReverbMix => self.reverb.mix,
            VocalParameter::ReverbDecay => self.reverb.decay_secs,
        }
    }

    pub fn set_parameter(&mut self, parameter: VocalParameter, value: f32) -> Result<(), DspError> {
        parameter.validate(value)?;
        match parameter {
            VocalParameter::Trim => self.input_gain_db = value,
            VocalParameter::Strength => self.strength = value,
            VocalParameter::Compressor => self.compressor_threshold_db = value,
            VocalParameter::Deesser => self.deess_threshold_db = value,
            VocalParameter::ReverbMix => self.reverb.mix = value,
            VocalParameter::ReverbDecay => self.reverb.decay_secs = value,
        }
        Ok(())
    }

    pub fn validate(self) -> Result<(), DspError> {
        for parameter in VocalParameter::ALL { parameter.validate(self.parameter(parameter))?; }
        self.reverb.validate()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DspError { Preset, InputGain, Strength, Threshold, ReverbMix, ReverbDecay, ReverbPredelay, SampleRate, Filter, BlockLength, NonFiniteAudio }
impl fmt::Display for DspError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Preset => "vocal preset must be off, natural, warm or bright",
            Self::InputGain => "input-gain-db must be finite and in -36..=18",
            Self::Strength => "vocal-strength must be finite and in 0..=1",
            Self::Threshold => "vocal thresholds must be finite and in -60..=0 dBFS",
            Self::ReverbMix => "reverb-mix must be finite and in 0..=1",
            Self::ReverbDecay => "reverb-decay-secs must be finite and in 0.2..=5",
            Self::ReverbPredelay => "reverb-predelay-ms must be finite and in 0..=80",
            Self::SampleRate => "vocal presets require 32000..=192000 Hz; off supports 8000..=192000 Hz",
            Self::Filter => "could not prepare vocal filter coefficients",
            Self::BlockLength => "vocal input and stereo output lengths must match",
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

/// Ten-millisecond linear transition on the processing clock. Repeated requests
/// for the same target do not restart a transition; the final value is exact.
struct ParameterRamp {
    current: f64,
    target: f64,
    step: f64,
    remaining: u32,
    duration: u32,
}

impl ParameterRamp {
    fn new(rate: u32, value: f64) -> Self {
        Self { current: value, target: value, step: 0.0, remaining: 0, duration: (rate / 100).max(1) }
    }

    fn set(&mut self, value: f64) {
        if self.target != value {
            self.target = value;
            self.remaining = self.duration;
            self.step = (value - self.current) / f64::from(self.duration);
        }
    }

    fn next_value(&mut self) -> f64 {
        if self.remaining != 0 {
            self.remaining -= 1;
            if self.remaining == 0 { self.current = self.target; }
            else { self.current += self.step; }
        }
        self.current
    }

    fn reset(&mut self) {
        self.current = self.target;
        self.remaining = 0;
    }
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
    input_gain: ParameterRamp,
    strength: ParameterRamp,
    compressor_threshold: ParameterRamp,
    deesser_threshold: ParameterRamp,
    processing: Option<Processing>,
    reverb: Reverb,
}
impl VocalChain {
    /// Construct on the control thread, before opening audio streams.
    pub fn new(sample_rate: u32, config: VocalConfig) -> Result<Self, DspError> {
        config.validate()?;
        let minimum = if config.preset == Preset::Off { 8000 } else { 32000 };
        if !(minimum..=192_000).contains(&sample_rate) { return Err(DspError::SampleRate); }
        // Prepare the selected EQ/dynamics even at zero strength, and prepare
        // the fixed room network even at zero mix. Live edits never allocate.
        let processing = if config.preset == Preset::Off { None }
            else { Some(Processing::new(f64::from(sample_rate), config)?) };
        let reverb = Reverb::new(sample_rate, config.reverb)?;
        Ok(Self { config,
            input_gain: ParameterRamp::new(sample_rate, gain(f64::from(config.input_gain_db))),
            strength: ParameterRamp::new(sample_rate, f64::from(config.strength)),
            compressor_threshold: ParameterRamp::new(sample_rate, f64::from(config.compressor_threshold_db)),
            deesser_threshold: ParameterRamp::new(sample_rate, f64::from(config.deess_threshold_db)),
            processing, reverb })
    }
    /// Requested settings, not transient ramp values.
    pub fn config(&self) -> VocalConfig { self.config }

    /// Apply a validated target on the audio-processing thread, at a block boundary.
    /// Preset, sample rate and pre-delay remain fixed. Only changed targets do work.
    pub fn set_parameter(&mut self, parameter: VocalParameter, value: f32) -> Result<(), DspError> {
        if self.config.parameter(parameter) == value { return Ok(()); }
        self.config.set_parameter(parameter, value)?;
        match parameter {
            VocalParameter::Trim => self.input_gain.set(gain(f64::from(value))),
            VocalParameter::Strength => self.strength.set(f64::from(value)),
            VocalParameter::Compressor => self.compressor_threshold.set(f64::from(value)),
            VocalParameter::Deesser => self.deesser_threshold.set(f64::from(value)),
            VocalParameter::ReverbMix => self.reverb.set_mix(value),
            VocalParameter::ReverbDecay => self.reverb.set_decay(value),
        }
        Ok(())
    }

    pub fn reset(&mut self) {
        if let Some(processing) = &mut self.processing { processing.reset(); }
        self.reverb.reset();
        for ramp in [&mut self.input_gain, &mut self.strength, &mut self.compressor_threshold, &mut self.deesser_threshold] { ramp.reset(); }
    }

    /// Process mono input into stereo output without allocation, locks, logging or I/O.
    /// Zero reverb duplicates the processed mono signal exactly. Reverb is independent of EQ strength.
    /// An invalid block silences both outputs and resets state; callers must stop that route.
    pub fn process(&mut self, samples: &[f32], left: &mut [f32], right: &mut [f32]) -> Result<Meters, DspError> {
        if left.len() != samples.len() || right.len() != samples.len() {
            left.fill(0.0); right.fill(0.0); self.reset(); return Err(DspError::BlockLength);
        }
        if samples.iter().any(|sample| !sample.is_finite()) {
            left.fill(0.0); right.fill(0.0); self.reset(); return Err(DspError::NonFiniteAudio);
        }
        let mut meters = Meters::default();
        for ((&sample, left), right) in samples.iter().zip(left.iter_mut()).zip(right.iter_mut()) {
            let dry = f64::from(sample) * self.input_gain.next_value();
            let strength = self.strength.next_value();
            let compressor = self.compressor_threshold.next_value();
            let deesser = self.deesser_threshold.next_value();
            let value = if let Some(processing) = &mut self.processing {
                processing.compressor.threshold = compressor;
                processing.deesser.threshold = deesser;
                let mut wet = dry;
                for filter in &mut processing.filters { wet = filter.run(wet); }
                wet = processing.compressor.run(wet);
                wet = processing.deesser.run(wet);
                if strength == 0.0 {
                    dry
                } else {
                    meters.compression_db = meters.compression_db.max(processing.compressor.reduction as f32);
                    meters.deessing_db = meters.deessing_db.max(processing.deesser.reduction as f32);
                    dry + strength * (wet - dry)
                }
            } else { dry };
            let stereo = self.reverb.run(value);
            *left = stereo[0] as f32;
            *right = stereo[1] as f32;
            meters.output_peak = meters.output_peak.max(left.abs()).max(right.abs());
        }
        if left.iter().chain(right.iter()).any(|sample| !sample.is_finite()) {
            left.fill(0.0); right.fill(0.0); self.reset(); return Err(DspError::NonFiniteAudio);
        }
        Ok(meters)
    }
}

#[cfg(test)]
mod tests;
