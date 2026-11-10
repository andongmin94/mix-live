//! Explicit file processing for listening tests; never captures or plays the user's microphone.
use crate::{InspectArgs, ParameterChange, Result, finish_child, parse_class_id, parse_parameter};
use clap::Args;
use hound::{SampleFormat, WavReader, WavSpec};
use serde_json::Value;
use std::{fs::File, io::BufReader, path::{Path, PathBuf}, process::Command};

#[derive(Debug, Clone, Args)]
pub struct RenderArgs {
    pub plugin: PathBuf,
    #[arg(long)]
    pub input: PathBuf,
    /// A NEW stereo float32 WAV. No overwrite, normalization or latency compensation.
    #[arg(long)]
    pub output: PathBuf,
    #[arg(long, value_parser = parse_class_id)]
    pub class_id: Option<String>,
    #[arg(long = "set", value_parser = parse_parameter)]
    pub parameters: Vec<ParameterChange>,
    #[arg(long)]
    pub state_in: Option<PathBuf>,
    /// Append this much silence to the input to render an effect's tail (0..30000 ms).
    #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u32).range(0..=30_000))]
    pub tail_ms: u32,
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..=120))]
    pub timeout_seconds: u64,
}

impl RenderArgs {
    fn plugin_options(&self, sample_rate: u32) -> InspectArgs {
        InspectArgs { plugin: self.plugin.clone(), class_id: self.class_id.clone(), sample_rate,
            parameters: self.parameters.clone(), state_in: self.state_in.clone(), state_out: None,
            timeout_seconds: self.timeout_seconds }
    }
}

/// Keep the staged WAV private until the worker (including native teardown) exits successfully.
pub fn render_file(executable: &Path, options: &RenderArgs) -> Result<Value> {
    let reader = open_input(&options.input)?;
    let expected_frames = output_frames(reader.spec(), reader.duration(), options.tail_ms)?;
    let plugin_options = options.plugin_options(reader.spec().sample_rate);
    plugin_options.validate()?;
    drop(reader);
    if options.output.exists() { return Err("WAV output already exists; choose a new filename".into()); }
    let parent = options.output.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let directory = tempfile::tempdir_in(parent)?;
    let staged = tempfile::TempPath::try_from_path(directory.path().join("processed.wav"))?;
    let report_path = directory.path().join("report.json");
    let mut command = Command::new(executable);
    command.arg("__render");
    plugin_options.append_plugin_args(&mut command);
    command.arg("--input").arg(&options.input).arg("--output").arg(&staged)
        .arg("--tail-ms").arg(options.tail_ms.to_string()).arg("--report").arg(&report_path);
    let mut report = finish_child(command, &report_path, options.timeout_seconds)?;
    if report["operation"] != "render_wav" || report["output_frames"].as_u64() != Some(expected_frames) {
        return Err("invalid WAV render report; output was not published".into());
    }
    staged.persist_noclobber(&options.output)?;
    report["output"] = serde_json::to_value(&options.output)?;
    Ok(report)
}

fn open_input(path: &Path) -> Result<WavReader<BufReader<File>>> {
    let reader = WavReader::open(path)?;
    let spec = reader.spec();
    if !matches!(spec.channels, 1 | 2) || !(8_000..=192_000).contains(&spec.sample_rate) {
        return Err("input WAV must be mono/stereo at 8000..192000 Hz".into());
    }
    if !matches!((spec.sample_format, spec.bits_per_sample), (SampleFormat::Float, 32) | (SampleFormat::Int, 8 | 16 | 24 | 32)) {
        return Err("input WAV must be PCM 8/16/24/32-bit or float32".into());
    }
    if reader.duration() == 0 || reader.len() % u32::from(spec.channels) != 0 {
        return Err("input WAV must contain complete, nonempty frames".into());
    }
    Ok(reader)
}

fn output_frames(spec: WavSpec, frames: u32, tail_ms: u32) -> Result<u64> {
    if tail_ms > 30_000 { return Err("tail-ms must be in 0..30000".into()); }
    let frames = u64::from(frames) + u64::from(spec.sample_rate) * u64::from(tail_ms) / 1000;
    // Hound uses RIFF's u32 chunk lengths. Reserve room for its headers instead of wrapping.
    if frames > (u64::from(u32::MAX) - 4096) / 8 { return Err("stereo float WAV would exceed the RIFF size limit".into()); }
    Ok(frames)
}

/// The iterator checks the declared data length: a short file is an error, not padded audio.
#[cfg(any(windows, test))]
fn read_frame(reader: &mut WavReader<BufReader<File>>) -> Result<[f32; 2]> {
    let spec = reader.spec();
    let mut frame = [0.0; 2];
    for sample in frame.iter_mut().take(usize::from(spec.channels)) {
        *sample = match spec.sample_format {
            SampleFormat::Float => reader.samples::<f32>().next().ok_or("truncated WAV input")??,
            SampleFormat::Int => {
                let value = reader.samples::<i32>().next().ok_or("truncated WAV input")??;
                (f64::from(value) / 2.0_f64.powi(i32::from(spec.bits_per_sample) - 1)) as f32
            }
        };
        if !sample.is_finite() { return Err("WAV input contains NaN/infinite samples".into()); }
    }
    if spec.channels == 1 { frame[1] = frame[0]; }
    Ok(frame)
}

