//! Bounded SPSC transport and independently clocked outputs. No device API here.
//! Rubato owns the anti-alias filter; we only steer its ratio from queue occupancy.
use std::sync::{Arc, atomic::{AtomicI32, AtomicU32, AtomicU64, AtomicU8, Ordering}};
use rtrb::{Consumer, Producer, RingBuffer};
use rubato::{Adjustable, Async, FixedAsync, Resampler, SincInterpolationParameters};
use rubato::audioadapter_buffers::direct::InterleavedSlice;

pub const QUANTUM: usize = 128;
const MAX_CORRECTION: f64 = 0.001; // +/- 1000 ppm. This is not an audio device latency claim.
const CEILING: f32 = 0.98; // Sample clipping guard, NOT a transparent or true-peak limiter.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Fault { Device = 1, Input = 2, Overflow = 3, Resampler = 4, Routing = 5 }

#[derive(Default)]
pub struct Health {
    fault: AtomicU8,
    pub captured_frames: AtomicU64,
    pub input_peak: AtomicU32,
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
            3 => Some("output queue overflow; stopped instead of building unbounded latency"),
            4 => Some("resampler failed"),
            _ => Some("routing failed"),
        }
    }
}

#[derive(Default)]
pub struct OutputStats {
    pub underruns: AtomicU64,
    pub clipped_frames: AtomicU64,
    pub queue_frames: AtomicU64,
    pub correction_ppm: AtomicI32,
    pub peak: AtomicU32,
}

pub struct OutputWriter {
    producer: Producer<[f32; 2]>,
    health: Arc<Health>,
}
impl OutputWriter {
    pub fn push(&mut self, frame: [f32; 2]) {
        if self.health.failed() { return; }
        if !frame.iter().all(|x| x.is_finite()) {
            self.health.fail(Fault::Input);
        } else if self.producer.push(frame).is_err() {
            self.health.fail(Fault::Overflow);
        }
    }
}

pub struct OutputReader {
    consumer: Consumer<[f32; 2]>,
    resampler: Async<f32>,
    input: Vec<f32>,
    output: Vec<f32>,
    cursor: usize,
    valid: usize,
    primed: bool,
    target: usize,
    input_rate: f64,
    alpha: f64,
    filtered_error: f64,
    pub queue_ms: f64,
    pub resampler_delay_ms: f64,
    pub stats: Arc<OutputStats>,
    health: Arc<Health>,
}

pub fn channel(input_rate: u32, output_rate: u32, queue_ms: u32, health: Arc<Health>) -> Result<(OutputWriter, OutputReader), String> {
    if !(8_000..=192_000).contains(&input_rate) || !(8_000..=192_000).contains(&output_rate) || !(10..=100).contains(&queue_ms) {
        return Err("rates must be 8000..=192000 Hz and queue depth 10..=100 ms".into());
    }
    let resampler = Async::<f32>::new_sinc(
        f64::from(output_rate) / f64::from(input_rate), 1.002,
        &SincInterpolationParameters::default(), QUANTUM, 2, FixedAsync::Output,
    ).map_err(|e| e.to_string())?;
    let target = ((input_rate as usize * queue_ms as usize) / 1000).max(resampler.input_frames_max() * 3);
    let (producer, consumer) = RingBuffer::new(target * 4);
    let reader = OutputReader {
        consumer,
        input: vec![0.0; resampler.input_frames_max() * 2],
        output: vec![0.0; resampler.output_frames_max() * 2],
        resampler_delay_ms: resampler.output_delay() as f64 * 1000.0 / f64::from(output_rate),
        resampler,
        cursor: 0, valid: 0, primed: false, target,
        input_rate: f64::from(input_rate),
        alpha: 1.0 - (-(QUANTUM as f64) / f64::from(output_rate) / 0.5).exp(),
        filtered_error: 0.0,
        queue_ms: target as f64 * 1000.0 / f64::from(input_rate),
        stats: Arc::new(OutputStats::default()),
        health: health.clone(),
    };
    Ok((OutputWriter { producer, health }, reader))
}

