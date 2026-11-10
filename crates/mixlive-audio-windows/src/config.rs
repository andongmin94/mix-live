use clap::{Args, Parser, Subcommand};
use mixlive_effects::{Preset, VocalConfig};

#[derive(Parser, Debug)]
#[command(name = "mixlive-audio", version, about = "MixLive development audio path (WASAPI, not a finished broadcast app)")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// List endpoint IDs and their default channel/rate/format configuration. Opens no streams.
    Devices,
    /// Validate devices, channels and vocal settings. Does not capture or play sound.
    Check(RouteOptions),
    /// Capture a mic, optionally process its voice, and route it. Use headphones at low volume.
    Run {
        #[command(flatten)]
        route: RouteOptions,
        /// Stop automatically after this many seconds. No audio is recorded to disk.
        #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..=3600))]
        seconds: u64,
    },
}

#[derive(Args, Clone, Debug)]
pub struct RouteOptions {
    /// Exact capture endpoint ID printed by devices, not its display name.
    #[arg(long)]
    pub input: String,
    /// One-based channel within that endpoint's default stream (not an ASIO channel number).
    #[arg(long, default_value_t = 1)]
    pub input_channel: u16,
    /// Exact playback endpoint ID. Can be headphones or an installed virtual cable's playback end.
    #[arg(long)]
    pub output: String,
    /// One channel, or a stereo pair; all other device channels stay silent.
    #[arg(long, value_delimiter = ',', default_value = "1,2")]
    pub output_channels: Vec<u16>,
    /// Optional second, distinct playback endpoint (for example an installed virtual cable).
    #[arg(long)]
    pub send: Option<String>,
    #[arg(long, value_delimiter = ',', default_value = "1,2")]
    pub send_channels: Vec<u16>,
    /// Digital mic trim BEFORE vocal processing (-36..18 dB). Cannot fix input clipping.
    #[arg(long, default_value = "0", allow_hyphen_values = true)]
    pub input_gain_db: f32,
    /// Final attenuation on BOTH mixes, AFTER vocal processing (-96..0 dB).
    #[arg(long, default_value = "-18", allow_hyphen_values = true)]
    pub output_gain_db: f32,
    /// Additional attenuation on the second mix.
    #[arg(long, default_value = "0", allow_hyphen_values = true)]
    pub send_gain_db: f32,
    /// Vocal starting point: off, natural, warm or bright. No pitch correction or reverb yet.
    #[arg(long, default_value_t = Preset::Off)]
    pub vocal: Preset,
    /// Dry/processed blend (0..1), not loudness matching. Fixed for this run.
    #[arg(long, default_value = "1")]
    pub vocal_strength: f32,
    /// RMS compressor threshold after input trim (-60..0 dBFS). No automatic makeup gain.
    #[arg(long, default_value = "-24", allow_hyphen_values = true)]
    pub vocal_threshold_db: f32,
    /// High-band de-esser threshold (-60..0 dBFS).
    #[arg(long, default_value = "-30", allow_hyphen_values = true)]
    pub deess_threshold_db: f32,
    /// Target per-output queue depth, not round-trip latency or a hardware buffer size.
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u32).range(10..=100))]
    pub queue_ms: u32,
}

impl RouteOptions {
    pub fn vocal_config(&self) -> VocalConfig {
        VocalConfig { preset: self.vocal, input_gain_db: self.input_gain_db, strength: self.vocal_strength,
            compressor_threshold_db: self.vocal_threshold_db, deess_threshold_db: self.deess_threshold_db }
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.input.is_empty() || self.output.is_empty() || self.input_channel == 0 {
            return Err("explicit endpoint IDs and a one-based input channel are required".into());
        }
        if self.send.as_deref() == Some(self.output.as_str()) {
            return Err("output and send must be distinct endpoints; two streams on one endpoint are not supported yet".into());
        }
        self.vocal_config().validate().map_err(|e| e.to_string())?;
        for db in [self.output_gain_db, self.send_gain_db] {
            if !db.is_finite() || !(-96.0..=0.0).contains(&db) {
                return Err("output/send gain must be finite and between -96 and 0 dB".into());
            }
        }
        if !(10..=100).contains(&self.queue_ms) { return Err("queue-ms must be in 10..=100".into()); }
        Ok(())
    }
}

/// Zero-based physical channel mapping, validated before any stream starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputMap { Mono(usize), Stereo(usize, usize) }
impl OutputMap {
    pub fn new(channels: &[u16], device_channels: u16) -> Result<Self, String> {
        if channels.iter().any(|&c| c == 0 || c > device_channels) {
            return Err(format!("output channels must be in 1..={device_channels}"));
        }
        match channels {
            [one] => Ok(Self::Mono(usize::from(*one - 1))),
            [left, right] if left != right => Ok(Self::Stereo(usize::from(*left - 1), usize::from(*right - 1))),
            _ => Err("choose one channel or two distinct channels, e.g. 1 or 1,2".into()),
        }
    }
    pub fn sample(self, channel: usize, frame: [f32; 2]) -> f32 {
        match self {
            Self::Mono(index) if index == channel => (frame[0] + frame[1]) * 0.5,
            Self::Stereo(left, _) if left == channel => frame[0],
            Self::Stereo(_, right) if right == channel => frame[1],
            _ => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn channels_are_one_based_and_unused_outputs_are_silent() {
        let map = OutputMap::new(&[3, 4], 4).unwrap();
        assert_eq!((0..4).map(|c| map.sample(c, [0.2, 0.4])).collect::<Vec<_>>(), [0.0, 0.0, 0.2, 0.4]);
        assert_eq!(OutputMap::new(&[1], 1).unwrap().sample(0, [0.2, 0.4]), 0.3);
        for channels in [vec![], vec![0], vec![5], vec![1, 1], vec![1, 2, 3]] { assert!(OutputMap::new(&channels, 4).is_err()); }
    }
    #[test]
    fn cli_requires_explicit_endpoints_and_valid_vocal_settings() {
        assert!(Cli::try_parse_from(["mixlive-audio", "run"]).is_err());
        assert!(Cli::try_parse_from(["mixlive-audio", "check", "--input", "mic", "--output", "phones", "--gain-db", "-18"]).is_err());
        assert!(Cli::try_parse_from(["mixlive-audio", "check", "--input", "mic", "--output", "phones", "--vocal", "magic"]).is_err());
        let cli = Cli::try_parse_from(["mixlive-audio", "check", "--input", "mic", "--output", "phones", "--vocal", "warm", "--input-gain-db", "6"]).unwrap();
        let Command::Check(mut options) = cli.command else { panic!("expected check") };
        assert!(options.validate().is_ok());
        assert_eq!(options.output_gain_db, -18.0);
        assert_eq!(options.vocal, Preset::Warm);
        options.vocal_strength = f32::NAN;
        assert!(options.validate().is_err());
        options.vocal_strength = 1.0;
        options.output_gain_db = 1.0;
        assert!(options.validate().is_err());
        options.output_gain_db = -18.0;
        options.send = Some(options.output.clone());
        assert!(options.validate().is_err());
    }
}
