//! Bounded SPSC clock transport. Output mixes share a bridge; AUX/apps use their input boundaries.
//! Rubato owns the anti-alias filter; we only steer its ratio from queue occupancy.
use std::sync::{Arc, atomic::{AtomicI32, AtomicU32, AtomicU64, AtomicU8, Ordering}};
use crate::config::MAX_MIXES;
use rtrb::{Consumer, Producer, RingBuffer};
use rubato::{Adjustable, Async, FixedAsync, Resampler, SincInterpolationParameters};
use rubato::audioadapter_buffers::direct::InterleavedSlice;

pub const QUANTUM: usize = 128;
/// At most three stereo mixes, packed in the endpoint's route order. Unused lanes stay zero.
pub type OutputFrame = [f32; MAX_MIXES * 2];
const MAX_CORRECTION: f64 = 0.001; // +/- 1000 ppm. This is not an audio device latency claim.
const CEILING: f32 = 0.98; // Digital sample ceiling, not true peak or hearing protection.
const RELEASE_SECONDS: f32 = 0.080;

/// Zero-lookahead, stereo-linked sample limiter. Reduce gain immediately to meet
/// the ceiling, then recover with a bounded linear slope on this endpoint clock.
/// The release slope reaches unity within 80 ms of uninterrupted quiet input.
struct StereoLimiter {
    gain: f32,
    release_step: f32,
}
impl StereoLimiter {
    fn new(rate: u32) -> Self {
        Self { gain: 1.0, release_step: 1.0 / (RELEASE_SECONDS * rate as f32) }
    }

    fn apply(&mut self, stereo: &mut [f32]) -> f32 {
        let peak = stereo[0].abs().max(stereo[1].abs());
        let required = if peak > CEILING { CEILING / peak } else { 1.0 };
        self.gain = (self.gain + self.release_step).min(required);
        if self.gain != 1.0 {
            // Same coefficient on both channels. The clamp only covers rounding at the ceiling.
            for sample in stereo { *sample = (*sample * self.gain).clamp(-CEILING, CEILING); }
        }
        self.gain
    }
}

/// Interval maxima on a physical playback endpoint's lane order.
#[derive(Clone, Copy, Debug, Default)]
pub struct LimiterReading {
    pub input_peak: f32,
    pub reduction_db: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Fault { Device = 1, Input = 2, Overflow = 3, Resampler = 4, Routing = 5, Application = 6 }

#[derive(Default)]
pub struct Health {
    fault: AtomicU8,
    pub captured_frames: AtomicU64,
    pub input_peak: AtomicU32,
    pub aux_peak: AtomicU32,
    pub voice_peak: AtomicU32,
}
impl Health {
    pub fn fail(&self, fault: Fault) {
        let _ = self.fault.compare_exchange(0, fault as u8, Ordering::Relaxed, Ordering::Relaxed);
    }
    pub fn failed(&self) -> bool { self.fault.load(Ordering::Relaxed) != 0 }
    pub fn error(&self) -> Option<&'static str> {
        match self.fault.load(Ordering::Relaxed) {
            0 => None,
            1 => Some("audio device error; streams stopped, no replacement device selected"),
            2 => Some("invalid or non-finite capture samples"),
            3 => Some("audio clock queue overflow; stopped instead of building unbounded latency"),
            4 => Some("resampler failed"),
            6 => Some("application audio capture failed or lost continuity; stopped without changing capture targets"),
            _ => Some("routing failed"),
        }
    }
}

pub struct OutputStats {
    pub underruns: AtomicU64,
    pub limited_frames: AtomicU64,
    pub queue_frames: AtomicU64,
    pub correction_ppm: AtomicI32,
    peaks: [AtomicU32; MAX_MIXES],
    pre_limiter_peaks: [AtomicU32; MAX_MIXES],
    minimum_gains: [AtomicU32; MAX_MIXES],
}
impl Default for OutputStats {
    fn default() -> Self {
        Self { underruns: AtomicU64::new(0), limited_frames: AtomicU64::new(0),
            queue_frames: AtomicU64::new(0), correction_ppm: AtomicI32::new(0),
            peaks: Default::default(), pre_limiter_peaks: Default::default(),
            minimum_gains: std::array::from_fn(|_| AtomicU32::new(1.0_f32.to_bits())) }
    }
}
impl OutputStats {
    /// One peak per stereo mix in endpoint lane order, after playback gain and limiting.
    /// Only the control thread consumes these interval maxima.
    pub fn take_peaks(&self) -> [f32; MAX_MIXES] {
        std::array::from_fn(|lane| f32::from_bits(self.peaks[lane].swap(0, Ordering::Relaxed)))
    }

