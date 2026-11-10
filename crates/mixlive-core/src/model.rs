use std::error::Error;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelLayout {
    Mono,
    Stereo,
}

#[derive(Clone, Debug)]
pub struct Source {
    pub name: String,
    pub channels: ChannelLayout,
    pub gain_db: f32,
    pub muted: bool,
}

#[derive(Clone, Debug)]
pub struct Mix {
    pub name: String,
    pub gain_db: f32,
    pub muted: bool,
    /// Source indices forbidden in this mix, e.g. Discord return in Discord send.
    pub excluded_sources: Vec<usize>,
}

#[derive(Clone, Copy, Debug)]
pub struct Send {
    pub source: usize,
    pub mix: usize,
    pub gain_db: f32,
    pub enabled: bool,
}

/// A configuration snapshot prepared outside the audio callback.
///
/// Indices refer only to this snapshot. They are not persistent device or scene identities.
/// Durable scene serialization belongs to the future application layer, not this render plan.
#[derive(Clone, Debug)]
pub struct Scene {
    pub sources: Vec<Source>,
    pub mixes: Vec<Mix>,
    pub sends: Vec<Send>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigError {
    EmptySources,
    EmptyMixes,
    ZeroBlockCapacity,
    InvalidGain { section: &'static str, index: usize },
    InvalidSource { index: usize },
    InvalidMix { index: usize },
    DuplicateSend { source: usize, mix: usize },
    ExcludedSource { source: usize, mix: usize },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptySources => write!(f, "a scene needs at least one source"),
            Self::EmptyMixes => write!(f, "a scene needs at least one mix"),
            Self::ZeroBlockCapacity => write!(f, "maximum block size must be greater than zero"),
            Self::InvalidGain { section, index } => {
                write!(f, "{section} {index}: gain must be finite and between -96 and +24 dB")
            }
            Self::InvalidSource { index } => write!(f, "source index {index} does not exist"),
            Self::InvalidMix { index } => write!(f, "mix index {index} does not exist"),
            Self::DuplicateSend { source, mix } => {
                write!(f, "source {source} has more than one send to mix {mix}")
            }
            Self::ExcludedSource { source, mix } => {
                write!(f, "mix-minus: source {source} is excluded from mix {mix}")
            }
        }
    }
}

impl Error for ConfigError {}

pub(crate) fn linear_gain(db: f32) -> Option<f32> {
    if db.is_finite() && (-96.0..=24.0).contains(&db) {
        Some(10.0_f32.powf(db / 20.0))
    } else {
        None
    }
}

impl Scene {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.sources.is_empty() {
            return Err(ConfigError::EmptySources);
        }
        if self.mixes.is_empty() {
            return Err(ConfigError::EmptyMixes);
        }
        for (index, source) in self.sources.iter().enumerate() {
            if linear_gain(source.gain_db).is_none() {
                return Err(ConfigError::InvalidGain { section: "source", index });
            }
        }
        for (index, mix) in self.mixes.iter().enumerate() {
            if linear_gain(mix.gain_db).is_none() {
                return Err(ConfigError::InvalidGain { section: "mix", index });
            }
            for &source in &mix.excluded_sources {
                if source >= self.sources.len() {
                    return Err(ConfigError::InvalidSource { index: source });
                }
            }
        }
        for (index, send) in self.sends.iter().enumerate() {
            if send.source >= self.sources.len() {
                return Err(ConfigError::InvalidSource { index: send.source });
            }
            let mix = self
                .mixes
                .get(send.mix)
                .ok_or(ConfigError::InvalidMix { index: send.mix })?;
            if linear_gain(send.gain_db).is_none() {
                return Err(ConfigError::InvalidGain { section: "send", index });
            }
            if self.sends[..index]
                .iter()
                .any(|other| other.source == send.source && other.mix == send.mix)
            {
                return Err(ConfigError::DuplicateSend { source: send.source, mix: send.mix });
            }
            if send.enabled && mix.excluded_sources.contains(&send.source) {
                return Err(ConfigError::ExcludedSource { source: send.source, mix: send.mix });
            }
        }
        Ok(())
    }
}
