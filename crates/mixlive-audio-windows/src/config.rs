use clap::{Args, Parser, Subcommand, ValueEnum};
use crate::control::MixSettings;
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
    /// List running application executable paths and unambiguous process-tree roots.
    Apps,
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
        /// Read live mute/gain/status/stop commands from stdin. EOF stops the route.
        #[arg(long)]
        interactive: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum SceneCommand {
    /// Save settings to a NEW JSON file. Does not open or verify audio devices.
    Save {
        file: PathBuf,
        #[command(flatten)]
        route: Box<RouteOptions>,
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
        /// Read live commands; changes apply only to this run and never overwrite the Scene.
        #[arg(long)]
        interactive: bool,
    },
}

/// Three fixed logical destinations, independent of selected physical endpoints.
pub const MAX_MIXES: usize = 3;

/// Explicit connections. Both means monitor + call; All also includes broadcast.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[repr(u8)]
#[serde(rename_all = "kebab-case")]
pub enum AuxDestination { Off = 0, Output = 1, Send = 2, Both = 3,
    Broadcast = 4, OutputBroadcast = 5, SendBroadcast = 6, All = 7 }
impl AuxDestination {
    pub fn includes(self, mix: usize) -> bool {
        mix < MAX_MIXES && self as u8 & (1 << mix) != 0
    }

    pub fn from_enabled(enabled: [bool; MAX_MIXES]) -> Self {
        match enabled {
            [false, false, false] => Self::Off, [true, false, false] => Self::Output,
            [false, true, false] => Self::Send, [true, true, false] => Self::Both,
            [false, false, true] => Self::Broadcast, [true, false, true] => Self::OutputBroadcast,
            [false, true, true] => Self::SendBroadcast, [true, true, true] => Self::All,
        }
    }
}

/// Bound the real-time frame/queue and UI route matrix to eight independent applications.
pub const MAX_APPS: usize = 8;

/// A persistent source identity and its independent initial mix settings. No PID is stored.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppSource {
    pub path: String,
    pub channels: Vec<u16>,
    pub gain_db: f32,
    pub to: AuxDestination,
    pub muted: bool,
    pub output_gain_db: f32,
    pub send_gain_db: f32,
    pub broadcast_gain_db: f32,
}

impl AppSource {
    pub fn new(path: String) -> Self {
        Self { path, channels: vec![1, 2], gain_db: -12.0, to: AuxDestination::Off,
            muted: false, output_gain_db: 0.0, send_gain_db: 0.0, broadcast_gain_db: 0.0 }
    }

    fn validate(&self, has_send: bool, has_broadcast: bool) -> Result<(), String> {
        if self.path.trim().is_empty() { return Err("application executable path must not be blank".into()); }
        InputMap::new(&self.channels, 2)?;
        if self.to.includes(1) && !has_send { return Err("application send destination requires --send".into()); }
        if self.to.includes(2) && !has_broadcast { return Err("application broadcast destination requires --broadcast".into()); }
        for db in [self.gain_db, self.output_gain_db, self.send_gain_db, self.broadcast_gain_db] {
            if !db.is_finite() || !(-96.0..=0.0).contains(&db) {
                return Err("application trim/route gain must be finite and between -96 and 0 dB".into());
            }
        }
        Ok(())
    }
}

fn parse_app(path: &str) -> Result<AppSource, String> {
    let app = AppSource::new(path.to_owned());
    app.validate(true, true)?;
    Ok(app)
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
    /// Add an exact executable path from apps. Repeat for distinct trees; initially disconnected.
    #[arg(long = "app", value_parser = parse_app)]
    pub apps: Vec<AppSource>,
    /// One-based mono channel or stereo pair for the physical AUX endpoint.
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
    /// Broadcast playback endpoint, independently optional.
    #[arg(long)]
    pub broadcast: Option<String>,
    #[arg(long, value_delimiter = ',', default_value = "1,2")]
    pub broadcast_channels: Vec<u16>,
    /// Digital mic trim BEFORE vocal processing (-36..18 dB). Cannot fix input clipping.
    #[arg(long, default_value = "0", allow_hyphen_values = true)]
    pub input_gain_db: f32,
    /// Initial master attenuation on every mix, after output resampling (-96..0 dB).
    #[arg(long, default_value = "-18", allow_hyphen_values = true)]
    pub output_gain_db: f32,
    /// Additional attenuation on the monitor mix only.
    #[arg(long, default_value = "0", allow_hyphen_values = true)]
    pub monitor_gain_db: f32,
    /// Additional attenuation on the call mix.
    #[arg(long, default_value = "0", allow_hyphen_values = true)]
    pub send_gain_db: f32,
    /// Additional attenuation on the broadcast mix.
    #[arg(long, default_value = "0", allow_hyphen_values = true)]
    pub broadcast_gain_db: f32,
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
    /// Explicit initial mixer state in the Scene. Direct CLI runs use the normal defaults.
    #[arg(skip)]
    pub mix: MixSettings,
}

impl RouteOptions {
    pub fn has_aux(&self) -> bool { self.aux_input.is_some() }

    /// Logical indices are stable even when the optional call mix is absent.
    pub fn playback_bindings(&self) -> impl Iterator<Item = (usize, &str, &[u16])> {
        [(Some(self.output.as_str()), self.output_channels.as_slice()),
            (self.send.as_deref(), self.send_channels.as_slice()),
            (self.broadcast.as_deref(), self.broadcast_channels.as_slice())]
            .into_iter().enumerate().filter_map(|(index, (id, channels))| id.map(|id| (index, id, channels)))
    }