#[cfg(windows)]
pub fn render_in_child(options: &RenderArgs) -> Result<Value> {
    use crate::{BLOCK_FRAMES, host};
    use hound::WavWriter;
    use serde_json::json;
    use std::{fs::OpenOptions, io::BufWriter};
    use vst3_host::audio::BusAudioBuffers;

    let mut reader = open_input(&options.input)?;
    let spec = reader.spec();
    let input_frames = u64::from(reader.duration());
    let total_frames = output_frames(spec, reader.duration(), options.tail_ms)?;
    let mut plugin = host::load_configured(&options.plugin_options(spec.sample_rate))?;
    let layout = plugin.audio_bus_layout()?;
    let stereo = |buses: &[vst3_host::audio::AudioBusConfig]| {
        buses.len() == 1 && buses[0].active && buses[0].channel_count == 2
    };
    if !stereo(&layout.inputs) || !stereo(&layout.outputs) {
        return Err("WAV rendering requires exactly one active stereo input and output bus; no buses are silently omitted".into());
    }
    let latency = plugin.latency_samples();
    let mut buffers = BusAudioBuffers::new(&layout, BLOCK_FRAMES, f64::from(spec.sample_rate));
    // Even direct invocation of the hidden worker cannot overwrite an existing file.
    let file = OpenOptions::new().write(true).create_new(true).open(&options.output)?;
    let mut file = BufWriter::new(file);
    let output_spec = WavSpec { channels: 2, sample_rate: spec.sample_rate, bits_per_sample: 32, sample_format: SampleFormat::Float };
    let mut writer = WavWriter::new(&mut file, output_spec)?;
    plugin.start_processing()?;
    let rendered = (|| -> Result<f32> {
        let mut position = 0_u64;
        let mut peak = 0.0_f32;
        while position < total_frames {
            let frames = (total_frames - position).min(BLOCK_FRAMES as u64) as usize;
            buffers.clear();
            buffers.block_size = frames;
            for index in 0..frames {
                if position + (index as u64) < input_frames {
                    let sample = read_frame(&mut reader)?;
                    buffers.inputs[0].channels[0][index] = sample[0];
                    buffers.inputs[0].channels[1][index] = sample[1];
                }
            }
            plugin.process_bus_audio(&mut buffers)?;
            if plugin.audio_bus_layout()? != layout || plugin.latency_samples() != latency {
                return Err("plugin changed its bus layout or latency during file processing".into());
            }
            for index in 0..frames {
                for channel in &buffers.outputs[0].channels {
                    let sample = channel[index];
                    if !sample.is_finite() { return Err("plugin returned NaN/infinite samples".into()); }
                    peak = peak.max(sample.abs());
                    writer.write_sample(sample)?;
                }
            }
            position += frames as u64;
        }
        Ok(peak)
    })();
    let stopped = plugin.stop_processing();
    let peak = rendered?;
    stopped?;
    writer.finalize()?;
    file.get_ref().sync_all()?;
    Ok(json!({ "schema": 1, "operation": "render_wav", "plugin": plugin.info(),
        "input_frames": input_frames, "output_frames": total_frames, "tail_frames": total_frames - input_frames,
        "sample_rate": spec.sample_rate, "input_channels": spec.channels, "output_channels": 2,
        "output_peak": peak, "latency_samples": latency, "latency_compensated": false,
        "state_loaded": options.state_in.is_some(), "live_audio_connected": false,
        "note": "Offline float32 WAV only; no normalization, limiting, automatic full-tail detection or live compatibility approval." }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hound::WavWriter;

    #[test]
    fn pcm_depths_are_scaled_and_mono_is_duplicated() {
        let directory = tempfile::tempdir().unwrap();
        for bits in [8, 16, 24, 32] {
            let path = directory.path().join(format!("{bits}.wav"));
            let spec = WavSpec { channels: 1, sample_rate: 44_100, bits_per_sample: bits, sample_format: SampleFormat::Int };
            let mut writer = WavWriter::create(&path, spec).unwrap();
            let value = (1_i64 << (bits - 2)) as i32;
            writer.write_sample(value).unwrap(); writer.write_sample(-value).unwrap(); writer.finalize().unwrap();
            let mut reader = open_input(&path).unwrap();
            assert_eq!(read_frame(&mut reader).unwrap(), [0.5; 2]);
            assert_eq!(read_frame(&mut reader).unwrap(), [-0.5; 2]);
        }
    }

    #[test]
    fn float_headroom_is_kept_and_nonfinite_or_truncated_samples_fail() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("float.wav");
        let spec = WavSpec { channels: 2, sample_rate: 48_000, bits_per_sample: 32, sample_format: SampleFormat::Float };
        let mut writer = WavWriter::create(&path, spec).unwrap();
        for sample in [2.0_f32, -3.0, f32::NAN, 0.0] { writer.write_sample(sample).unwrap(); }
        writer.finalize().unwrap();
        let mut reader = open_input(&path).unwrap();
        assert_eq!(read_frame(&mut reader).unwrap(), [2.0, -3.0]);
        assert!(read_frame(&mut reader).is_err());
        drop(reader);
        let mut writer = WavWriter::create(&path, spec).unwrap();
        for sample in [2.0_f32, -3.0, 0.1, 0.2] { writer.write_sample(sample).unwrap(); }
        writer.finalize().unwrap();
        let length = std::fs::metadata(&path).unwrap().len();
        std::fs::OpenOptions::new().write(true).open(&path).unwrap().set_len(length - 4).unwrap();
        let mut reader = open_input(&path).unwrap();
        read_frame(&mut reader).unwrap();
        assert!(read_frame(&mut reader).is_err());
        assert!(output_frames(spec, u32::MAX, 0).is_err());
        assert_eq!(output_frames(spec, 613, 1).unwrap(), 661);
    }
}
