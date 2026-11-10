//! Device-independent, in-memory audio routing. This crate opens no devices.
//!
//! Current scope: mono/stereo sources, per-source and per-send gains, independent stereo mixes,
//! mix-minus exclusions, mute, and sample-peak metering. No effects, resampling or latency
//! compensation are implemented yet. Render calls use buffers allocated at construction.
//!
//! Float headroom is preserved: outputs are NOT clipped or limited. A device adapter must not
//! present this foundation as a finished live sound processor.

mod engine;
mod model;

pub use engine::{Engine, Input, Output, ProcessError};
pub use model::{ChannelLayout, ConfigError, Mix, Scene, Send, Source};

#[cfg(test)]
mod tests;
