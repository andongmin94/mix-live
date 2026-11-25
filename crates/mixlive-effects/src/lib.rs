//! Streaming vocal chain: mono trim/EQ/dynamics/de-essing -> stereo room reverb -> stereo echo.
//! No automatic makeup gain, lookahead buffer, audio I/O or UI dependencies.
//! Filters use biquad's RBJ coefficient implementation; presets are unvalidated starting points,
//! not a claim of production sound quality. IIR phase/group delay is not zero.
use std::{fmt, str::FromStr};
use biquad::{Biquad, Coefficients, DirectForm2Transposed, Type, Q_BUTTERWORTH_F64};

mod reverb;
pub use reverb::ReverbConfig;
use reverb::Reverb;
mod echo;
pub use echo::EchoConfig;
use echo::Echo;
mod eq;
use eq::{Equalizer, EqTargets};

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

/// Settings applied together at a processing block boundary. Independent live
/// parameter targets and their current ramps remain unchanged.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VocalSettings {
    pub preset: Preset,
    pub reverb_predelay_ms: f32,
    pub echo_time_ms: f32,
}
impl VocalSettings {
    pub fn validate(self) -> Result<(), DspError> {
        if !(0.0..=80.0).contains(&self.reverb_predelay_ms) { return Err(DspError::ReverbPredelay); }
        if !(10.0..=1000.0).contains(&self.echo_time_ms) { return Err(DspError::EchoTime); }
        Ok(())
    }

    /// Replace only these three settings; validate the resulting config before use.
    pub fn apply_to(self, config: &mut VocalConfig) {
        config.preset = self.preset;
        config.reverb.predelay_ms = self.reverb_predelay_ms;
        config.echo.time_ms = self.echo_time_ms;
    }
}

/// Parameters that can change without changing processing topology or a delay length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum VocalParameter { Trim, Strength, Compressor, Deesser, ReverbMix, ReverbDecay, EchoMix, EchoFeedback, CompressorRatio, CompressorAttack, CompressorRelease, DeesserMaximum, EqHpf, EqBody, EqPresence, EqAir }

impl VocalParameter {
    pub const ALL: [Self; 16] = [Self::Trim, Self::Strength, Self::Compressor, Self::Deesser, Self::ReverbMix, Self::ReverbDecay, Self::EchoMix, Self::EchoFeedback,
        Self::CompressorRatio, Self::CompressorAttack, Self::CompressorRelease, Self::DeesserMaximum,
        Self::EqHpf, Self::EqBody, Self::EqPresence, Self::EqAir];

    pub fn name(self) -> &'static str {
        match self {
            Self::Trim => "trim", Self::Strength => "strength", Self::Compressor => "compressor",
            Self::Deesser => "deesser", Self::ReverbMix => "reverb", Self::ReverbDecay => "decay",
            Self::EchoMix => "echo", Self::EchoFeedback => "echo-feedback",
            Self::CompressorRatio => "ratio", Self::CompressorAttack => "attack", Self::CompressorRelease => "release", Self::DeesserMaximum => "deess-max",
            Self::EqHpf => "eq-hpf", Self::EqBody => "eq-body", Self::EqPresence => "eq-presence", Self::EqAir => "eq-air",
        }
    }

    pub fn range(self) -> std::ops::RangeInclusive<f32> {
        match self {
            Self::Trim => -36.0..=18.0,
            Self::Strength | Self::ReverbMix | Self::EchoMix => 0.0..=1.0,
            Self::EchoFeedback => 0.0..=0.85,
            Self::Compressor | Self::Deesser => -60.0..=0.0,
            Self::ReverbDecay => 0.2..=5.0,
            Self::CompressorRatio => 1.0..=8.0,
            Self::CompressorAttack => 1.0..=50.0,
            Self::CompressorRelease => 20.0..=500.0,
            Self::DeesserMaximum => 0.0..=12.0,
            Self::EqHpf => 40.0..=200.0,
            Self::EqBody | Self::EqPresence | Self::EqAir => -6.0..=6.0,
        }
    }

    pub fn available_for(self, preset: Preset) -> bool {
        preset != Preset::Off || matches!(self, Self::Trim | Self::ReverbMix | Self::ReverbDecay | Self::EchoMix | Self::EchoFeedback)
    }

    pub fn validate(self, value: f32) -> Result<(), DspError> {
        if self.range().contains(&value) { return Ok(()); }
        Err(match self {
            Self::Trim => DspError::InputGain,
            Self::Strength => DspError::Strength,
            Self::Compressor | Self::Deesser => DspError::Threshold,
            Self::ReverbMix => DspError::ReverbMix,
            Self::ReverbDecay => DspError::ReverbDecay,
            Self::EchoMix => DspError::EchoMix,
            Self::EchoFeedback => DspError::EchoFeedback,
            Self::CompressorRatio => DspError::CompressorRatio,
            Self::CompressorAttack => DspError::CompressorAttack,
            Self::CompressorRelease => DspError::CompressorRelease,
            Self::DeesserMaximum => DspError::DeesserMaximum,
            Self::EqHpf => DspError::EqCutoff,
            Self::EqBody | Self::EqPresence | Self::EqAir => DspError::EqGain,
        })
    }
}

