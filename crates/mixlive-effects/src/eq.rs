//! Live EQ uses biquad's DF1 form, which retains input/output history during retuning.
//! Coefficients are prepared only for a changed target and interpolated for 10 ms.
use biquad::{Biquad, Coefficients, DirectForm1, Type, Q_BUTTERWORTH_F64};
use super::{DspError, EqConfig, Preset, VocalConfig, VocalParameter};

#[derive(Clone, Copy)]
pub(super) struct EqTargets {
    preset: Preset,
    coefficients: [Coefficients<f64>; 4],
}

pub(super) struct Equalizer {
    filters: [LiveFilter; 4],
    preset: Preset,
    rate: f64,
}

impl Equalizer {
    pub(super) fn new(rate: u32, config: VocalConfig) -> Result<Self, DspError> {
        let targets = Self::prepare(rate, config.preset, config.eq)?;
        Ok(Self {
            filters: targets.coefficients.map(|coefficients| LiveFilter::new(coefficients, rate)),
            preset: config.preset,
            rate: f64::from(rate),
        })
    }

    pub(super) fn prepare(rate: u32, preset: Preset, config: EqConfig) -> Result<EqTargets, DspError> {
        let coefficient = |parameter, value| target(preset, parameter, value, f64::from(rate)).map(|(_, c)| c);
        Ok(EqTargets { preset, coefficients: [
            coefficient(VocalParameter::EqHpf, config.hpf_hz)?,
            coefficient(VocalParameter::EqBody, config.body_db)?,
            coefficient(VocalParameter::EqPresence, config.presence_db)?,
            coefficient(VocalParameter::EqAir, config.air_db)?,
        ] })
    }

    pub(super) fn apply_targets(&mut self, targets: EqTargets) {
        for (filter, coefficients) in self.filters.iter_mut().zip(targets.coefficients) { filter.set(coefficients); }
        self.preset = targets.preset;
    }

    pub(super) fn set_parameter(&mut self, parameter: VocalParameter, value: f32) -> Result<(), DspError> {
        let (index, coefficients) = target(self.preset, parameter, value, self.rate)?;
        self.filters[index].set(coefficients);
        Ok(())
    }

    pub(super) fn run(&mut self, mut value: f64) -> f64 {
        for filter in &mut self.filters { value = filter.run(value); }
        value
    }

    pub(super) fn reset(&mut self) {
        for filter in &mut self.filters { filter.reset(); }
    }
}

fn target(preset: Preset, parameter: VocalParameter, value: f32, rate: f64) -> Result<(usize, Coefficients<f64>), DspError> {
    let (body, presence, air) = match preset {
        Preset::Warm => (1.5, 0.0, -1.0),
        Preset::Bright => (-1.5, 2.0, 1.5),
        Preset::Natural | Preset::Off => (0.0, 1.0, 0.0),
    };
    let value = f64::from(value);
    let (index, kind, hz, q) = match parameter {
        VocalParameter::EqHpf => (0, Type::HighPass, value, Q_BUTTERWORTH_F64),
        VocalParameter::EqBody => (1, Type::LowShelf(body + value), 180.0, Q_BUTTERWORTH_F64),
        VocalParameter::EqPresence => (2, Type::PeakingEQ(presence + value), 2800.0, 0.8),
        VocalParameter::EqAir => (3, Type::HighShelf(air + value), 7500.0, Q_BUTTERWORTH_F64),
        _ => unreachable!("only EQ parameters are passed to the equalizer"),
    };
    let coefficients = Coefficients::from_normalized_params(kind, 2.0 * hz / rate, q).map_err(|_| DspError::Filter)?;
    Ok((index, coefficients))
}

struct LiveFilter {
    filter: DirectForm1<f64>,
    current: Coefficients<f64>,
    start: Coefficients<f64>,
    target: Coefficients<f64>,
    remaining: u32,
    duration: u32,
    quiet_frames: u8,
}

impl LiveFilter {
    fn new(coefficients: Coefficients<f64>, rate: u32) -> Self {
        Self { filter: DirectForm1::new(coefficients), current: coefficients, start: coefficients,
            target: coefficients, remaining: 0, duration: (rate / 100).max(1), quiet_frames: 0 }
    }

    fn set(&mut self, target: Coefficients<f64>) {
        if [self.target.a1, self.target.a2, self.target.b0, self.target.b1, self.target.b2]
            == [target.a1, target.a2, target.b0, target.b1, target.b2] { return; }
        self.start = self.current;
        self.target = target;
        self.remaining = self.duration;
    }

    fn run(&mut self, input: f64) -> f64 {
        if self.remaining != 0 {
            self.remaining -= 1;
            self.current = if self.remaining == 0 { self.target } else {
                // A real normalized second-order denominator's stable region is convex;
                // interpolate the full coefficient vector between the valid RBJ endpoints.
                let progress = f64::from(self.duration - self.remaining) / f64::from(self.duration);
                let mix = |start, end| start + progress * (end - start);
                Coefficients { a1: mix(self.start.a1, self.target.a1), a2: mix(self.start.a2, self.target.a2),
                    b0: mix(self.start.b0, self.target.b0), b1: mix(self.start.b1, self.target.b1), b2: mix(self.start.b2, self.target.b2) }
            };
            self.filter.update_coefficients(self.current);
        }
        let output = self.filter.run(input);
        // DF1 stores two input/output frames privately. Flush only once both frames
        // are inaudible, before the recursive state reaches the subnormal range.
        if input.abs() < 1e-100 && output.abs() < 1e-100 {
            if self.quiet_frames < 2 {
                self.quiet_frames += 1;
                if self.quiet_frames == 2 { self.filter.reset_state(); }
            }
        } else { self.quiet_frames = 0; }
        output
    }

    fn reset(&mut self) {
        self.current = self.target;
        self.remaining = 0;
        self.filter.update_coefficients(self.target);
        self.filter.reset_state();
        self.quiet_frames = 0;
    }
}
