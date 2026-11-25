//! Non-intrusive microphone pitch observation. The audio callback publishes complete
//! contiguous windows into a bounded SPSC queue; estimation stays on the control thread.
//! No voice modification, note correction, file output or additional audio-path latency.
//! Normalized square difference: McLeod & Wyvill, "A Smarter Way to Find Pitch" (2005).
//! Use FFT autocorrelation and exact overlapping energies, not an FFT-scaled denominator.
use crate::config::PitchScale;
use rtrb::{Consumer, Producer, RingBuffer};
use rustfft::{Fft, FftPlanner, num_complex::Complex64};
use std::sync::Arc;

const MIN_HZ: f64 = 70.0;
const MAX_HZ: f64 = 1200.0;
const MIN_CLARITY: f64 = 0.80;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PitchReading {
    pub frequency_hz: f64,
    pub midi_note: u8,
    pub cents: f64,
    pub clarity: f64,
    pub target_midi_note: u8,
    /// Signed measured pitch minus the selected scale target, in cents.
    pub target_cents: f64,
}
impl PitchReading {
    pub fn note_name(self) -> String {
        note_name(self.midi_note)
    }
    pub fn target_note_name(self) -> String { note_name(self.target_midi_note) }
}
fn note_name(note: u8) -> String {
    format!("{}{}", PitchScale::NAMES[usize::from(note % 12)], i16::from(note / 12) - 1)
}

/// All FFT plans and buffers are prepared once; never select a pitch beyond the
/// supported lag window or compare FFT-scaled correlation with unscaled energy.
struct Estimator {
    rate: usize,
    fft: Arc<dyn Fft<f64>>,
    inverse: Arc<dyn Fft<f64>>,
    spectrum: Vec<Complex64>,
    scratch: Vec<Complex64>,
    energy: Vec<f64>,
    nsdf: Vec<f64>,
}
impl Estimator {
    fn new(rate: usize, size: usize) -> Self {
        let fft_len = size * 2;
        let mut planner = FftPlanner::<f64>::new();
        let fft = planner.plan_fft_forward(fft_len);
        let inverse = planner.plan_fft_inverse(fft_len);
        let scratch_len = fft.get_inplace_scratch_len().max(inverse.get_inplace_scratch_len());
        Self { rate, fft, inverse, spectrum: vec![Complex64::default(); fft_len],
            scratch: vec![Complex64::default(); scratch_len], energy: vec![0.0; size + 1],
            // Include a neighbor for quadratic interpolation at the lowest supported note.
            nsdf: vec![0.0; (rate as f64 / MIN_HZ).ceil() as usize + 2] }
    }

    fn estimate(&mut self, signal: &[f64]) -> Option<(f64, f64)> {
        if signal.iter().any(|sample| !sample.is_finite()) { return None; }
        let n = signal.len();
        let mean = signal.iter().sum::<f64>() / n as f64;
        self.energy[0] = 0.0;
        for (i, (&sample, bin)) in signal.iter().zip(&mut self.spectrum).enumerate() {
            let x = sample - mean;
            *bin = Complex64::new(x, 0.0);
            self.energy[i + 1] = self.energy[i] + x * x;
        }
        let total = self.energy[n];
        if total / (n as f64) < 1e-5 { return None; } // mean-square -50 dBFS
        self.spectrum[n..].fill(Complex64::default());
        self.fft.process_with_scratch(&mut self.spectrum, &mut self.scratch);
        for bin in &mut self.spectrum { *bin = Complex64::new(bin.norm_sqr(), 0.0); }
        self.inverse.process_with_scratch(&mut self.spectrum, &mut self.scratch);
        let fft_len = self.spectrum.len() as f64;
        // At lag t: sum of x[i]^2 + x[i+t]^2 for 0 <= i < n-t.
        for (lag, value) in self.nsdf.iter_mut().enumerate() {
            let denominator = self.energy[n - lag] + (total - self.energy[lag]);
            *value = if denominator > 0.0 {
                (2.0 * self.spectrum[lag].re / (fft_len * denominator)).clamp(-1.0, 1.0)
            } else { 0.0 };
        }
        // Exclude the initial positive lobe around lag zero. Search the first
        // sufficiently strong periodic peak, rather than a later harmonic multiple.
        let first_negative = self.nsdf.iter().position(|&v| v < 0.0)?;
        let peaks = (first_negative.max(1)..self.nsdf.len() - 1).filter(|&lag| {
            self.nsdf[lag] >= MIN_CLARITY && self.nsdf[lag] > self.nsdf[lag - 1] && self.nsdf[lag] >= self.nsdf[lag + 1]
        });
        let best = peaks.clone().map(|lag| self.nsdf[lag]).fold(0.0_f64, f64::max);
        let lag = peaks.into_iter().find(|&lag| self.nsdf[lag] >= 0.9 * best)?;
        let (a, b, c) = (self.nsdf[lag - 1], self.nsdf[lag], self.nsdf[lag + 1]);
        let curvature = a - 2.0 * b + c;
        let offset = (0.5 * (a - c) / curvature).clamp(-0.5, 0.5);
        let hz = self.rate as f64 / (lag as f64 + offset);
        if !(MIN_HZ..=MAX_HZ).contains(&hz) { return None; }
        let clarity = (b - 0.25 * (a - c) * offset).clamp(0.0, 1.0);
        Some((hz, clarity))
    }
}