impl OutputReader {
    /// One stereo frame. Underrun and any latched fault yield silence, never stale or dry audio.
    pub fn next_frame(&mut self) -> [f32; 2] {
        if self.health.failed() || (self.cursor == self.valid && !self.refill()) { return [0.0; 2]; }
        let mut frame = [self.output[self.cursor * 2], self.output[self.cursor * 2 + 1]];
        self.cursor += 1;
        if !frame.iter().all(|x| x.is_finite()) {
            self.health.fail(Fault::Resampler);
            return [0.0; 2];
        }
        if frame.iter().any(|x| x.abs() > CEILING) {
            self.stats.clipped_frames.fetch_add(1, Ordering::Relaxed);
            frame.iter_mut().for_each(|x| *x = x.clamp(-CEILING, CEILING));
        }
        self.stats.peak.fetch_max(frame[0].abs().max(frame[1].abs()).to_bits(), Ordering::Relaxed);
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
        for frame in self.input[..needed * 2].chunks_exact_mut(2) {
            let Ok(value) = self.consumer.pop() else {
                self.health.fail(Fault::Resampler);
                return false;
            };
            frame.copy_from_slice(&value);
        }
        let Ok(input) = InterleavedSlice::new(&self.input, 2, needed) else {
            self.health.fail(Fault::Resampler); return false;
        };
        let Ok(mut output) = InterleavedSlice::new_mut(&mut self.output, 2, QUANTUM) else {
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
        let (mut writer, mut reader) = channel(44_100, 48_000, 30, health.clone()).unwrap();
        for _ in 0..reader.target { writer.push([0.2, -0.1]); }
        for _ in 0..100 {
            for _ in 0..reader.resampler.input_frames_next() { writer.push([0.2, -0.1]); }
            for _ in 0..QUANTUM { let _ = reader.next_frame(); }
        }
        let frame = reader.next_frame();
        assert!((frame[0] - 0.2).abs() < 0.001 && (frame[1] + 0.1).abs() < 0.001);
        health.fail(Fault::Device);
        assert_eq!(reader.next_frame(), [0.0; 2]);
    }

    #[test]
    fn independent_clock_drift_remains_bounded() {
        for ppm in [-500.0, 500.0] {
            let health = Arc::new(Health::default());
            let (mut writer, mut reader) = channel(16_000, 16_000, 30, health.clone()).unwrap();
            for _ in 0..reader.target { writer.push([0.1, 0.1]); }
            let mut fraction = 0.0;
            for _ in 0..3750 { // 30 simulated seconds, no wall-clock sleeps.
                fraction += QUANTUM as f64 * (1.0 + ppm / 1_000_000.0);
                while fraction >= 1.0 { writer.push([0.1, 0.1]); fraction -= 1.0; }
                for _ in 0..QUANTUM { let _ = reader.next_frame(); }
            }
            assert!(!health.failed(), "{ppm}: {:?}", health.error());
            assert_eq!(reader.stats.underruns.load(Ordering::Relaxed), 0);
            let remaining = reader.consumer.slots();
            assert!(remaining > reader.target / 3 && remaining < reader.target * 2, "{ppm}: {remaining}");
        }
    }

    #[test]
    fn starvation_overflow_and_clip_guard_are_explicit() {
        let health = Arc::new(Health::default());
        let (mut writer, mut reader) = channel(48_000, 48_000, 30, health.clone()).unwrap();
        assert_eq!(reader.next_frame(), [0.0; 2]);
        for _ in 0..reader.target { writer.push([2.0, -2.0]); }
        for _ in 0..reader.target * 2 { assert!(reader.next_frame().iter().all(|x| x.abs() <= CEILING)); }
        assert!(reader.stats.clipped_frames.load(Ordering::Relaxed) > 0);
        assert!(reader.stats.underruns.load(Ordering::Relaxed) > 0);
        assert_eq!(reader.next_frame(), [0.0; 2]);
        for _ in 0..reader.target * 4 + 1 { writer.push([0.2, 0.2]); }
        assert!(health.failed());
        assert_eq!(reader.next_frame(), [0.0; 2]);
        assert!(channel(0, 48_000, 30, health.clone()).is_err());
        assert!(channel(48_000, 48_000, 0, health).is_err());
    }
}
