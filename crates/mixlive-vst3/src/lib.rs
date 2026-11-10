//! Device-free plugin tools. This module never joins the live microphone callback.
//! Native plugins run in a disposable process, not a security sandbox.

use clap::{Args, Parser, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashSet, fs::File, io::{Read, Write}, path::{Path, PathBuf}, process::{Command as Process, ExitStatus, Stdio}, time::Duration};
use wait_timeout::ChildExt;

#[cfg(windows)]
pub mod host;
pub mod editor;
pub mod render;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const MAX_REPORT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_STATE_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_STATE_BYTES: usize = 16 * 1024 * 1024;
pub const BLOCK_FRAMES: usize = 256;
pub const TEST_BLOCKS: usize = 16;

#[derive(Debug, Parser)]
#[command(version, about = "MixLive VST3 tools: inspect, edit and process WAV files without opening audio devices")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// List installed bundle paths without loading their native code.
    List {
        /// Additional VST3 folder; may be repeated.
        #[arg(long)]
        directory: Vec<PathBuf>,
    },
    /// Inspect one bundle, process synthetic buffers and optionally load/save its state.
    Inspect(InspectArgs),
    /// Open the plugin's own editor. No audio preview. Close the window to finish.
    Edit(editor::EditArgs),
    /// Process a mono/stereo WAV through one stereo effect. Never overwrites an existing file.
    Render(render::RenderArgs),
    #[command(name = "__probe", hide = true)]
    Probe {
        #[command(flatten)]
        options: InspectArgs,
        #[arg(long)]
        report: PathBuf,
    },
    #[command(name = "__edit", hide = true)]
    EditChild {
        #[command(flatten)]
        options: editor::EditArgs,
        #[arg(long)]
        report: PathBuf,
    },
    #[command(name = "__render", hide = true)]
    RenderChild {
        #[command(flatten)]
        options: render::RenderArgs,
        #[arg(long)]
        report: PathBuf,
    },
}

#[derive(Debug, Clone, Args)]
pub struct InspectArgs {
    pub plugin: PathBuf,
    /// A particular audio component's 32-digit class ID, not a display name.
    #[arg(long, value_parser = parse_class_id)]
    pub class_id: Option<String>,
    #[arg(long, default_value_t = 48_000, value_parser = clap::value_parser!(u32).range(8_000..=192_000))]
    pub sample_rate: u32,
    /// Normalized VST3 parameter ID=value (0..1), not a value in dB or Hz.
    #[arg(long = "set", value_parser = parse_parameter)]
    pub parameters: Vec<ParameterChange>,
    #[arg(long)]
    pub state_in: Option<PathBuf>,
    /// Create a new MixLive VST3 state file. Existing files are never overwritten.
    #[arg(long)]
    pub state_out: Option<PathBuf>,
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..=120))]
    pub timeout_seconds: u64,
}

impl InspectArgs {
    pub fn validate(&self) -> Result<()> {
        if !self.plugin.exists() { return Err(format!("plugin not found: {}", self.plugin.display()).into()); }
        if !(8_000..=192_000).contains(&self.sample_rate) || !(1..=120).contains(&self.timeout_seconds) {
            return Err("sample rate must be 8000..192000 Hz and timeout 1..120 seconds".into());
        }
        if let Some(id) = &self.class_id { parse_class_id(id)?; }
        let mut ids = HashSet::new();
        for change in &self.parameters {
            if !change.normalized.is_finite() || !(0.0..=1.0).contains(&change.normalized) {
                return Err("normalized parameter value must be finite and in 0..1".into());
            }
            if !ids.insert(change.id) { return Err(format!("duplicate VST3 parameter {}", change.id).into()); }
        }
        if let Some(path) = &self.state_in {
            let state = StateFile::read(path)?;
            if self.class_id.as_ref().is_some_and(|id| !id.eq_ignore_ascii_case(&state.class_id)) {
                return Err("requested class ID differs from the state file".into());
            }
        }
        if self.state_out.as_ref().is_some_and(|path| path.exists()) {
            return Err("state output already exists; choose a new filename".into());
        }
        Ok(())
    }