/// EQ targets. The three gain controls are offsets to the selected preset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EqConfig {
    pub hpf_hz: f32,
    pub body_db: f32,
    pub presence_db: f32,
    pub air_db: f32,
}
impl Default for EqConfig {
    fn default() -> Self { Self { hpf_hz: 75.0, body_db: 0.0, presence_db: 0.0, air_db: 0.0 } }
}

/// Explicit dynamics values, independent of the chosen EQ preset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DynamicsConfig {
    pub ratio: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub deess_max_db: f32,
}
impl Default for DynamicsConfig {
    fn default() -> Self { Self { ratio: 2.5, attack_ms: 8.0, release_ms: 140.0, deess_max_db: 4.0 } }
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
    pub eq: EqConfig,
    pub dynamics: DynamicsConfig,
    pub reverb: ReverbConfig,
    pub echo: EchoConfig,
}
impl Default for VocalConfig {
    fn default() -> Self {
        Self { preset: Preset::Off, input_gain_db: 0.0, strength: 1.0, compressor_threshold_db: -24.0, deess_threshold_db: -30.0, eq: EqConfig::default(), dynamics: DynamicsConfig::default(), reverb: ReverbConfig::default(), echo: EchoConfig::default() }
    }
}
impl VocalConfig {
    pub fn settings(self) -> VocalSettings {
        VocalSettings { preset: self.preset, reverb_predelay_ms: self.reverb.predelay_ms,
            echo_time_ms: self.echo.time_ms }
    }

    pub fn parameter(self, parameter: VocalParameter) -> f32 {
        match parameter {
            VocalParameter::Trim => self.input_gain_db,
            VocalParameter::Strength => self.strength,
            VocalParameter::Compressor => self.compressor_threshold_db,
            VocalParameter::Deesser => self.deess_threshold_db,
            VocalParameter::ReverbMix => self.reverb.mix,
            VocalParameter::ReverbDecay => self.reverb.decay_secs,
            VocalParameter::EchoMix => self.echo.mix,
            VocalParameter::EchoFeedback => self.echo.feedback,
            VocalParameter::CompressorRatio => self.dynamics.ratio,
            VocalParameter::CompressorAttack => self.dynamics.attack_ms,
            VocalParameter::CompressorRelease => self.dynamics.release_ms,
            VocalParameter::DeesserMaximum => self.dynamics.deess_max_db,
            VocalParameter::EqHpf => self.eq.hpf_hz,
            VocalParameter::EqBody => self.eq.body_db,
            VocalParameter::EqPresence => self.eq.presence_db,
            VocalParameter::EqAir => self.eq.air_db,
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
            VocalParameter::EchoMix => self.echo.mix = value,
            VocalParameter::EchoFeedback => self.echo.feedback = value,
            VocalParameter::CompressorRatio => self.dynamics.ratio = value,
            VocalParameter::CompressorAttack => self.dynamics.attack_ms = value,
            VocalParameter::CompressorRelease => self.dynamics.release_ms = value,
            VocalParameter::DeesserMaximum => self.dynamics.deess_max_db = value,
            VocalParameter::EqHpf => self.eq.hpf_hz = value,
            VocalParameter::EqBody => self.eq.body_db = value,
            VocalParameter::EqPresence => self.eq.presence_db = value,
            VocalParameter::EqAir => self.eq.air_db = value,
        }
        Ok(())
    }

