use clap::{Args, Parser, Subcommand, ValueEnum};
use mixlive_effects::{Preset, VocalConfig};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
    /// Save, inspect, validate or run a device/vocal scene. Loading alone never starts audio.
    Scene {
        #[command(subcommand)]
        command: SceneCommand,
    },
    /// Mix a processed mic with optional AUX audio. Use headphones at low volume.
    Run {
        #[command(flatten)]
        route: RouteOptions,
        /// Stop automatically after this many seconds. No audio is recorded to disk.
        #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..=3600))]
        seconds: u64,
    },
}

#[derive(Subcommand, Debug)]
pub enum SceneCommand {
    /// Save settings to a NEW JSON file. Does not open or verify audio devices.
    Save {
        file: PathBuf,
        #[command(flatten)]
        route: RouteOptions,
    },
    /// Read and validate saved settings, then print JSON. Does not open audio devices.
    Show { file: PathBuf },
    /// Resolve the saved device IDs and channel configuration without starting streams.
    Check { file: PathBuf },
    /// Recheck the saved devices, then explicitly start this route. Use headphones at low volume.
    Run {
        file: PathBuf,
        #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..=3600))]
        seconds: u64,
    },
}

/// Which existing mix receives AUX. Send-only avoids replaying MR already heard elsewhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum AuxDestination { Output, Send, Both }
impl AuxDestination {
    pub fn includes(self, mix: usize) -> bool {
        match self { Self::Output => mix == 0, Self::Send => mix == 1, Self::Both => mix < 2 }
    }
}

/// Selected capture channels. Mono is duplicated; stereo order is preserved, never downmixed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputMap { pub left: usize, pub right: usize }
impl InputMap {
    pub fn new(channels: &[u16], device_channels: u16) -> Result<Self, String> {
        if channels.iter().any(|&c| c == 0 || c > device_channels) {
            return Err(format!("AUX channels must be in 1..={device_channels}"));
        }
        match channels {
            [one] => Ok(Self { left: usize::from(*one - 1), right: usize::from(*one - 1) }),
            [left, right] if left != right => Ok(Self { left: usize::from(*left - 1), right: usize::from(*right - 1) }),
            _ => Err("AUX requires one channel or two distinct channels".into()),
        }
    }
    pub fn contains(self, channel: usize) -> bool { self.left == channel || self.right == channel }
    /// Caller supplies a complete device frame matching the channel count validated at setup.
    pub fn read<T: Copy>(self, frame: &[T]) -> [T; 2] { [frame[self.left], frame[self.right]] }
}

#[derive(Args, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteOptions {
    /// Exact capture endpoint ID printed by devices, not its display name.
    #[arg(long)]
    pub input: String,
    /// One-based channel within that endpoint's default stream (not an ASIO channel number).
    #[arg(long, default_value_t = 1)]
    pub input_channel: u16,
    /// Optional capture endpoint carrying MR/line/virtual-cable audio, NOT an app name or playback ID.
    #[arg(long)]
    pub aux_input: Option<String>,
    /// One-based mono channel or stereo pair. May share the mic endpoint on other channels.
    #[arg(long, value_delimiter = ',', default_value = "1,2")]
    pub aux_channels: Vec<u16>,
    /// AUX trim before mixing (-96..0 dB); vocal effects never process this source.
    #[arg(long, default_value = "-12", allow_hyphen_values = true)]
    pub aux_gain_db: f32,
    /// Send-only by default to avoid duplicate monitoring. Send/both require --send.
    #[arg(long, value_enum, default_value = "send")]
    pub aux_to: AuxDestination,
    /// Exact playback endpoint ID. Can be headphones or an installed virtual cable's playback end.
    #[arg(long)]
    pub output: String,
    /// One channel, or a stereo pair; all other unassigned device channels stay silent.
    #[arg(long, value_delimiter = ',', default_value = "1,2")]
    pub output_channels: Vec<u16>,
    /// Second mix's playback endpoint. May share output if its channels do not overlap.
    #[arg(long)]
    pub send: Option<String>,
    #[arg(long, value_delimiter = ',', default_value = "1,2")]
    pub send_channels: Vec<u16>,
    /// Digital mic trim BEFORE vocal processing (-36..18 dB). Cannot fix input clipping.
    #[arg(long, default_value = "0", allow_hyphen_values = true)]
    pub input_gain_db: f32,
    /// Final attenuation on BOTH mixes, AFTER vocal processing and AUX summing (-96..0 dB).
    #[arg(long, default_value = "-18", allow_hyphen_values = true)]
    pub output_gain_db: f32,
    /// Additional attenuation on the second mix.
    #[arg(long, default_value = "0", allow_hyphen_values = true)]
    pub send_gain_db: f32,
    /// Vocal starting point: off, natural, warm or bright. No pitch correction or reverb yet.
    #[arg(long, default_value_t = Preset::Off)]
    #[serde(with = "preset_serde")]
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
    /// Target per-endpoint queue depth, not round-trip latency or a hardware buffer size.
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u32).range(10..=100))]
    pub queue_ms: u32,
}

