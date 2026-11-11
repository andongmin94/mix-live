//! Bounded SPSC clock transport. Output mixes share a bridge; AUX uses one at its input boundary.
//! Rubato owns the anti-alias filter; we only steer its ratio from queue occupancy.
use std::sync::{Arc, atomic::{AtomicI32, AtomicU32, AtomicU64, AtomicU8, Ordering}};
use rtrb::{Consumer, Producer, RingBuffer};
use rubato::{Adjustable, Async, FixedAsync, Resampler, SincInterpolationParameters};
use rubato::audioadapter_buffers::direct::InterleavedSlice;

pub const QUANTUM: usize = 128;
/// At most two stereo mixes, packed in the endpoint's route order. Unused lanes stay zero.
pub type OutputFrame = [f32; 4];
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
    pub aux_peak: AtomicU32,
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
    pub queue_ms: f64,
    pub resampler_delay_ms: f64,
    pub stats: Arc<OutputStats>,
    health: Arc<Health>,
}

pub fn channel(input_rate: u32, output_rate: u32, queue_ms: u32, mixes: usize, health: Arc<Health>) -> Result<(OutputWriter, OutputReader), String> {
    if !(8_000..=192_000).contains(&input_rate) || !(8_000..=192_000).contains(&output_rate) || !(10..=100).contains(&queue_ms) {
        return Err("rates must be 8000..=192000 Hz and queue depth 10..=100 ms".into());
    }
    if !(1..=2).contains(&mixes) { return Err("an endpoint carries one or two stereo mixes".into()); }
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
        queue_ms: target as f64 * 1000.0 / f64::from(input_rate),
        stats: Arc::new(OutputStats::default()),
        health: health.clone(),
    };
    Ok((OutputWriter { producer, mixes, health }, reader))
}

impl OutputReader {
    /// All mixes for ONE output frame, with the final sample clipping guard.
    pub fn next_frame(&mut self) -> OutputFrame {
        let mut frame = self.read_frame();
        if frame.iter().any(|x| x.abs() > CEILING) {
            self.stats.clipped_frames.fetch_add(1, Ordering::Relaxed);
            frame.iter_mut().for_each(|x| *x = x.clamp(-CEILING, CEILING));
        }
        let peak = frame.iter().fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
        self.stats.peak.fetch_max(peak.to_bits(), Ordering::Relaxed);
        frame
    }

    /// Input-clock boundary for AUX. Preserve float headroom until source gain and mixing.
    /// Silence on starvation; a latched device/format/overflow error stops the entire route.
    pub fn next_source_frame(&mut self) -> [f32; 2] {
        let frame = self.read_frame();
        [frame[0], frame[1]]
    }