    pub(crate) fn append_plugin_args(&self, command: &mut Process) {
        command.arg(&self.plugin);
        if let Some(id) = &self.class_id { command.arg("--class-id").arg(id); }
        if let Some(path) = &self.state_in { command.arg("--state-in").arg(path); }
        for parameter in &self.parameters {
            command.arg("--set").arg(format!("{}={}", parameter.id, parameter.normalized));
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParameterChange {
    pub id: u32,
    pub normalized: f64,
}

pub fn parse_class_id(value: &str) -> std::result::Result<String, String> {
    if value.len() != 32 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("class ID must contain exactly 32 hexadecimal digits".into());
    }
    Ok(value.to_ascii_uppercase())
}

fn parse_parameter(value: &str) -> std::result::Result<ParameterChange, String> {
    let (id, value) = value.split_once('=').ok_or("use ID=normalized-value")?;
    let id = id.parse::<u32>().map_err(|_| "parameter ID must be an unsigned integer")?;
    let normalized = value.parse::<f64>().map_err(|_| "parameter value must be a number")?;
    if !normalized.is_finite() || !(0.0..=1.0).contains(&normalized) {
        return Err("normalized parameter value must be finite and in 0..1".into());
    }
    Ok(ParameterChange { id, normalized })
}

/// The opaque host payload contains the component and controller states, not just slider values.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateFile {
    pub schema: u32,
    pub class_id: String,
    pub state: Vec<u8>,
}

impl StateFile {
    pub fn validate(&self) -> Result<()> {
        if self.schema != 1 { return Err("unsupported MixLive VST3 state schema".into()); }
        parse_class_id(&self.class_id)?;
        if self.state.len() > MAX_STATE_BYTES { return Err("plugin state exceeds 16 MiB".into()); }
        Ok(())
    }
    pub fn read(path: &Path) -> Result<Self> {
        let state: Self = serde_json::from_slice(&read_bounded(path, MAX_STATE_FILE_BYTES)?)?;
        state.validate()?;
        Ok(state)
    }
    pub fn save_new(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let bytes = serde_json::to_vec(self)?;
        if bytes.len() as u64 > MAX_STATE_FILE_BYTES { return Err("serialized state exceeds 64 MiB".into()); }
        write_new(path, &bytes)
    }
}

fn read_bounded(path: &Path, max: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max { return Err(format!("file exceeds {max} bytes: {}", path.display()).into()); }
    Ok(bytes)
}

/// Publish a complete file without a partial save or a racy exists-then-overwrite check.
pub fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file_mut().sync_all()?;
    temporary.persist_noclobber(path)?;
    Ok(())
}

/// One child attempt for inspection or file rendering. No live path is affected or retried.
pub(crate) fn finish_child(mut command: Process, report: &Path, timeout_seconds: u64) -> Result<Value> {
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::inherit());
    let mut child = command.spawn()?;
    let status = match child.wait_timeout(Duration::from_secs(timeout_seconds)) {
        Ok(Some(status)) => status,
        Ok(None) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("VST3 worker timed out after {timeout_seconds}s; no live path was changed").into());
        }
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error.into());
        }
    };
    read_child_report(status, report)
}

/// Shared by deadline-bound probes and interactive editor sessions.
pub(crate) fn read_child_report(status: ExitStatus, report: &Path) -> Result<Value> {
    if !status.success() { return Err(format!("VST3 worker failed ({status}); no live path was changed").into()); }
    let report: Value = serde_json::from_slice(&read_bounded(report, MAX_REPORT_BYTES)?)?;
    if report["schema"] != 1 || report["live_audio_connected"] != false {
        return Err("invalid VST3 worker report".into());
    }
    Ok(report)
}

/// A crash/abort in plugin discovery does not load code into this process.
pub fn inspect(executable: &Path, options: &InspectArgs) -> Result<Value> {
    options.validate()?;
    let directory = tempfile::tempdir()?;
    let report = directory.path().join("report.json");
    let mut command = Process::new(executable);
    command.arg("__probe");
    options.append_plugin_args(&mut command);
    command.arg("--report").arg(&report).arg("--sample-rate").arg(options.sample_rate.to_string());
    if let Some(path) = &options.state_out { command.arg("--state-out").arg(path); }
    finish_child(command, &report, options.timeout_seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_rejects_ambiguous_classes_and_invalid_normalized_values() {
        assert_eq!(parse_class_id("abcdef0123456789abcdef0123456789").unwrap(), "ABCDEF0123456789ABCDEF0123456789");
        for id in ["vst3:1234", "1234", "🦀", "ZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ"] { assert!(parse_class_id(id).is_err()); }
        assert_eq!(parse_parameter("17=0.25").unwrap(), ParameterChange { id: 17, normalized: 0.25 });
        for value in ["x=0.5", "1=NaN", "1=inf", "1=-0.1", "1=1.1", "1"] { assert!(parse_parameter(value).is_err()); }
        assert!(Cli::try_parse_from(["plugins", "inspect", "x.vst3", "--sample-rate", "0"]).is_err());
        assert!(Cli::try_parse_from(["plugins", "inspect", "x.vst3", "--timeout-seconds", "0"]).is_err());
    }

    #[test]
    fn duplicate_parameter_ids_are_rejected_before_native_loading() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut options = InspectArgs { plugin: file.path().into(), class_id: None, sample_rate: 48_000,
            parameters: vec![ParameterChange { id: 1, normalized: 0.25 }; 2],
            state_in: None, state_out: None, timeout_seconds: 30 };
        assert!(options.validate().unwrap_err().to_string().contains("duplicate"));
        options.parameters.pop();
        assert!(options.validate().is_ok());
        options.parameters[0].normalized = f64::NAN;
        assert!(options.validate().is_err());
    }

    #[test]
    fn state_roundtrip_is_versioned_and_never_overwrites() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vocal.json");
        let state = StateFile { schema: 1, class_id: "0123456789ABCDEF0123456789ABCDEF".into(), state: vec![1, 2, 3] };
        state.save_new(&path).unwrap();
        assert_eq!(StateFile::read(&path).unwrap().state, vec![1, 2, 3]);
        assert!(StateFile { state: vec![9], ..state }.save_new(&path).is_err());
        assert_eq!(StateFile::read(&path).unwrap().state, vec![1, 2, 3]);
        assert!(serde_json::from_str::<StateFile>(r#"{"schema":1,"class_id":"x","state":[],"unexpected":true}"#).is_err());
        assert!(StateFile { schema: 2, class_id: "0123456789ABCDEF0123456789ABCDEF".into(), state: vec![] }.validate().is_err());
        assert!(read_bounded(&path, 2).is_err());
    }
}