    /// The processing path only publishes linear values; dB conversion stays off callbacks.
    pub fn take_limiters(&self) -> [LimiterReading; MAX_MIXES] {
        std::array::from_fn(|lane| {
            let input_peak = f32::from_bits(self.pre_limiter_peaks[lane].swap(0, Ordering::Relaxed));
            let gain = f32::from_bits(self.minimum_gains[lane].swap(1.0_f32.to_bits(), Ordering::Relaxed));
            LimiterReading { input_peak, reduction_db: if gain == 1.0 { 0.0 } else { -20.0 * gain.log10() } }
        })
    }
}

pub struct OutputWriter {
    producer: Producer<OutputFrame>,
    mixes: usize,
    health: Arc<Health>,
}
impl OutputWriter {
    pub fn mix_count(&self) -> usize { self.mixes }
    pub fn push(&mut self, frame: OutputFrame) {
        if self.health.failed() { return; }
        if !frame.iter().all(|x| x.is_finite()) {
            self.health.fail(Fault::Input);
        } else if self.producer.push(frame).is_err() {
            self.health.fail(Fault::Overflow);
        }
    }
}

pub struct OutputReader {
    consumer: Consumer<OutputFrame>,
    resampler: Async<f32>,
    channels: usize,
    input: Vec<f32>,
    output: Vec<f32>,
    cursor: usize,
    valid: usize,
    primed: bool,
    target: usize,
    input_rate: f64,
    alpha: f64,
    filtered_error: f64,
    limiters: [StereoLimiter; MAX_MIXES],
    pub queue_ms: f64,
    pub resampler_delay_ms: f64,
    pub stats: Arc<OutputStats>,
    health: Arc<Health>,
}

pub fn channel(input_rate: u32, output_rate: u32, queue_ms: u32, mixes: usize, health: Arc<Health>) -> Result<(OutputWriter, OutputReader), String> {
    if !(8_000..=192_000).contains(&input_rate) || !(8_000..=192_000).contains(&output_rate) || !(10..=100).contains(&queue_ms) {
        return Err("rates must be 8000..=192000 Hz and queue depth 10..=100 ms".into());
    }
    if !(1..=MAX_MIXES).contains(&mixes) { return Err("an endpoint carries one to three stereo mixes".into()); }
    let channels = mixes * 2;
    let resampler = Async::<f32>::new_sinc(
        f64::from(output_rate) / f64::from(input_rate), 1.002,
        &SincInterpolationParameters::default(), QUANTUM, channels, FixedAsync::Output,
    ).map_err(|e| e.to_string())?;
    let target = ((input_rate as usize * queue_ms as usize) / 1000).max(resampler.input_frames_max() * 3);
    let (producer, consumer) = RingBuffer::new(target * 4);
    let reader = OutputReader {
        consumer, channels,
        input: vec![0.0; resampler.input_frames_max() * channels],
        output: vec![0.0; resampler.output_frames_max() * channels],
        resampler_delay_ms: resampler.output_delay() as f64 * 1000.0 / f64::from(output_rate),
        resampler,
        cursor: 0, valid: 0, primed: false, target,
        input_rate: f64::from(input_rate),
        alpha: 1.0 - (-(QUANTUM as f64) / f64::from(output_rate) / 0.5).exp(),
        filtered_error: 0.0,
        limiters: std::array::from_fn(|_| StereoLimiter::new(output_rate)),
        queue_ms: target as f64 * 1000.0 / f64::from(input_rate),
        stats: Arc::new(OutputStats::default()),
        health: health.clone(),
    };
    Ok((OutputWriter { producer, mixes, health }, reader))
}

