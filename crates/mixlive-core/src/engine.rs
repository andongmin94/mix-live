use std::error::Error;
use std::fmt;

use crate::model::linear_gain;
use crate::{ChannelLayout, ConfigError, Scene};

/// Borrowed, planar audio. All inputs must already share one sample rate and time origin.
/// Mono is duplicated to left and right with unity gain (not summed or power-normalized).
#[derive(Clone, Copy, Debug)]
pub enum Input<'a> {
    Mono(&'a [f32]),
    Stereo { left: &'a [f32], right: &'a [f32] },
}

impl<'a> Input<'a> {
    fn channels(self) -> ChannelLayout {
        match self {
            Self::Mono(_) => ChannelLayout::Mono,
            Self::Stereo { .. } => ChannelLayout::Stereo,
        }
    }

    fn planes(self) -> (&'a [f32], &'a [f32]) {
        match self {
            Self::Mono(samples) => (samples, samples),
            Self::Stereo { left, right } => (left, right),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Output<'a> {
    pub left: &'a [f32],
    pub right: &'a [f32],
}

impl Output<'_> {
    /// Linear sample peak, NOT true peak, LUFS, or a latency/quality measurement.
    pub fn sample_peak(self) -> f32 {
        self.left.iter().chain(self.right).fold(0.0_f32, |peak, sample| peak.max(sample.abs()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcessError {
    InvalidFrameCount,
    SourceCountMismatch,
    SendCountMismatch,
    ShortSendGain { send: usize },
    InvalidSendGain { send: usize },
    ChannelMismatch { source: usize },
    ShortInput { source: usize },
    NonFiniteInput { source: usize },
    NonFiniteOutput { mix: usize },
}

impl fmt::Display for ProcessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFrameCount => write!(f, "block size must be in 1..=configured capacity"),
            Self::SourceCountMismatch => write!(f, "input count must match the compiled scene"),
            Self::SendCountMismatch => write!(f, "gain envelope count must match the scene sends"),
            Self::ShortSendGain { send } => write!(f, "send {send}: fewer gains than requested"),
            Self::InvalidSendGain { send } => write!(f, "send {send}: gains must be finite and in 0..=1"),
            Self::ChannelMismatch { source } => write!(f, "source {source}: channel layout mismatch"),
            Self::ShortInput { source } => write!(f, "source {source}: fewer samples than requested"),
            Self::NonFiniteInput { source } => write!(f, "source {source}: non-finite audio samples"),
            Self::NonFiniteOutput { mix } => write!(f, "mix {mix}: audio arithmetic overflow"),
        }
    }
}

impl Error for ProcessError {}

struct CompiledSource {
    channels: ChannelLayout,
    gain: f32,
}

struct CompiledSend {
    index: usize,
    source: usize,
    mix: usize,
    gain: f32,
}

struct MixBuffer {
    gain: f32,
    muted: bool,
    left: Vec<f32>,
    right: Vec<f32>,
}

pub struct Engine {
    sources: Vec<CompiledSource>,
    sends: Vec<CompiledSend>,
    send_count: usize,
    mixes: Vec<MixBuffer>,
    max_frames: usize,
    rendered_frames: usize,
}

impl Engine {
    /// Allocate and validate outside the realtime thread. The topology stays immutable.
    /// Callers may supply sample-wise send attenuation without rebuilding the scene.
    pub fn new(scene: &Scene, max_frames: usize) -> Result<Self, ConfigError> {
        scene.validate()?;
        if max_frames == 0 {
            return Err(ConfigError::ZeroBlockCapacity);
        }
        let sources = scene
            .sources
            .iter()
            .enumerate()
            .map(|(index, source)| {
                let gain = linear_gain(source.gain_db)
                    .ok_or(ConfigError::InvalidGain { section: "source", index })?;
                Ok(CompiledSource {
                    channels: source.channels,
                    gain: if source.muted { 0.0 } else { gain },
                })
            })
            .collect::<Result<Vec<_>, ConfigError>>()?;
        let sends = scene
            .sends
            .iter()
            .enumerate()
            .filter(|(_, send)| send.enabled)
            .map(|(index, send)| {
                let gain = linear_gain(send.gain_db)
                    .ok_or(ConfigError::InvalidGain { section: "send", index })?;
                Ok(CompiledSend { index, source: send.source, mix: send.mix, gain })
            })
            .collect::<Result<Vec<_>, ConfigError>>()?;
        let mixes = scene
            .mixes
            .iter()
            .enumerate()
            .map(|(index, mix)| {
                let gain = linear_gain(mix.gain_db)
                    .ok_or(ConfigError::InvalidGain { section: "mix", index })?;
                Ok(MixBuffer {
                    gain,
                    muted: mix.muted,
                    left: vec![0.0; max_frames],
                    right: vec![0.0; max_frames],
                })
            })
            .collect::<Result<Vec<_>, ConfigError>>()?;
        Ok(Self { sources, sends, send_count: scene.sends.len(), mixes, max_frames, rendered_frames: 0 })
    }

    /// Render synchronized input buffers into independent stereo mixes.
    ///
    /// No allocation, locking, logging, device access, or serialization occurs here. These
    /// statements describe this source, not an independently measured realtime deadline.
    /// On any error, `output()` returns None until a later successful render: a host must then
    /// send silence, never reuse an earlier buffer or bypass to an unprocessed microphone.
    pub fn render(&mut self, frames: usize, inputs: &[Input<'_>]) -> Result<(), ProcessError> {
        self.render_block(frames, inputs, None)
    }

    /// Apply one linear 0..=1 envelope per ORIGINAL Scene::sends entry, including disabled sends.
    /// Envelopes multiply the configured source/send/mix gains; they cannot enable disabled
    /// routes, override mix-minus or unmute a source/mix. The caller owns smoothing and storage.
    /// No allocation or topology mutation occurs. Invalid envelopes hide all outputs, as above.
    pub fn render_with_send_gains(&mut self, frames: usize, inputs: &[Input<'_>],
        gains: &[&[f32]]) -> Result<(), ProcessError> {
        self.render_block(frames, inputs, Some(gains))
    }

    fn render_block(&mut self, frames: usize, inputs: &[Input<'_>],
        gains: Option<&[&[f32]]>) -> Result<(), ProcessError> {
        self.rendered_frames = 0;
        if frames == 0 || frames > self.max_frames {
            return Err(ProcessError::InvalidFrameCount);
        }
        if inputs.len() != self.sources.len() {
            return Err(ProcessError::SourceCountMismatch);
        }
        if let Some(gains) = gains {
            if gains.len() != self.send_count { return Err(ProcessError::SendCountMismatch); }
            for (send, envelope) in gains.iter().enumerate() {
                if envelope.len() < frames { return Err(ProcessError::ShortSendGain { send }); }
                if envelope[..frames].iter().any(|value| !value.is_finite() || !(0.0..=1.0).contains(value)) {
                    return Err(ProcessError::InvalidSendGain { send });
                }
            }
        }
        for (source, (input, spec)) in inputs.iter().zip(&self.sources).enumerate() {
            if input.channels() != spec.channels {
                return Err(ProcessError::ChannelMismatch { source });
            }
            let (left, right) = input.planes();
            if left.len() < frames || right.len() < frames {
                return Err(ProcessError::ShortInput { source });
            }
            if left[..frames].iter().chain(&right[..frames]).any(|sample| !sample.is_finite()) {
                return Err(ProcessError::NonFiniteInput { source });
            }
        }
        for mix in &mut self.mixes {
            mix.left[..frames].fill(0.0);
            mix.right[..frames].fill(0.0);
        }
        for send in &self.sends {
            let source = &self.sources[send.source];
            let mix = &mut self.mixes[send.mix];
            if mix.muted || source.gain == 0.0 {
                continue;
            }
            let gain = source.gain * send.gain;
            let (left, right) = inputs[send.source].planes();
            for (frame, (out_left, out_right)) in mix.left[..frames].iter_mut().zip(&mut mix.right[..frames]).enumerate() {
                let gain = gain * gains.map_or(1.0, |envelopes| envelopes[send.index][frame]);
                *out_left += left[frame] * gain;
                *out_right += right[frame] * gain;
            }
        }
        for (index, mix) in self.mixes.iter_mut().enumerate() {
            for sample in mix.left[..frames].iter_mut().chain(&mut mix.right[..frames]) {
                *sample *= mix.gain;
                if !sample.is_finite() {
                    return Err(ProcessError::NonFiniteOutput { mix: index });
                }
            }
        }
        self.rendered_frames = frames;
        Ok(())
    }

    pub fn output(&self, mix: usize) -> Option<Output<'_>> {
        if self.rendered_frames == 0 {
            return None;
        }
        self.mixes.get(mix).map(|buffer| Output {
            left: &buffer.left[..self.rendered_frames],
            right: &buffer.right[..self.rendered_frames],
        })
    }
}

#[cfg(test)]
mod send_gain_tests {
    use super::*;
    use crate::{Mix, Send, Source};

    fn scene() -> Scene {
        Scene {
            sources: vec![Source { name: "return".into(), channels: ChannelLayout::Stereo, gain_db: -6.0, muted: false }],
            mixes: vec![
                Mix { name: "call".into(), gain_db: 0.0, muted: false, excluded_sources: vec![0] },
                Mix { name: "monitor".into(), gain_db: -3.0, muted: false, excluded_sources: vec![] },
            ],
            // Keep a disabled, excluded send BEFORE the enabled one to verify original indices.
            sends: vec![
                Send { source: 0, mix: 0, gain_db: 0.0, enabled: false },
                Send { source: 0, mix: 1, gain_db: -6.0, enabled: true },
            ],
        }
    }

    #[test]
    fn envelopes_preserve_original_indices_mix_minus_and_configured_gains() {
        let mut spec = scene();
        let mut engine = Engine::new(&spec, 4).unwrap();
        let inputs = [Input::Stereo { left: &[0.8; 4], right: &[-0.4; 4] }];
        let envelope = [1.0, 0.75, 0.25, 0.0];
        engine.render_with_send_gains(4, &inputs, &[&[1.0; 4], &envelope]).unwrap();
        let output = engine.output(1).unwrap();
        let pointer = output.left.as_ptr();
        let fixed = linear_gain(-6.0).unwrap().powi(2) * linear_gain(-3.0).unwrap();
        for (index, attenuation) in envelope.iter().enumerate() {
            assert!((output.left[index] - 0.8 * fixed * attenuation).abs() < 1e-7);
            assert!((output.right[index] + 0.4 * fixed * attenuation).abs() < 1e-7);
        }
        assert_eq!(engine.output(0).unwrap().sample_peak(), 0.0);
        engine.render_with_send_gains(1, &inputs, &[&[1.0], &[0.5]]).unwrap();
        assert_eq!(engine.output(1).unwrap().left.as_ptr(), pointer);
        assert_eq!(engine.output(1).unwrap().left.len(), 1);
        spec.sources[0].muted = true;
        let mut muted = Engine::new(&spec, 4).unwrap();
        muted.render_with_send_gains(4, &inputs, &[&[1.0; 4], &[1.0; 4]]).unwrap();
        assert_eq!(muted.output(1).unwrap().sample_peak(), 0.0);
        spec.sources[0].muted = false;
        spec.mixes[1].muted = true;
        let mut muted = Engine::new(&spec, 4).unwrap();
        muted.render_with_send_gains(4, &inputs, &[&[1.0; 4], &[1.0; 4]]).unwrap();
        assert_eq!(muted.output(1).unwrap().sample_peak(), 0.0);
    }

    #[test]
    fn invalid_envelopes_never_expose_previous_audio() {
        let mut engine = Engine::new(&scene(), 4).unwrap();
        let inputs = [Input::Stereo { left: &[0.5; 4], right: &[0.25; 4] }];
        for invalid in [f32::NAN, f32::INFINITY, -0.1, 1.1] {
            engine.render(4, &inputs).unwrap();
            assert_eq!(engine.render_with_send_gains(4, &inputs, &[&[1.0; 4], &[invalid; 4]]),
                Err(ProcessError::InvalidSendGain { send: 1 }));
            assert!(engine.output(0).is_none() && engine.output(1).is_none());
        }
        assert_eq!(engine.render_with_send_gains(4, &inputs, &[]), Err(ProcessError::SendCountMismatch));
        assert_eq!(engine.render_with_send_gains(4, &inputs, &[&[1.0; 4], &[1.0; 3]]),
            Err(ProcessError::ShortSendGain { send: 1 }));
        assert!(engine.output(1).is_none());
        engine.render_with_send_gains(4, &inputs, &[&[1.0; 4], &[1.0; 4]]).unwrap();
        assert!(engine.output(1).unwrap().sample_peak() > 0.0);
    }
}