pub(crate) struct PitchTap {
    writer: Producer<f32>,
    window: Vec<f32>,
    filled: usize,
}
pub(crate) struct PitchMonitor {
    reader: Consumer<f32>,
    signal: Vec<f64>,
    detector: Estimator,
    key: u8,
    scale: PitchScale,
}

pub(crate) fn channel(rate: u32, key: u8, scale: PitchScale) -> Result<(PitchTap, PitchMonitor), String> {
    if !(8000..=192_000).contains(&rate) { return Err("unsupported pitch monitor sample rate".into()); }
    if key > 11 { return Err("invalid pitch reference key".into()); }
    let rate = rate as usize;
    // 40–80 ms, at the actual microphone sample rate; not an audio delay.
    let size = (rate / 25).next_power_of_two();
    let capacity = size * (rate * 2 / size);
    let (writer, reader) = RingBuffer::new(capacity);
    Ok((PitchTap { writer, window: vec![0.0; size], filled: 0 },
        PitchMonitor { reader, signal: vec![0.0; size], detector: Estimator::new(rate, size), key, scale }))
}

impl PitchTap {
    pub(crate) fn push(&mut self, mut samples: &[f32]) {
        while !samples.is_empty() {
            let count = samples.len().min(self.window.len() - self.filled);
            self.window[self.filled..self.filled + count].copy_from_slice(&samples[..count]);
            self.filled += count;
            samples = &samples[count..];
            if self.filled == self.window.len() {
                // Only publish complete windows. A slow observer may miss measurements,
                // but never blocks the callback, fills an audio queue or faults the route.
                if self.writer.slots() >= self.window.len() {
                    for &sample in &self.window {
                        let _ = self.writer.push(sample);
                    }
                }
                self.filled = 0;
            }
        }
    }
}

