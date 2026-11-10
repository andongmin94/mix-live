//! Reuse CI's existing native Gain fixture. No audio devices, recordings or commercial plugins.
#![cfg(windows)]
use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
use mixlive_vst3::{ParameterChange, render::{RenderArgs, render_file}};
use std::path::{Path, PathBuf};

fn executable() -> &'static Path { Path::new(env!("CARGO_BIN_EXE_mixlive-plugins")) }
fn options(directory: &Path) -> RenderArgs {
    RenderArgs { plugin: PathBuf::from(std::env::var_os("MIXLIVE_GAIN_VST3").expect("build the existing CI gain fixture")),
        input: directory.join("dry.wav"), output: directory.join("wet.wav"), class_id: None,
        parameters: vec![ParameterChange { id: 0, normalized: 0.25 }], state_in: None,
        tail_ms: 0, timeout_seconds: 30 }
}
fn write_float(path: &Path, samples: &[f32]) {
    let spec = WavSpec { channels: 2, sample_rate: 48_000, bits_per_sample: 32, sample_format: SampleFormat::Float };
    let mut writer = WavWriter::create(path, spec).unwrap();
    for sample in samples { writer.write_sample(*sample).unwrap(); }
    writer.finalize().unwrap();
}

#[test]
#[ignore = "requires the existing MIXLIVE_GAIN_VST3 fixture"]
fn wav_render_preserves_stereo_headroom_partial_blocks_and_mono_tail() {
    let directory = tempfile::tempdir().unwrap();
    let mut args = options(directory.path());
    let mut input: Vec<f32> = (0..613).flat_map(|index| {
        [(index as f32 * 0.17).sin() * 0.5, (index as f32 * 0.31).cos() * -0.3]
    }).collect();
    input[0] = 8.0;
    write_float(&args.input, &input);
    let original = std::fs::read(&args.input).unwrap();
    let report = render_file(executable(), &args).unwrap();
    assert_eq!(report["operation"], "render_wav");
    assert_eq!(report["output_frames"], 613);
    assert_eq!(report["live_audio_connected"], false);
    assert_eq!(report["latency_compensated"], false);
    assert_eq!(std::fs::read(&args.input).unwrap(), original);
    let mut reader = WavReader::open(&args.output).unwrap();
    assert_eq!(reader.spec().sample_format, SampleFormat::Float);
    assert_eq!(reader.spec().channels, 2);
    let samples: Vec<f32> = reader.samples().map(std::result::Result::unwrap).collect();
    assert_eq!(samples.len(), input.len());
    for (actual, input) in samples.iter().zip(&input) { assert!((actual - input * 0.25).abs() < 1e-6); }
    assert_eq!(samples[0], 2.0);
    drop(reader);
    let saved = std::fs::read(&args.output).unwrap();
    assert!(render_file(executable(), &args).is_err());
    assert_eq!(std::fs::read(&args.output).unwrap(), saved);

    args.input = directory.path().join("mono.wav");
    args.output = directory.path().join("mono-wet.wav");
    args.tail_ms = 1;
    let spec = WavSpec { channels: 1, sample_rate: 44_100, bits_per_sample: 24, sample_format: SampleFormat::Int };
    let mut writer = WavWriter::create(&args.input, spec).unwrap();
    for _ in 0..613 { writer.write_sample(4_194_304_i32).unwrap(); }
    writer.finalize().unwrap();
    let report = render_file(executable(), &args).unwrap();
    assert_eq!(report["sample_rate"], 44_100);
    assert_eq!(report["tail_frames"], 44);
    assert_eq!(report["output_frames"], 657);
    let mut reader = WavReader::open(&args.output).unwrap();
    assert_eq!(reader.duration(), 657);
    let samples: Vec<f32> = reader.samples().map(std::result::Result::unwrap).collect();
    assert!(samples[..1226].iter().all(|x| (*x - 0.125).abs() < 1e-6));
    assert!(samples[1226..].iter().all(|x| x.abs() < 1e-6));
}

#[test]
#[ignore = "requires the existing MIXLIVE_GAIN_VST3 and MIXLIVE_STATE_VST3 fixtures"]
fn failed_file_processing_never_publishes_or_overwrites_audio() {
    let directory = tempfile::tempdir().unwrap();
    let mut args = options(directory.path());
    write_float(&args.input, &[0.5, -0.25, f32::NAN, 0.0]);
    assert!(render_file(executable(), &args).is_err());
    assert!(!args.output.exists());
    write_float(&args.input, &[0.5, -0.25, 0.2, -0.1]);
    let length = std::fs::metadata(&args.input).unwrap().len();
    std::fs::OpenOptions::new().write(true).open(&args.input).unwrap().set_len(length - 4).unwrap();
    assert!(render_file(executable(), &args).is_err());
    assert!(!args.output.exists());
    write_float(&args.input, &[0.5, -0.25]);
    args.plugin = PathBuf::from(std::env::var_os("MIXLIVE_STATE_VST3").unwrap());
    args.parameters.clear();
    // This fixture is a synth with no audio input; it must not silently replace the source.
    assert!(render_file(executable(), &args).is_err());
    assert!(!args.output.exists());
    args.plugin = directory.path().join("invalid.vst3");
    std::fs::write(&args.plugin, b"not a native module").unwrap();
    assert!(render_file(executable(), &args).is_err());
    assert!(!args.output.exists());
    assert!(std::fs::read_dir(directory.path()).unwrap().all(|entry| !entry.unwrap().path().is_dir()));
}
