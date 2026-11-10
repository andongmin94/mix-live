//! WASAPI adapter. Endpoint selection, stream setup and text formatting stay off callbacks.
use std::{error::Error, sync::{Arc, atomic::Ordering}};
use cpal::{traits::{DeviceTrait, HostTrait, StreamTrait}, Device, Host, Sample, SampleFormat, SizedSample, Stream, StreamConfig, SupportedStreamConfig};
use crate::{bridge::{self, Fault, Health, OutputReader, OutputStats}, capture::{Capture, CAPTURE_BLOCK}, config::{OutputMap, RouteOptions}};

pub type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

pub fn list_devices() -> Result<Vec<String>> {
    let host = cpal::host_from_id(cpal::HostId::Wasapi)?;
    let mut rows = Vec::new();
    for device in host.input_devices()? {
        rows.push(format!("INPUT  {device}\n  id: {}\n  default: {:?}", device.id()?, device.default_input_config()));
    }
    for device in host.output_devices()? {
        rows.push(format!("OUTPUT {device}\n  id: {}\n  default: {:?}", device.id()?, device.default_output_config()));
    }
    Ok(rows)
}

struct OutputEndpoint {
    device: Device,
    config: SupportedStreamConfig,
    map: OutputMap,
    gain_db: f32,
}

pub struct Prepared {
    input: Device,
    input_config: SupportedStreamConfig,
    input_channel: usize,
    gain_db: f32,
    queue_ms: u32,
    outputs: Vec<OutputEndpoint>,
}

fn find(host: &Host, id: &str) -> Result<Device> {
    let device_id: cpal::DeviceId = id.parse()?;
    host.device_by_id(&device_id).ok_or_else(|| format!("endpoint is unavailable: {id}; run devices again (no default substituted)").into())
}

fn validate_config(config: &SupportedStreamConfig) -> Result<()> {
    if config.channels() == 0 || config.channels() > 256 || !(8_000..=192_000).contains(&config.sample_rate()) {
        return Err("this device path supports 1..=256 channels at 8000..=192000 Hz".into());
    }
    if !matches!(config.sample_format(), SampleFormat::F32 | SampleFormat::F64 | SampleFormat::I16 | SampleFormat::I32 | SampleFormat::U16) {
        return Err(format!("unsupported endpoint sample format: {}", config.sample_format()).into());
    }
    Ok(())
}

impl Prepared {
    /// Resolve exact IDs and check channels without building or starting any streams.
    pub fn new(options: &RouteOptions) -> Result<Self> {
        options.validate()?;
        let host = cpal::host_from_id(cpal::HostId::Wasapi)?;
        let input = find(&host, &options.input)?;
        // WASAPI can also capture a render device's loopback. This S1 command must take a mic,
        // never start capturing the desktop just because an output ID was pasted here.
        if !host.input_devices()?.any(|device| device == input) {
            return Err("input must be a capture endpoint listed under INPUT".into());
        }
        let input_config = input.default_input_config()?;
        validate_config(&input_config)?;
        if options.input_channel > input_config.channels() {
            return Err(format!("input channel {} exceeds this endpoint's {} channels", options.input_channel, input_config.channels()).into());
        }
        let mut outputs = Vec::new();
        let mut add = |id: &str, channels: &[u16], gain_db: f32| -> Result<()> {
            let device = find(&host, id)?;
            if !host.output_devices()?.any(|candidate| candidate == device) {
                return Err("output must be a playback endpoint listed under OUTPUT".into());
            }
            let config = device.default_output_config()?;
            validate_config(&config)?;
            let map = OutputMap::new(channels, config.channels())?;
            outputs.push(OutputEndpoint { device, config, map, gain_db });
            Ok(())
        };
        add(&options.output, &options.output_channels, 0.0)?;
        if let Some(id) = &options.send { add(id, &options.send_channels, options.send_gain_db)?; }
        Ok(Self { input, input_config, input_channel: usize::from(options.input_channel - 1), gain_db: options.gain_db, queue_ms: options.queue_ms, outputs })
    }

    pub fn describe(&self) -> String {
        let mut text = format!("WASAPI input: {}\n  {:?}; channel {}; input gain {} dB\n", self.input, self.input_config, self.input_channel + 1, self.gain_db);
        for (index, endpoint) in self.outputs.iter().enumerate() {
            text.push_str(&format!("output {index}: {}\n  {:?}; map {:?} (zero-based); mix gain {} dB\n", endpoint.device, endpoint.config, endpoint.map, endpoint.gain_db));
        }
        text.push_str("Check validates configuration only, not actual device operation. ASIO, vocal DSP and driver installation are not part of S1.\n");
        text
    }