    pub fn vocal_config(&self) -> VocalConfig {
        VocalConfig { preset: self.vocal, input_gain_db: self.input_gain_db, strength: self.vocal_strength,
            compressor_threshold_db: self.vocal_threshold_db, deess_threshold_db: self.deess_threshold_db }
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.input.trim().is_empty() || self.output.trim().is_empty() || self.input_channel == 0 {
            return Err("explicit endpoint IDs and a one-based input channel are required".into());
        }
        // Validate inactive saved channel shapes too; preparation checks actual channel counts.
        for channels in [&self.output_channels, &self.send_channels, &self.broadcast_channels] {
            OutputMap::new(channels, u16::MAX)?;
        }
        let mut bindings: Vec<(&str, OutputMap)> = Vec::with_capacity(MAX_MIXES);
        for (_, id, channels) in self.playback_bindings() {
            if id.trim().is_empty() { return Err("playback endpoint ID must not be blank".into()); }
            let map = OutputMap::new(channels, u16::MAX)?;
            if bindings.iter().any(|&(other_id, other_map)| id == other_id && map.overlaps(other_map)) {
                return Err("output mix channels must not overlap on the same endpoint".into());
            }
            bindings.push((id, map));
        }
        let aux = InputMap::new(&self.aux_channels, u16::MAX)?;
        if self.apps.len() > MAX_APPS { return Err(format!("at most {MAX_APPS} application sources are supported")); }
        for (index, app) in self.apps.iter().enumerate() {
            app.validate(self.send.is_some(), self.broadcast.is_some())?;
            if self.apps[..index].iter().any(|other| other.path.eq_ignore_ascii_case(&app.path)) {
                return Err(format!("duplicate application source: {}", app.path));
            }
        }
        if self.has_aux() && self.aux_to.includes(1) && self.send.is_none() {
            return Err("AUX destination send/both requires --send; use --aux-to output for one mix".into());
        }
        if self.has_aux() && self.aux_to.includes(2) && self.broadcast.is_none() {
            return Err("AUX broadcast destination requires --broadcast".into());
        }
        if let Some(id) = &self.aux_input {
            if id.trim().is_empty() { return Err("AUX endpoint ID must not be blank".into()); }
            if id == &self.input && aux.contains(usize::from(self.input_channel - 1)) {
                return Err("microphone and AUX channels must not overlap on the same input endpoint".into());
            }
        }
        if !self.aux_gain_db.is_finite() || !(-96.0..=0.0).contains(&self.aux_gain_db) {
            return Err("AUX gain must be finite and between -96 and 0 dB".into());
        }
        self.vocal_config().validate().map_err(|e| e.to_string())?;
        self.mix.validate()?;
        for db in [self.output_gain_db, self.monitor_gain_db, self.send_gain_db, self.broadcast_gain_db] {
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

/// Map one packed endpoint frame. Preparation guarantees disjoint maps in lane order.
pub fn output_sample(maps: &[OutputMap], channel: usize, frame: &[f32; MAX_MIXES * 2]) -> f32 {
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
        let packed = [0.1, 0.2, -0.3, 0.4, 0.6, -0.5];
        assert_eq!((0..6).map(|channel| output_sample(&maps, channel, &packed)).collect::<Vec<_>>(),
            [0.2, 0.1, 0.4, -0.3, 0.0, 0.0]);
        let all = [maps[0], maps[1], OutputMap::Stereo(5, 4)];
        assert_eq!((0..8).map(|channel| output_sample(&all, channel, &packed)).collect::<Vec<_>>(),
            [0.2, 0.1, 0.4, -0.3, -0.5, 0.6, 0.0, 0.0]);
        let mono = [OutputMap::Mono(0), OutputMap::Mono(2)];
        assert!(!mono[0].overlaps(mono[1]));
        let result: Vec<f32> = (0..4).map(|c| output_sample(&mono, c, &[0.2, 0.4, -0.1, 0.3, 0.0, 0.0])).collect();
        assert!((result[0] - 0.3).abs() < 1e-6 && (result[2] - 0.1).abs() < 1e-6);
        assert_eq!((result[1], result[3]), (0.0, 0.0));
        for (a, b) in [(OutputMap::Mono(1), maps[0]), (maps[0], OutputMap::Stereo(3, 1)), (mono[0], mono[0])] {
            assert!(a.overlaps(b) && b.overlaps(a));
        }
        assert!(OutputMap::new(&[3, 4], 2).is_err());
    }
    #[test]
    fn broadcast_can_exist_without_call_and_shared_channels_must_be_disjoint() {
        let Command::Check(mut route) = Cli::parse_from(["audio", "check", "--input", "mic", "--output", "phones",
            "--broadcast", "phones", "--broadcast-channels", "5,6", "--app", "browser.exe"]).command else { panic!("expected check") };
        route.apps[0].to = AuxDestination::OutputBroadcast;
        assert!(route.validate().is_ok());
        assert_eq!(route.playback_bindings().map(|(mix, _, _)| mix).collect::<Vec<_>>(), [0, 2]);
        route.send = Some("phones".into());
        route.send_channels = vec![3, 4];
        assert!(route.validate().is_ok());
        route.broadcast_channels = vec![4, 6];
        assert!(route.validate().unwrap_err().contains("overlap"));
        route.broadcast_channels = vec![5, 6];
        route.broadcast = None;
        assert!(route.validate().unwrap_err().contains("requires --broadcast"));
        for bits in 0..8 {
            let enabled = std::array::from_fn(|mix| bits & (1 << mix) != 0);
            let destination = AuxDestination::from_enabled(enabled);
            assert_eq!(std::array::from_fn::<_, MAX_MIXES, _>(|mix| destination.includes(mix)), enabled);
        }
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