impl OutputReader {
    /// All mixes for ONE output frame. Validated linear playback gains (0..=1) are
    /// applied after resampling and before the independent stereo-linked sample limiters.
    pub(crate) fn next_frame(&mut self, gains: [f32; MAX_MIXES]) -> OutputFrame {
        let mut frame = self.read_frame();
        let mut limited = false;
        for (lane, stereo) in frame[..self.channels].chunks_exact_mut(2).enumerate() {
            for sample in stereo.iter_mut() { *sample *= gains[lane]; }
            let input_peak = stereo[0].abs().max(stereo[1].abs());
            let gain = self.limiters[lane].apply(stereo);
            if input_peak > 0.0 && gain < 1.0 {
                limited = true;
                self.stats.minimum_gains[lane].fetch_min(gain.to_bits(), Ordering::Relaxed);
            }
            self.stats.pre_limiter_peaks[lane].fetch_max(input_peak.to_bits(), Ordering::Relaxed);
            let peak = stereo[0].abs().max(stereo[1].abs());
            self.stats.peaks[lane].fetch_max(peak.to_bits(), Ordering::Relaxed);
        }
        if limited { self.stats.limited_frames.fetch_add(1, Ordering::Relaxed); }
        frame
    }

    /// Input-clock boundary for AUX/apps. Preserve float headroom until source gain and mixing.
    /// Silence on starvation; a latched device/format/overflow error stops the entire route.
    pub fn next_source_frame(&mut self) -> [f32; 2] {
        let frame = self.read_frame();
        [frame[0], frame[1]]
    }

    fn read_frame(&mut self) -> OutputFrame {
        if self.health.failed() || (self.cursor == self.valid && !self.refill()) { return [0.0; 6]; }
        let mut frame = [0.0; 6];
        let start = self.cursor * self.channels;
        frame[..self.channels].copy_from_slice(&self.output[start..start + self.channels]);
        self.cursor += 1;
        if !frame.iter().all(|x| x.is_finite()) {
            self.health.fail(Fault::Resampler);
            return [0.0; 6];
        }
        frame
    }