    pub fn validate(self) -> Result<(), DspError> {
        for parameter in VocalParameter::ALL { parameter.validate(self.parameter(parameter))?; }
        self.reverb.validate()?;
        self.echo.validate()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DspError { Preset, InputGain, Strength, Threshold, ReverbMix, ReverbDecay, ReverbPredelay, EchoMix, EchoFeedback, EchoTime, EqCutoff, EqGain, CompressorRatio, CompressorAttack, CompressorRelease, DeesserMaximum, SampleRate, Filter, BlockLength, NonFiniteAudio }
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
            Self::EchoMix => "echo-mix must be finite and in 0..=1",
            Self::EchoFeedback => "echo-feedback must be finite and in 0..=0.85",
            Self::EchoTime => "echo-time-ms must be finite and in 10..=1000",
            Self::EqCutoff => "eq-hpf-hz must be finite and in 40..=200",
            Self::EqGain => "EQ gain offsets must be finite and in -6..=6 dB",
            Self::CompressorRatio => "compressor-ratio must be finite and in 1..=8",
            Self::CompressorAttack => "compressor-attack-ms must be finite and in 1..=50",
            Self::CompressorRelease => "compressor-release-ms must be finite and in 20..=500",
            Self::DeesserMaximum => "deess-max-db must be finite and in 0..=12",
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
    fn new(rate: f64, threshold: f64, config: DynamicsConfig) -> Self {
        Self { power: 0.0, reduction: 0.0, detector: coefficient(10.0, rate),
            attack: coefficient(f64::from(config.attack_ms), rate), release: coefficient(f64::from(config.release_ms), rate),
            threshold, ratio: f64::from(config.ratio) }
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
        // Enforce the current, ramped maximum even while the release envelope is decaying.
        self.reduction = self.reduction.min(self.maximum);
        // Complementary residual: exactly the original when no attenuation is requested.
        sample + (gain(-self.reduction) - 1.0) * high
    }
    fn reset(&mut self) { self.low.reset(); self.power = 0.0; self.high_power = 0.0; self.reduction = 0.0; }
}

struct Processing {
    eq: Equalizer,
    compressor: Compressor,
    deesser: DeEsser,
}
impl Processing {
    fn new(sample_rate: u32, config: VocalConfig) -> Result<Self, DspError> {
        let rate = f64::from(sample_rate);
        let dynamics = config.dynamics;
        Ok(Self {
            eq: Equalizer::new(sample_rate, config)?,
            compressor: Compressor::new(rate, f64::from(config.compressor_threshold_db), dynamics),
            deesser: DeEsser::new(rate, f64::from(config.deess_threshold_db), f64::from(dynamics.deess_max_db))?,
        })
    }
    fn reset(&mut self) {
        self.eq.reset();
        self.compressor.reset(); self.deesser.reset();
    }
}

/// Prepare on the control thread for the current chain's sample rate and settings.
/// After one application, return this value to the control thread to retire any
/// replaced EQ/dynamics state. Delay buffers remain owned by the running chain.
pub struct PreparedVocalChange {
    settings: VocalSettings,
    eq_snapshot: EqConfig,
    eq_targets: Option<EqTargets>,
    replacement: Option<Option<Processing>>,
    predelay_frames: usize,
    echo_frames: usize,
}
impl PreparedVocalChange {
    pub fn new(rate: u32, current: VocalSettings, next_config: VocalConfig) -> Result<Self, DspError> {
        next_config.validate()?;
        let minimum = if next_config.preset == Preset::Off { 8000 } else { 32000 };
        if !(minimum..=192_000).contains(&rate) { return Err(DspError::SampleRate); }
        let enabled = next_config.preset != Preset::Off;
        let eq_targets = if enabled { Some(Equalizer::prepare(rate, next_config.preset, next_config.eq)?) } else { None };
        let replacement = if (current.preset != Preset::Off) == enabled { None }
            else if enabled { Some(Some(Processing::new(rate, next_config)?)) } else { Some(None) };
        let frames = |ms| (f64::from(ms) * f64::from(rate) / 1000.0).round() as usize;
        Ok(Self { settings: next_config.settings(), eq_snapshot: next_config.eq, eq_targets, replacement,
            predelay_frames: frames(next_config.reverb.predelay_ms), echo_frames: frames(next_config.echo.time_ms) })
    }
}

pub struct VocalChain {
    config: VocalConfig,
    rate: f64,
    input_gain: ParameterRamp,
    strength: ParameterRamp,
    compressor_threshold: ParameterRamp,
    deesser_threshold: ParameterRamp,
    compressor_ratio: ParameterRamp,
    compressor_attack: ParameterRamp,
    compressor_release: ParameterRamp,
    deess_maximum: ParameterRamp,
    processing: Option<Processing>,
    reverb: Reverb,
    echo: Echo,
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
            else { Some(Processing::new(sample_rate, config)?) };
        let reverb = Reverb::new(sample_rate, config.reverb)?;
        let echo = Echo::new(sample_rate, config.echo);
        let rate = f64::from(sample_rate);
        Ok(Self { config, rate,
            input_gain: ParameterRamp::new(sample_rate, gain(f64::from(config.input_gain_db))),
            strength: ParameterRamp::new(sample_rate, f64::from(config.strength)),
            compressor_threshold: ParameterRamp::new(sample_rate, f64::from(config.compressor_threshold_db)),
            deesser_threshold: ParameterRamp::new(sample_rate, f64::from(config.deess_threshold_db)),
            compressor_ratio: ParameterRamp::new(sample_rate, f64::from(config.dynamics.ratio)),
            compressor_attack: ParameterRamp::new(sample_rate, coefficient(f64::from(config.dynamics.attack_ms), rate)),
            compressor_release: ParameterRamp::new(sample_rate, coefficient(f64::from(config.dynamics.release_ms), rate)),
            deess_maximum: ParameterRamp::new(sample_rate, f64::from(config.dynamics.deess_max_db)),
            processing, reverb, echo })
    }
    /// Requested settings, not transient ramp values.
    pub fn config(&self) -> VocalConfig { self.config }

