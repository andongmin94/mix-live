//! Device-independent, in-memory audio routing. This crate opens no devices.
//!
//! Current scope: mono/stereo sources, per-source and per-send gains, independent stereo mixes,
//! mix-minus exclusions, mute, sample-peak metering and prepared integer-sample send delays.
//! No effects or resampling; device/plugin latency policy belongs to the host.
//! Render calls use buffers allocated before processing.
//!
//! Float headroom is preserved: outputs are NOT clipped or limited. A device adapter must not
//! present this foundation as a finished live sound processor.

mod engine;
mod model;

pub use engine::{Engine, Input, Output, ProcessError, SendDelays};
pub use model::{ChannelLayout, ConfigError, Mix, Scene, Send, Source};

#[cfg(test)]
mod tests;
