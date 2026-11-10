//! Control-thread VST3 operations only. Never call these functions from an audio callback.

use crate::{BLOCK_FRAMES, InspectArgs, Result, StateFile, TEST_BLOCKS};
use serde_json::{Value, json};
use std::path::PathBuf;
use vst3_host::{Plugin, Vst3Host, audio::{AudioBusLayout, BusAudioBuffers}, plugin::ProcessMode};

pub fn list(directories: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut host = Vst3Host::new()?;
    for directory in directories { host.add_scan_path(directory)?; }
    let mut paths = host.scan_plugin_paths();
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// Shared by synthetic inspection and WAV rendering; never run on the live audio thread.
pub(crate) fn load_configured(options: &InspectArgs) -> Result<Plugin> {
    options.validate()?;
    let saved = options.state_in.as_deref().map(StateFile::read).transpose()?;
    let requested = options.class_id.as_deref().or_else(|| saved.as_ref().map(|state| state.class_id.as_str()));
    let mut host = Vst3Host::builder().build()?;
    let mut plugin = match requested {
        Some(id) => host.load_plugin_class(&options.plugin, id)?,
        None => host.load_plugin(&options.plugin)?,
    };
    if requested.is_some_and(|id| !id.eq_ignore_ascii_case(&plugin.info().uid)) {
        return Err("loaded audio class differs from the requested class ID".into());
    }
    plugin.reconfigure(f64::from(options.sample_rate), BLOCK_FRAMES)?;
    plugin.set_process_mode(ProcessMode::Offline)?;
    if let Some(state) = saved.as_ref() {
        if !state.class_id.eq_ignore_ascii_case(&plugin.info().uid) {
            return Err("state belongs to another VST3 audio class".into());
        }
        plugin.load_state(&state.state)?;
    }
    let parameters = plugin.get_parameters()?;
    for change in &options.parameters {
        let parameter = parameters.iter().find(|parameter| parameter.id == change.id)
            .ok_or_else(|| format!("unknown VST3 parameter {}", change.id))?;
        if parameter.is_read_only { return Err(format!("VST3 parameter {} is read-only", change.id).into()); }
        plugin.set_parameter(change.id, change.normalized)?;
    }
    Ok(plugin)
}

pub fn probe(options: &InspectArgs) -> Result<Value> {
    let mut plugin = load_configured(options)?;
    let layout = plugin.audio_bus_layout()?;
    validate_layout(&layout)?;
    let latency = plugin.latency_samples();
    let mut buffers = BusAudioBuffers::new(&layout, BLOCK_FRAMES, f64::from(options.sample_rate));
    plugin.start_processing()?;
    let rendered = render(&mut plugin, &mut buffers);
    let stopped = plugin.stop_processing();
    let rendered = rendered?;
    stopped?;
    let final_layout = plugin.audio_bus_layout()?;
    if final_layout != layout {
        return Err("plugin changed its audio buses during the probe; configure it and inspect again".into());
    }
    let parameters = plugin.get_parameters()?;
    let state_bytes = if let Some(path) = options.state_out.as_deref() {
        let state = StateFile { schema: 1, class_id: plugin.info().uid.clone(), state: plugin.save_state()? };
        let length = state.state.len();
        state.save_new(path)?;
        Some(length)
    } else { None };
    Ok(json!({
        "schema": 1,
        "plugin": plugin.info(),
        "parameters": parameters,
        "audio_buses": layout,
        "sample_rate": options.sample_rate,
        "block_frames": BLOCK_FRAMES,
        "latency_samples_before": latency,
        "latency_samples_after": plugin.latency_samples(),
        "synthetic_processing": rendered,
        "state_loaded": options.state_in.is_some(),
        "state_saved_bytes": state_bytes,
        "live_audio_connected": false,
        "note": "Synthetic, offline preflight only. No audio devices or editor windows were opened by MixLive. This is not a live-compatibility or sound-quality approval."
    }))
}

fn validate_layout(layout: &AudioBusLayout) -> Result<()> {
    for buses in [&layout.inputs, &layout.outputs] {
        let total = buses.iter().try_fold(0_usize, |sum, bus| sum.checked_add(bus.channel_count));
        if buses.len() > 16 || total.is_none_or(|channels| channels > 64) {
            return Err("plugin bus layout exceeds the preflight limit (16 buses / 64 channels per direction)".into());
        }
    }
    if !layout.outputs.iter().any(|bus| bus.active && bus.channel_count > 0) {
        return Err("plugin has no active audio output".into());
    }
    Ok(())
}

fn render(plugin: &mut Plugin, buffers: &mut BusAudioBuffers) -> Result<Value> {
    let mut input_energy = 0.0_f64;
    let mut output_energy = 0.0_f64;
    let mut input_count = 0_u64;
    let mut output_count = 0_u64;
    let mut output_peak = 0.0_f32;
    for block in 0..TEST_BLOCKS {
        buffers.clear();
        for (bus_index, bus) in buffers.inputs.iter_mut().enumerate().filter(|(_, bus)| bus.active) {
            for (channel_index, channel) in bus.channels.iter_mut().enumerate() {
                let frequency = 220.0 + 110.0 * (channel_index + bus_index) as f64;
                for (sample_index, sample) in channel.iter_mut().enumerate() {
                    let time = (block * BLOCK_FRAMES + sample_index) as f64 / buffers.sample_rate;
                    *sample = (0.125 * (std::f64::consts::TAU * frequency * time).sin()) as f32;
                    input_energy += f64::from(*sample).powi(2);
                    input_count += 1;
                }
            }
        }
        plugin.process_bus_audio(buffers)?;
        for bus in buffers.outputs.iter().filter(|bus| bus.active) {
            for channel in &bus.channels {
                for &sample in channel {
                    if !sample.is_finite() { return Err("plugin returned NaN/infinite samples".into()); }
                    output_peak = output_peak.max(sample.abs());
                    output_energy += f64::from(sample).powi(2);
                    output_count += 1;
                }
            }
        }
    }
    let input_rms = if input_count == 0 { 0.0 } else { (input_energy / input_count as f64).sqrt() };
    let output_rms = if output_count == 0 { 0.0 } else { (output_energy / output_count as f64).sqrt() };
    Ok(json!({
        "frames": BLOCK_FRAMES * TEST_BLOCKS,
        "input_samples": input_count,
        "output_samples": output_count,
        "input_rms": input_rms,
        "output_rms": output_rms,
        "output_peak": output_peak,
        "output_input_rms_ratio": if input_rms > 0.0 { Some(output_rms / input_rms) } else { None },
        "finite": true
    }))
}
