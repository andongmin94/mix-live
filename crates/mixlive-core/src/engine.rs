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
    mixes: Vec<MixBuffer>,
    max_frames: usize,
    rendered_frames: usize,
}

impl Engine {
    /// Allocate and validate outside the realtime thread. One engine is one immutable scene.
    /// Live parameter changes and click-free scene replacement are not implemented here yet.
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
                Ok(CompiledSend { source: send.source, mix: send.mix, gain })
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
        Ok(Self { sources, sends, mixes, max_frames, rendered_frames: 0 })
    }

    /// Render synchronized input buffers into independent stereo mixes.
    ///
    /// No allocation, locking, logging, device access, or serialization occurs here. These
    /// statements describe this source, not an independently measured realtime deadline.
    /// On any error, `output()` returns None until a later successful render: a host must then
    /// send silence, never reuse an earlier buffer or bypass to an unprocessed microphone.
    pub fn render(&mut self, frames: usize, inputs: &[Input<'_>]) -> Result<(), ProcessError> {
        self.rendered_frames = 0;
        if frames == 0 || frames > self.max_frames {
            return Err(ProcessError::InvalidFrameCount);
        }
        if inputs.len() != self.sources.len() {
            return Err(ProcessError::SourceCountMismatch);
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
            for (out, sample) in mix.left[..frames].iter_mut().zip(&left[..frames]) {
                *out += sample * gain;
            }
            for (out, sample) in mix.right[..frames].iter_mut().zip(&right[..frames]) {
                *out += sample * gain;
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
