//! Device-free plugin preflight. This module never joins the live microphone callback.
//! Native plugins are inspected in a disposable process, not a security sandbox.

use clap::{Args, Parser, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fs::File, io::{Read, Write}, path::{Path, PathBuf}, process::{Command as Process, Stdio}, time::Duration};
use wait_timeout::ChildExt;

#[cfg(windows)]
pub mod host;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const MAX_REPORT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_STATE_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_STATE_BYTES: usize = 16 * 1024 * 1024;
pub const BLOCK_FRAMES: usize = 256;
pub const TEST_BLOCKS: usize = 16;

#[derive(Debug, Parser)]
#[command(version, about = "MixLive VST3 preflight: inspect and test plugins without opening audio devices")]
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
    #[command(name = "__probe", hide = true)]
    Probe {
        #[command(flatten)]
        options: InspectArgs,
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
        // A decimal JSON byte array is larger than its opaque payload; never save a file
        // which the same reader would reject at its on-disk limit.
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

/// Read-only parent: a crash/abort in plugin discovery does not load code into this process.
/// A deadline ends one attempt; we do not retry the same native binary automatically.
pub fn inspect(executable: &Path, options: &InspectArgs) -> Result<Value> {
    if !options.plugin.exists() { return Err(format!("plugin not found: {}", options.plugin.display()).into()); }
    if let Some(path) = &options.state_in {
        let state = StateFile::read(path)?;
        if options.class_id.as_ref().is_some_and(|id| !id.eq_ignore_ascii_case(&state.class_id)) {
            return Err("requested class ID differs from the state file".into());
        }
    }
    if options.state_out.as_ref().is_some_and(|path| path.exists()) {
        return Err("state output already exists; choose a new filename".into());
    }
    let directory = tempfile::tempdir()?;
    let report = directory.path().join("report.json");
    let mut command = Process::new(executable);
    command.arg("__probe").arg(&options.plugin)
        .arg("--report").arg(&report)
        .arg("--sample-rate").arg(options.sample_rate.to_string())
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::inherit());
    if let Some(id) = &options.class_id { command.arg("--class-id").arg(id); }
    if let Some(path) = &options.state_in { command.arg("--state-in").arg(path); }
    if let Some(path) = &options.state_out { command.arg("--state-out").arg(path); }
    for parameter in &options.parameters {
        command.arg("--set").arg(format!("{}={}", parameter.id, parameter.normalized));
    }
    let mut child = command.spawn()?;
    let status = match child.wait_timeout(Duration::from_secs(options.timeout_seconds)) {
        Ok(Some(status)) => status,
        Ok(None) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("VST3 probe timed out after {}s; plugin was not admitted to live audio", options.timeout_seconds).into());
        }
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error.into());
        }
    };
    if !status.success() {
        return Err(format!("VST3 probe failed ({status}); no live path was changed").into());
    }
    let report: Value = serde_json::from_slice(&read_bounded(&report, MAX_REPORT_BYTES)?)?;
    if report["schema"] != 1 || report["live_audio_connected"] != false {
        return Err("invalid VST3 probe report".into());
    }
    Ok(report)
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