impl RouteOptions {
    pub fn vocal_config(&self) -> VocalConfig {
        VocalConfig { preset: self.vocal, input_gain_db: self.input_gain_db, strength: self.vocal_strength,
            compressor_threshold_db: self.vocal_threshold_db, deess_threshold_db: self.deess_threshold_db }
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.input.trim().is_empty() || self.output.trim().is_empty() || self.input_channel == 0 {
            return Err("explicit endpoint IDs and a one-based input channel are required".into());
        }
        if self.send.as_ref().is_some_and(|id| id.trim().is_empty()) {
            return Err("send endpoint ID must not be blank".into());
        }
        // File-based settings do not pass through clap. Check channel shape here;
        // Prepared::new checks the actual device's channel count before opening streams.
        let output = OutputMap::new(&self.output_channels, u16::MAX)?;
        let send = OutputMap::new(&self.send_channels, u16::MAX)?;
        if self.send.as_deref() == Some(self.output.as_str()) && output.overlaps(send) {
            return Err("output and send channels must not overlap on the same endpoint".into());
        }
        let aux = InputMap::new(&self.aux_channels, u16::MAX)?;
        if let Some(id) = &self.aux_input {
            if id.trim().is_empty() { return Err("AUX endpoint ID must not be blank".into()); }
            if self.aux_to.includes(1) && self.send.is_none() {
                return Err("AUX destination send/both requires --send; use --aux-to output for one mix".into());
            }
            if id == &self.input && aux.contains(usize::from(self.input_channel - 1)) {
                return Err("microphone and AUX channels must not overlap on the same input endpoint".into());
            }
        }
        if !self.aux_gain_db.is_finite() || !(-96.0..=0.0).contains(&self.aux_gain_db) {
            return Err("AUX gain must be finite and between -96 and 0 dB".into());
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

// Persist the existing preset names without making the DSP crate depend on file formats.
mod preset_serde {
    use mixlive_effects::Preset;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &Preset, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(value)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Preset, D::Error> {
        String::deserialize(deserializer)?.parse().map_err(serde::de::Error::custom)
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
    pub fn overlaps(self, other: Self) -> bool {
        let channels = |map| match map { Self::Mono(one) => [one, one], Self::Stereo(left, right) => [left, right] };
        channels(self).iter().any(|channel| channels(other).contains(channel))
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

/// Map one packed endpoint frame. Preparation guarantees one/two disjoint maps in lane order.
pub fn output_sample(maps: &[OutputMap], channel: usize, frame: &[f32; 4]) -> f32 {
    maps.iter().zip(frame.chunks_exact(2))
        .map(|(map, stereo)| map.sample(channel, [stereo[0], stereo[1]])).sum()
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
        assert!(options.validate().unwrap_err().contains("overlap"));
        options.send_channels = vec![3, 4];
        assert!(options.validate().is_ok());
        options.send_channels = vec![2, 3];
        assert!(options.validate().unwrap_err().contains("overlap"));
        options.send = Some("other-endpoint".into());
        assert!(options.validate().is_ok());
    }
    #[test]
    fn shared_endpoint_maps_preserve_stereo_mono_and_unassigned_channels() {
        let maps = [OutputMap::new(&[2, 1], 6).unwrap(), OutputMap::new(&[4, 3], 6).unwrap()];
        assert!(!maps[0].overlaps(maps[1]));
        let packed = [0.1, 0.2, -0.3, 0.4];
        assert_eq!((0..6).map(|channel| output_sample(&maps, channel, &packed)).collect::<Vec<_>>(),
            [0.2, 0.1, 0.4, -0.3, 0.0, 0.0]);
        let mono = [OutputMap::Mono(0), OutputMap::Mono(2)];
        assert!(!mono[0].overlaps(mono[1]));
        let result: Vec<f32> = (0..4).map(|c| output_sample(&mono, c, &[0.2, 0.4, -0.1, 0.3])).collect();
        assert!((result[0] - 0.3).abs() < 1e-6 && (result[2] - 0.1).abs() < 1e-6);
        assert_eq!((result[1], result[3]), (0.0, 0.0));
        for (a, b) in [(OutputMap::Mono(1), maps[0]), (maps[0], OutputMap::Stereo(3, 1)), (mono[0], mono[0])] {
            assert!(a.overlaps(b) && b.overlaps(a));
        }
        assert!(OutputMap::new(&[3, 4], 2).is_err());
    }
    #[test]
    fn aux_selection_is_explicit_and_shared_capture_cannot_duplicate_the_mic() {
        let cli = Cli::try_parse_from(["mixlive-audio", "check", "--input", "mic", "--output", "phones", "--aux-input", "mr"]).unwrap();
        let Command::Check(mut options) = cli.command else { panic!("expected check") };
        assert_eq!(options.aux_to, AuxDestination::Send);
        assert!(options.validate().unwrap_err().contains("requires --send"));
        options.aux_to = AuxDestination::Output;
        assert!(options.validate().is_ok());
        options.aux_to = AuxDestination::Both;
        options.send = Some("call".into());
        assert!(options.validate().is_ok());
        options.aux_input = Some("mic".into());
        assert!(options.validate().unwrap_err().contains("overlap"));
        options.aux_channels = vec![4, 3];
        assert!(options.validate().is_ok());
        let stereo = InputMap::new(&options.aux_channels, 4).unwrap();
        assert_eq!((stereo.left, stereo.right), (3, 2));
        assert!(!stereo.contains(0));
        assert_eq!(stereo.read(&[0.1, 0.2, 0.3, 0.4]), [0.4, 0.3]);
        assert_eq!(InputMap::new(&[2], 2).unwrap().read(&[0.1, 0.2]), [0.2, 0.2]);
        assert_eq!(InputMap::new(&[2], 2).unwrap(), InputMap { left: 1, right: 1 });
        assert!(InputMap::new(&[3, 4], 2).is_err());
        for channels in [vec![], vec![0], vec![1, 1], vec![1, 2, 3]] {
            options.aux_channels = channels;
            assert!(options.validate().is_err());
        }
        options.aux_channels = vec![3, 4];
        options.aux_gain_db = f32::NAN;
        assert!(options.validate().is_err());
    }

}