    fn refill(&mut self) -> bool {
        let available = self.consumer.slots();
        self.stats.queue_frames.store(available as u64, Ordering::Relaxed);
        if !self.primed {
            if available < self.target { return false; }
            self.primed = true;
        }
        self.filtered_error += self.alpha * (available as f64 - self.target as f64 - self.filtered_error);
        // A deeper queue means the input clock is faster: consume slightly more input per output.
        let correction = (self.filtered_error / (self.input_rate * 2.0)).clamp(-MAX_CORRECTION, MAX_CORRECTION);
        self.stats.correction_ppm.store((correction * 1_000_000.0) as i32, Ordering::Relaxed);
        if self.resampler.set_resample_ratio_relative(1.0 - correction, true).is_err() {
            self.health.fail(Fault::Resampler);
            return false;
        }
        let needed = self.resampler.input_frames_next();
        if available < needed {
            self.stats.underruns.fetch_add(1, Ordering::Relaxed);
            self.primed = false;
            self.filtered_error = 0.0;
            self.resampler.reset();
            return false;
        }
        for frame in self.input[..needed * self.channels].chunks_exact_mut(self.channels) {
            let Ok(value) = self.consumer.pop() else {
                self.health.fail(Fault::Resampler);
                return false;
            };
            frame.copy_from_slice(&value[..self.channels]);
        }
        let Ok(input) = InterleavedSlice::new(&self.input, self.channels, needed) else {
            self.health.fail(Fault::Resampler); return false;
        };
        let Ok(mut output) = InterleavedSlice::new_mut(&mut self.output, self.channels, QUANTUM) else {
            self.health.fail(Fault::Resampler); return false;
        };
        match self.resampler.process_into_buffer(&input, &mut output, None) {
            Ok((used, written)) if used == needed && written == QUANTUM => {
                self.cursor = 0; self.valid = written; true
            }
            _ => { self.health.fail(Fault::Resampler); false }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_conversion_keeps_stereo_and_faults_silence_pending_audio() {
        let health = Arc::new(Health::default());
        let (mut writer, mut reader) = channel(44_100, 48_000, 30, 1, health.clone()).unwrap();
        for _ in 0..reader.target { writer.push([0.2, -0.1, 0.0, 0.0, 0.0, 0.0]); }
        for _ in 0..100 {
            for _ in 0..reader.resampler.input_frames_next() { writer.push([0.2, -0.1, 0.0, 0.0, 0.0, 0.0]); }
            for _ in 0..QUANTUM { let _ = reader.next_frame([1.0; 3]); }
        }
        let frame = reader.next_frame([1.0; 3]);
        assert!((frame[0] - 0.2).abs() < 0.001 && (frame[1] + 0.1).abs() < 0.001);
        assert_eq!(&frame[2..], &[0.0; 4]);
        health.fail(Fault::Device);
        assert_eq!(reader.next_frame([1.0; 3]), [0.0; 6]);
    }

    #[test]
    fn independent_clock_drift_remains_bounded() {
        for ppm in [-500.0, 500.0] {
            let health = Arc::new(Health::default());
            let (mut writer, mut reader) = channel(16_000, 16_000, 30, 1, health.clone()).unwrap();
            for _ in 0..reader.target { writer.push([0.1, 0.1, 0.0, 0.0, 0.0, 0.0]); }
            let mut fraction = 0.0;
            for _ in 0..3750 { // 30 simulated seconds, no wall-clock sleeps.
                fraction += QUANTUM as f64 * (1.0 + ppm / 1_000_000.0);
                while fraction >= 1.0 { writer.push([0.1, 0.1, 0.0, 0.0, 0.0, 0.0]); fraction -= 1.0; }
                for _ in 0..QUANTUM { let _ = reader.next_frame([1.0; 3]); }
            }
            assert!(!health.failed(), "{ppm}: {:?}", health.error());
            assert_eq!(reader.stats.underruns.load(Ordering::Relaxed), 0);
            let remaining = reader.consumer.slots();
            assert!(remaining > reader.target / 3 && remaining < reader.target * 2, "{ppm}: {remaining}");
        }
    }

    #[test]
    fn starvation_overflow_and_sample_limiting_are_explicit() {
        let health = Arc::new(Health::default());
        let (mut writer, mut reader) = channel(48_000, 48_000, 30, 1, health.clone()).unwrap();
        assert_eq!(reader.next_frame([1.0; 3]), [0.0; 6]);
        for _ in 0..reader.target { writer.push([2.0, -2.0, 0.0, 0.0, 0.0, 0.0]); }
        for _ in 0..reader.target * 2 { assert!(reader.next_frame([1.0; 3]).iter().all(|x| x.abs() <= CEILING)); }
        assert!(reader.stats.limited_frames.load(Ordering::Relaxed) > 0);
        assert!(reader.stats.underruns.load(Ordering::Relaxed) > 0);
        assert_eq!(reader.next_frame([1.0; 3]), [0.0; 6]);
        for _ in 0..reader.target * 4 + 1 { writer.push([0.2, 0.2, 0.0, 0.0, 0.0, 0.0]); }
        assert!(health.failed());
        assert_eq!(reader.next_frame([1.0; 3]), [0.0; 6]);
        assert!(channel(0, 48_000, 30, 1, health.clone()).is_err());
        assert!(channel(48_000, 48_000, 0, 1, health.clone()).is_err());
        for mixes in [0, 4] { assert!(channel(48_000, 48_000, 30, mixes, health.clone()).is_err()); }
    }

    #[test]
    fn shared_mixes_keep_sample_alignment_and_silence_together() {
        let health = Arc::new(Health::default());
        let (mut writer, mut reader) = channel(44_100, 48_000, 10, 3, health.clone()).unwrap();
        let mut position = 0;
        let mut push = |count: usize| {
            for _ in 0..count {
                let a = (position as f32 * 0.07).sin() * 0.3;
                let b = (position as f32 * 0.13).cos() * 0.2;
                writer.push([a, b, a * 0.25, b * -0.5, -a * 0.125, b * 0.75]);
                position += 1;
            }
        };
        push(reader.target);
        let mut peak = 0.0_f32;
        for _ in 0..50 {
            push(reader.resampler.input_frames_next());
            for _ in 0..QUANTUM {
                let frame = reader.next_frame([1.0; 3]);
                assert!((frame[2] - frame[0] * 0.25).abs() < 1e-6);
                assert!((frame[3] + frame[1] * 0.5).abs() < 1e-6);
                assert!((frame[4] + frame[0] * 0.125).abs() < 1e-6);
                assert!((frame[5] - frame[1] * 0.75).abs() < 1e-6);
                peak = peak.max(frame[0].abs());
            }
        }
        assert!(peak > 0.2);
        assert!(!health.failed());
        for _ in 0..reader.target * 3 { reader.next_frame([1.0; 3]); }
        assert!(reader.stats.underruns.load(Ordering::Relaxed) > 0);
        assert_eq!(reader.next_frame([1.0; 3]), [0.0; 6]);
        health.fail(Fault::Device);
        assert_eq!(reader.next_frame([1.0; 3]), [0.0; 6]);
    }
    #[test]
    fn linked_limiter_preserves_stereo_ratio_and_recovers_without_lookahead() {
        for rate in [8000, 44100, 48000, 192000] {
            let mut limiter = StereoLimiter::new(rate);
            let mut frame = [0.3, -0.2];
            assert_eq!(limiter.apply(&mut frame), 1.0);
            assert_eq!(frame, [0.3, -0.2]);
            frame = [2.0, -0.5];
            let attacked = limiter.apply(&mut frame);
            assert!((attacked - CEILING / 2.0).abs() < 1e-6);
            assert_eq!(frame[0], CEILING);
            assert!((frame[1] + CEILING * 0.25).abs() < 1e-6);
            let mut previous = attacked;
            for _ in 0..(rate / 10) {
                frame = [0.2, -0.1];
                let recovered = limiter.apply(&mut frame);
                assert!(recovered >= previous && recovered <= 1.0);
                assert!((frame[0] + 2.0 * frame[1]).abs() < 1e-7);
                previous = recovered;
            }
            assert_eq!(previous, 1.0);
            assert_eq!(frame, [0.2, -0.1]);
        }
    }

    #[test]
    fn shared_endpoint_limits_each_mix_independently_and_reports_actual_attenuation() {
        let health = Arc::new(Health::default());
        let (mut writer, mut reader) = channel(48000, 48000, 30, 3, health.clone()).unwrap();
        let input = [2.0, -0.5, 0.25, -0.125, -1.5, 0.3];
        for _ in 0..reader.target + 1024 { writer.push(input); }
        let mut output = [0.0; 6];
        for _ in 0..1024 { output = reader.next_frame([1.0; 3]); }
        for (actual, expected) in output.into_iter().zip([CEILING, -CEILING * 0.25, 0.25, -0.125, -CEILING, CEILING * 0.2]) {
            assert!((actual - expected).abs() < 0.001, "{actual} != {expected}");
        }
        let meters = reader.stats.take_limiters();
        assert!(meters[0].input_peak > 1.9 && meters[0].reduction_db > 6.0);
        assert!(meters[1].input_peak < 0.4 && meters[1].reduction_db == 0.0);
        assert!(meters[2].input_peak > 1.4 && meters[2].reduction_db > 3.0);
        assert!(reader.stats.limited_frames.load(Ordering::Relaxed) > 0);
        assert!(reader.stats.take_limiters().iter().all(|m| m.input_peak == 0.0 && m.reduction_db == 0.0));
        assert!(!health.failed());
    }

    #[test]
    fn aux_clock_conversion_preserves_headroom_before_source_attenuation() {
        let health = Arc::new(Health::default());
        let (mut writer, mut reader) = channel(44_100, 48_000, 10, 1, health.clone()).unwrap();
        assert_eq!(reader.next_source_frame(), [0.0; 2]);
        for _ in 0..reader.target { writer.push([2.0, -1.5, 0.0, 0.0, 0.0, 0.0]); }
        let mut sample = [0.0; 2];
        for _ in 0..12 {
            for _ in 0..reader.resampler.input_frames_next() { writer.push([2.0, -1.5, 0.0, 0.0, 0.0, 0.0]); }
            for _ in 0..QUANTUM { sample = reader.next_source_frame(); }
        }
        assert!((sample[0] - 2.0).abs() < 0.001 && (sample[1] + 1.5).abs() < 0.001);
        assert_eq!(reader.stats.limited_frames.load(Ordering::Relaxed), 0);
        for _ in 0..reader.target * 3 { reader.next_source_frame(); }
        assert!(reader.stats.underruns.load(Ordering::Relaxed) > 0);
        assert_eq!(reader.next_source_frame(), [0.0; 2]);
        writer.push([f32::NAN, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert!(health.failed());
        assert_eq!(reader.next_source_frame(), [0.0; 2]);
    }

}