    fn read_frame(&mut self) -> OutputFrame {
        if self.health.failed() || (self.cursor == self.valid && !self.refill()) { return [0.0; 4]; }
        let mut frame = [0.0; 4];
        let start = self.cursor * self.channels;
        frame[..self.channels].copy_from_slice(&self.output[start..start + self.channels]);
        self.cursor += 1;
        if !frame.iter().all(|x| x.is_finite()) {
            self.health.fail(Fault::Resampler);
            return [0.0; 4];
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
        for _ in 0..reader.target { writer.push([0.2, -0.1, 0.0, 0.0]); }
        for _ in 0..100 {
            for _ in 0..reader.resampler.input_frames_next() { writer.push([0.2, -0.1, 0.0, 0.0]); }
            for _ in 0..QUANTUM { let _ = reader.next_frame(); }
        }
        let frame = reader.next_frame();
        assert!((frame[0] - 0.2).abs() < 0.001 && (frame[1] + 0.1).abs() < 0.001);
        assert_eq!(&frame[2..], &[0.0; 2]);
        health.fail(Fault::Device);
        assert_eq!(reader.next_frame(), [0.0; 4]);
    }

    #[test]
    fn independent_clock_drift_remains_bounded() {
        for ppm in [-500.0, 500.0] {
            let health = Arc::new(Health::default());
            let (mut writer, mut reader) = channel(16_000, 16_000, 30, 1, health.clone()).unwrap();
            for _ in 0..reader.target { writer.push([0.1, 0.1, 0.0, 0.0]); }
            let mut fraction = 0.0;
            for _ in 0..3750 { // 30 simulated seconds, no wall-clock sleeps.
                fraction += QUANTUM as f64 * (1.0 + ppm / 1_000_000.0);
                while fraction >= 1.0 { writer.push([0.1, 0.1, 0.0, 0.0]); fraction -= 1.0; }
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
        let (mut writer, mut reader) = channel(48_000, 48_000, 30, 1, health.clone()).unwrap();
        assert_eq!(reader.next_frame(), [0.0; 4]);
        for _ in 0..reader.target { writer.push([2.0, -2.0, 0.0, 0.0]); }
        for _ in 0..reader.target * 2 { assert!(reader.next_frame().iter().all(|x| x.abs() <= CEILING)); }
        assert!(reader.stats.clipped_frames.load(Ordering::Relaxed) > 0);
        assert!(reader.stats.underruns.load(Ordering::Relaxed) > 0);
        assert_eq!(reader.next_frame(), [0.0; 4]);
        for _ in 0..reader.target * 4 + 1 { writer.push([0.2, 0.2, 0.0, 0.0]); }
        assert!(health.failed());
        assert_eq!(reader.next_frame(), [0.0; 4]);
        assert!(channel(0, 48_000, 30, 1, health.clone()).is_err());
        assert!(channel(48_000, 48_000, 0, 1, health.clone()).is_err());
        for mixes in [0, 3] { assert!(channel(48_000, 48_000, 30, mixes, health.clone()).is_err()); }
    }

    #[test]
    fn shared_mixes_keep_sample_alignment_and_silence_together() {
        let health = Arc::new(Health::default());
        let (mut writer, mut reader) = channel(44_100, 48_000, 10, 2, health.clone()).unwrap();
        let mut position = 0;
        let mut push = |count: usize| {
            for _ in 0..count {
                let a = (position as f32 * 0.07).sin() * 0.3;
                let b = (position as f32 * 0.13).cos() * 0.2;
                writer.push([a, b, a * 0.25, b * -0.5]);
                position += 1;
            }
        };
        push(reader.target);
        let mut peak = 0.0_f32;
        for _ in 0..50 {
            push(reader.resampler.input_frames_next());
            for _ in 0..QUANTUM {
                let frame = reader.next_frame();
                assert!((frame[2] - frame[0] * 0.25).abs() < 1e-6);
                assert!((frame[3] + frame[1] * 0.5).abs() < 1e-6);
                peak = peak.max(frame[0].abs());
            }
        }
        assert!(peak > 0.2);
        assert!(!health.failed());
        for _ in 0..reader.target * 3 { reader.next_frame(); }
        assert!(reader.stats.underruns.load(Ordering::Relaxed) > 0);
        assert_eq!(reader.next_frame(), [0.0; 4]);
        health.fail(Fault::Device);
        assert_eq!(reader.next_frame(), [0.0; 4]);
    }
    #[test]
    fn aux_clock_conversion_preserves_headroom_before_source_attenuation() {
        let health = Arc::new(Health::default());
        let (mut writer, mut reader) = channel(44_100, 48_000, 10, 1, health.clone()).unwrap();
        assert_eq!(reader.next_source_frame(), [0.0; 2]);
        for _ in 0..reader.target { writer.push([2.0, -1.5, 0.0, 0.0]); }
        let mut sample = [0.0; 2];
        for _ in 0..12 {
            for _ in 0..reader.resampler.input_frames_next() { writer.push([2.0, -1.5, 0.0, 0.0]); }
            for _ in 0..QUANTUM { sample = reader.next_source_frame(); }
        }
        assert!((sample[0] - 2.0).abs() < 0.001 && (sample[1] + 1.5).abs() < 0.001);
        assert_eq!(reader.stats.clipped_frames.load(Ordering::Relaxed), 0);
        for _ in 0..reader.target * 3 { reader.next_source_frame(); }
        assert!(reader.stats.underruns.load(Ordering::Relaxed) > 0);
        assert_eq!(reader.next_source_frame(), [0.0; 2]);
        writer.push([f32::NAN, 0.0, 0.0, 0.0]);
        assert!(health.failed());
        assert_eq!(reader.next_source_frame(), [0.0; 2]);
    }

}