    /// Apply once on the processing thread at a block boundary, without allocating
    /// or freeing buffers. Active presets retain EQ/dynamics history; toggling off
    /// swaps only that processing stage. Wet delay read positions ramp for 10 ms.
    pub fn apply_settings(&mut self, change: &mut PreparedVocalChange) {
        let eq_targets = change.eq_targets.map(|prepared| {
            if self.config.eq == change.eq_snapshot { prepared } else {
                // Independent atomic EQ targets may have reached this worker after
                // the control-side snapshot. Reconcile before its unchanged-target
                // fast path could leave the prepared filters using stale offsets.
                Equalizer::prepare(self.rate as u32, change.settings.preset, self.config.eq)
                    .expect("validated live EQ targets at the current processing rate")
            }
        });
        if let Some(replacement) = &mut change.replacement { std::mem::swap(&mut self.processing, replacement); }
        if let (Some(processing), Some(targets)) = (&mut self.processing, eq_targets) { processing.eq.apply_targets(targets); }
        self.reverb.set_predelay_frames(change.predelay_frames);
        self.echo.set_time_frames(change.echo_frames);
        change.settings.apply_to(&mut self.config);
    }

    /// Apply a validated target on the audio-processing thread, at a block boundary.
    /// Only changed targets do work; settings changes use a prepared block-boundary plan.
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
            VocalParameter::EchoMix => self.echo.set_mix(value),
            VocalParameter::EchoFeedback => self.echo.set_feedback(value),
            VocalParameter::CompressorRatio => self.compressor_ratio.set(f64::from(value)),
            VocalParameter::CompressorAttack => self.compressor_attack.set(coefficient(f64::from(value), self.rate)),
            VocalParameter::CompressorRelease => self.compressor_release.set(coefficient(f64::from(value), self.rate)),
            VocalParameter::DeesserMaximum => self.deess_maximum.set(f64::from(value)),
            VocalParameter::EqHpf | VocalParameter::EqBody | VocalParameter::EqPresence | VocalParameter::EqAir => {
                if let Some(processing) = &mut self.processing { processing.eq.set_parameter(parameter, value)?; }
            }
        }
        Ok(())
    }

    pub fn reset(&mut self) {
        if let Some(processing) = &mut self.processing { processing.reset(); }
        self.reverb.reset();
        self.echo.reset();
        for ramp in [&mut self.input_gain, &mut self.strength, &mut self.compressor_threshold, &mut self.deesser_threshold,
            &mut self.compressor_ratio, &mut self.compressor_attack, &mut self.compressor_release, &mut self.deess_maximum] { ramp.reset(); }
    }

    /// Process mono input into stereo output without allocation, locks, logging or I/O.
    /// With reverb/echo at zero the processed mono signal is duplicated exactly.
    /// Both spatial effects are independent of EQ strength.
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
            let ratio = self.compressor_ratio.next_value();
            let attack = self.compressor_attack.next_value();
            let release = self.compressor_release.next_value();
            let maximum = self.deess_maximum.next_value();
            let value = if let Some(processing) = &mut self.processing {
                processing.compressor.threshold = compressor;
                processing.deesser.threshold = deesser;
                processing.compressor.ratio = ratio;
                processing.compressor.attack = attack;
                processing.compressor.release = release;
                processing.deesser.maximum = maximum;
                let mut wet = processing.eq.run(dry);
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
            let stereo = self.echo.run(self.reverb.run(value));
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
