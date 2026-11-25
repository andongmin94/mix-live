//! Non-intrusive microphone pitch observation. The audio callback publishes complete
//! contiguous windows into a bounded SPSC queue; estimation stays on the control thread.
//! No voice modification, note correction, file output or additional audio-path latency.
use pitch_detection::detector::{PitchDetector, mcleod::McLeodDetector};
use rtrb::{Consumer, Producer, RingBuffer};

const MIN_HZ: f64 = 70.0;
const MAX_HZ: f64 = 1200.0;
const MIN_CLARITY: f64 = 0.80;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PitchReading {
    pub frequency_hz: f64,
    pub midi_note: u8,
    pub cents: f64,
    pub clarity: f64,
}
impl PitchReading {
    pub fn note_name(self) -> String {
        const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
        format!("{}{}", NAMES[usize::from(self.midi_note % 12)], i16::from(self.midi_note / 12) - 1)
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
    detector: McLeodDetector<f64>,
    rate: usize,
}

pub(crate) fn channel(rate: u32) -> Result<(PitchTap, PitchMonitor), String> {
    if !(8000..=192_000).contains(&rate) { return Err("unsupported pitch monitor sample rate".into()); }
    let rate = rate as usize;
    // 40–80 ms, at the actual microphone sample rate; not an audio delay.
    let size = (rate / 25).next_power_of_two();
    let capacity = size * (rate * 2 / size);
    let (writer, reader) = RingBuffer::new(capacity);
    Ok((PitchTap { writer, window: vec![0.0; size], filled: 0 },
        PitchMonitor { reader, signal: vec![0.0; size], detector: McLeodDetector::new(size, size), rate }))
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
        if self.signal.iter().any(|value| !value.is_finite()) { return None; }
        // Remove DC before detection; the threshold is mean-square -50 dBFS.
        let mean = self.signal.iter().sum::<f64>() / self.signal.len() as f64;
        for sample in &mut self.signal { *sample -= mean; }
        let power_threshold = self.signal.len() as f64 * 1e-5;
        let pitch = self.detector.get_pitch(&self.signal, self.rate, power_threshold, MIN_CLARITY)?;
        if !(MIN_HZ..=MAX_HZ).contains(&pitch.frequency) || !pitch.clarity.is_finite() || pitch.clarity < MIN_CLARITY {
            return None;
        }
        let note = 69.0 + 12.0 * (pitch.frequency / 440.0).log2();
        let nearest = note.round();
        Some(PitchReading { frequency_hz: pitch.frequency, midi_note: nearest as u8,
            cents: (note - nearest) * 100.0, clarity: pitch.clarity.min(1.0) })
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
            let (mut tap, mut monitor) = channel(rate).unwrap();
            let size = tap.window.len();
            for (hz, note, name) in [(110.0, 45, "A2"), (220.0, 57, "A3"), (445.0, 69, "A4")] {
                let signal = tone(rate, size, hz);
                for part in signal.chunks(137) { tap.push(part); }
                let reading = monitor.take().expect("clear synthetic tone");
                assert_eq!(reading.midi_note, note);
                assert_eq!(reading.note_name(), name);
                assert!((reading.frequency_hz / hz - 1.0).abs() < 0.005, "{rate}: {reading:?}");
                let expected = 100.0 * (69.0 + 12.0 * (hz / 440.0).log2() - f64::from(note));
                assert!((reading.cents - expected).abs() < 9.0);
                assert!(reading.clarity >= MIN_CLARITY);
                assert!(monitor.take().is_none());
            }
        }
    }

    #[test]
    fn complete_windows_only_and_silence_dc_or_invalid_input_do_not_hold_previous_note() {
        let (mut tap, mut monitor) = channel(48000).unwrap();
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
    fn slow_observer_drops_whole_windows_and_resumes_with_current_signal() {
        let (mut tap, mut monitor) = channel(8000).unwrap();
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