    pub fn start(self) -> Result<Active> {
        let health = Arc::new(Health::default());
        let mut streams = Vec::new();
        let mut writers = Vec::new();
        let mut stats = Vec::new();
        let mut timing = Vec::new();
        for endpoint in self.outputs {
            let (writer, reader) = bridge::channel(self.input_config.sample_rate(), endpoint.config.sample_rate(), self.queue_ms, health.clone())?;
            timing.push(format!("queue target {:.2} ms + resampler delay {:.2} ms (excludes hardware and OS; NOT round-trip latency)", reader.queue_ms, reader.resampler_delay_ms));
            stats.push(reader.stats.clone());
            writers.push((writer, endpoint.gain_db));
            let format = endpoint.config.sample_format();
            let config = endpoint.config.config();
            let stream = match format {
                SampleFormat::F32 => output::<f32>(&endpoint.device, config, endpoint.map, reader, health.clone())?,
                SampleFormat::F64 => output::<f64>(&endpoint.device, config, endpoint.map, reader, health.clone())?,
                SampleFormat::I16 => output::<i16>(&endpoint.device, config, endpoint.map, reader, health.clone())?,
                SampleFormat::I32 => output::<i32>(&endpoint.device, config, endpoint.map, reader, health.clone())?,
                SampleFormat::U16 => output::<u16>(&endpoint.device, config, endpoint.map, reader, health.clone())?,
                _ => return Err("sample format changed after validation".into()),
            };
            timing.push(format!("output driver buffer query: {:?} frames", stream.buffer_size()));
            streams.push(stream);
        }
        let capture = Capture::new(self.gain_db, writers, health.clone())?;
        let format = self.input_config.sample_format();
        let config = self.input_config.config();
        let input_stream = match format {
            SampleFormat::F32 => input::<f32>(&self.input, config, self.input_channel, capture, health.clone())?,
            SampleFormat::F64 => input::<f64>(&self.input, config, self.input_channel, capture, health.clone())?,
            SampleFormat::I16 => input::<i16>(&self.input, config, self.input_channel, capture, health.clone())?,
            SampleFormat::I32 => input::<i32>(&self.input, config, self.input_channel, capture, health.clone())?,
            SampleFormat::U16 => input::<u16>(&self.input, config, self.input_channel, capture, health.clone())?,
            _ => return Err("sample format changed after validation".into()),
        };
        timing.push(format!("input driver buffer query: {:?} frames", input_stream.buffer_size()));
        // Output emits silence until each queue is primed. Any startup error drops every stream.
        for stream in &streams { stream.play()?; }
        input_stream.play()?;
        Ok(Active { _input: input_stream, _outputs: streams, health, outputs: stats, timing })
    }
}

pub struct Active {
    // Field drop order stops capture before the outputs.
    _input: Stream,
    _outputs: Vec<Stream>,
    pub health: Arc<Health>,
    pub outputs: Vec<Arc<OutputStats>>,
    pub timing: Vec<String>,
}

fn input<T>(device: &Device, config: StreamConfig, selected: usize, mut capture: Capture, health: Arc<Health>) -> Result<Stream>
where T: SizedSample + Send + 'static, f32: cpal::FromSample<T> {
    let channels = usize::from(config.channels);
    let error_health = health.clone();
    let mut mono = [0.0_f32; CAPTURE_BLOCK];
    Ok(device.build_input_stream::<T, _, _>(config, move |data, _| {
        if health.failed() { return; }
        if data.len() % channels != 0 { health.fail(Fault::Input); return; }
        for part in data.chunks(channels * CAPTURE_BLOCK) {
            let frames = part.len() / channels;
            for (frame, sample) in part.chunks_exact(channels).zip(mono.iter_mut()) {
                *sample = f32::from_sample(frame[selected]);
            }
            capture.push_mono(&mono[..frames]);
        }
    }, move |_| error_health.fail(Fault::Device), None)?)
}

fn output<T>(device: &Device, config: StreamConfig, map: OutputMap, mut reader: OutputReader, health: Arc<Health>) -> Result<Stream>
where T: SizedSample + cpal::FromSample<f32> + Send + 'static {
    let channels = usize::from(config.channels);
    let error_health = health.clone();
    Ok(device.build_output_stream::<T, _, _>(config, move |data, _| {
        data.fill(T::EQUILIBRIUM);
        if health.failed() { return; }
        if data.len() % channels != 0 { health.fail(Fault::Routing); return; }
        for frame in data.chunks_exact_mut(channels) {
            let stereo = reader.next_frame();
            for (channel, sample) in frame.iter_mut().enumerate() { *sample = T::from_sample(map.sample(channel, stereo)); }
        }
        if health.failed() { data.fill(T::EQUILIBRIUM); }
    }, move |_| error_health.fail(Fault::Device), None)?)
}

impl Active {
    /// Called by the UI/CLI thread, never an audio callback. Peaks are since the previous read.
    pub fn report(&self) -> String {
        let mut text = format!("captured={} input_peak={:.3}", self.health.captured_frames.load(Ordering::Relaxed), f32::from_bits(self.health.input_peak.swap(0, Ordering::Relaxed)));
        for (index, stats) in self.outputs.iter().enumerate() {
            text.push_str(&format!(" | out{index}: peak={:.3} queue={} frames correction={}ppm underruns={} clipped={}", f32::from_bits(stats.peak.swap(0, Ordering::Relaxed)), stats.queue_frames.load(Ordering::Relaxed), stats.correction_ppm.load(Ordering::Relaxed), stats.underruns.load(Ordering::Relaxed), stats.clipped_frames.load(Ordering::Relaxed)));
        }
        text
    }
}