impl PitchMonitor {
    /// Consume only complete windows; analyze the newest available one, not every queued frame.
    /// No new observation or no sufficiently clear pitch means None, never a held old note.
    pub(crate) fn take(&mut self) -> Option<PitchReading> {
        let windows = self.reader.slots() / self.signal.len();
        if windows == 0 { return None; }
        for _ in 0..windows {
            for sample in &mut self.signal { *sample = f64::from(self.reader.pop().ok()?); }
        }
        let (frequency_hz, clarity) = self.detector.estimate(&self.signal)?;
        let note = 69.0 + 12.0 * (frequency_hz / 440.0).log2();
        let nearest = note.round();
        let target_midi_note = self.scale.nearest_note(self.key, note);
        Some(PitchReading { frequency_hz, midi_note: nearest as u8,
            cents: (note - nearest) * 100.0, clarity, target_midi_note,
            target_cents: (note - f64::from(target_midi_note)) * 100.0 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, count: usize, hz: f64) -> Vec<f32> {
        (0..count).map(|i| (0.2 * (std::f64::consts::TAU * hz * i as f64 / f64::from(rate)).sin()) as f32).collect()
    }

    #[test]
    fn estimates_note_and_cents_at_supported_sample_rates() {
        for rate in [8000, 44100, 48000, 96000, 192000] {
            let (mut tap, mut monitor) = channel(rate, 0, PitchScale::Chromatic).unwrap();
            let size = tap.window.len();
            for (hz, note, name) in [(75.0, 38, "D2"), (110.0, 45, "A2"), (220.0, 57, "A3"), (445.0, 69, "A4"), (1190.0, 86, "D6")] {
                let signal = tone(rate, size, hz);
                for part in signal.chunks(137) { tap.push(part); }
                let reading = monitor.take().unwrap_or_else(|| panic!("missing clear {hz} Hz tone at {rate} Hz"));
                assert_eq!(reading.midi_note, note);
                assert_eq!(reading.note_name(), name);
                assert!((reading.frequency_hz / hz - 1.0).abs() < 0.005, "{rate}: {reading:?}");
                let expected = 100.0 * (69.0 + 12.0 * (hz / 440.0).log2() - f64::from(note));
                assert!((reading.cents - expected).abs() < 9.0);
                assert!(reading.clarity >= MIN_CLARITY);
                assert_eq!(reading.target_midi_note, reading.midi_note);
                assert_eq!(reading.target_cents, reading.cents);
                assert!(monitor.take().is_none());
            }
        }
    }

    #[test]
    fn complete_windows_only_and_silence_dc_or_invalid_input_do_not_hold_previous_note() {
        let (mut tap, mut monitor) = channel(48000, 0, PitchScale::Chromatic).unwrap();
        let size = tap.window.len();
        let signal = tone(48000, size, 440.0);
        tap.push(&signal[..size - 1]);
        assert!(monitor.take().is_none());
        tap.push(&signal[size - 1..]);
        assert_eq!(monitor.take().unwrap().note_name(), "A4");
        for value in [0.0, 0.2, f32::NAN] {
            tap.push(&vec![value; size]);
            assert!(monitor.take().is_none());
        }
    }

    #[test]
    fn normalized_detector_rejects_out_of_range_tones_and_tracks_harmonic_fundamental() {
        for rate in [8000, 48000, 192000] {
            let (mut tap, mut monitor) = channel(rate, 0, PitchScale::Chromatic).unwrap();
            let size = tap.window.len();
            for hz in [55.0, 1600.0] {
                tap.push(&tone(rate, size, hz));
                assert!(monitor.take().is_none(), "out of range {hz} Hz at {rate} Hz");
            }
            let mut input = tone(rate, size, 110.0);
            let harmonics = tone(rate, size, 220.0);
            for (sample, harmonic) in input.iter_mut().zip(harmonics) {
                *sample += 0.7 * harmonic;
            }
            tap.push(&input);
            assert!((monitor.take().unwrap().frequency_hz - 110.0).abs() < 0.6);
        }
    }

    #[test]
    fn reference_scales_select_nearest_allowed_note_without_modifying_observation() {
        let (mut tap, mut monitor) = channel(48000, 0, PitchScale::Major).unwrap();
        let size = tap.window.len();
        // C#4 slightly sharp is between C4 and D4; reference must be D4.
        let hz = 440.0 * 2.0_f64.powf((61.1 - 69.0) / 12.0);
        tap.push(&tone(48000, size, hz));
        let reading = monitor.take().unwrap();
        assert_eq!(reading.note_name(), "C#4");
        assert_eq!(reading.target_note_name(), "D4");
        assert!((reading.cents - 10.0).abs() < 1.0);
        assert!((reading.target_cents + 90.0).abs() < 1.0);
        assert_eq!(PitchScale::Major.nearest_note(0, 61.0), 60);
        assert_eq!(PitchScale::Minor.nearest_note(0, 63.1), 63);
        assert_eq!(PitchScale::Major.nearest_note(2, 61.0), 61);
        assert_eq!(PitchScale::Chromatic.nearest_note(11, 61.4), 61);
        assert!(channel(48000, 12, PitchScale::Major).is_err());
    }

    #[test]
    fn slow_observer_drops_whole_windows_and_resumes_with_current_signal() {
        let (mut tap, mut monitor) = channel(8000, 0, PitchScale::Chromatic).unwrap();
        let size = tap.window.len();
        let first = tone(8000, size, 220.0);
        let next = tone(8000, size, 440.0);
        tap.push(&first);
        tap.push(&next);
        assert_eq!(monitor.take().unwrap().note_name(), "A4");
        let capacity = tap.writer.slots();
        for _ in 0..(capacity / size + 3) { tap.push(&first); }
        assert_eq!(tap.writer.slots(), 0);
        assert_eq!(monitor.take().unwrap().note_name(), "A3");
        assert!(monitor.take().is_none());
        tap.push(&next);
        assert_eq!(monitor.take().unwrap().note_name(), "A4");
    }
}
