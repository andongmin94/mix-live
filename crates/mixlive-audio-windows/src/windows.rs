//! WASAPI adapter. Endpoint selection, stream setup and text formatting stay off callbacks.
use std::{error::Error, sync::{Arc, atomic::Ordering}};
use cpal::{traits::{DeviceTrait, HostTrait, StreamTrait}, Device, Host, Sample, SampleFormat, SizedSample, Stream, StreamConfig, SupportedStreamConfig};
use mixlive_effects::VocalChain;
use crate::{bridge::{self, Fault, Health, OutputReader, OutputStats, OutputWriter}, capture::{Capture, VocalStats, CAPTURE_BLOCK}, config::{AuxDestination, InputMap, OutputMap, RouteOptions, output_sample}, control::{LiveControls, PlaybackGains}, plugin::{PluginArgs, native::{self, CaptureSink, PluginWorker}}};

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

// Each entry owns exactly one device stream/clock bridge; routes index the independent mixes.
struct OutputEndpoint { device: Device, config: SupportedStreamConfig, routes: Vec<(usize, OutputMap)> }

struct AuxiliaryEndpoint { device: Device, config: SupportedStreamConfig, map: InputMap }

// Shared input channels need neither a second stream nor another clock queue.
enum AuxFeed { Disabled, Shared(InputMap), Independent(Box<OutputReader>) }

pub struct Prepared {
    input: Device,
    input_config: SupportedStreamConfig,
    input_channel: usize,
    auxiliary: Option<AuxiliaryEndpoint>,
    aux_mix: Option<(f32, AuxDestination)>,
    vocal: VocalChain,
    output_gain_db: f32,
    send_gain_db: Option<f32>,
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
    /// Resolve exact IDs, channels and DSP coefficients without building or starting streams.
    pub fn new(options: &RouteOptions) -> Result<Self> {
        options.validate()?;
        let host = cpal::host_from_id(cpal::HostId::Wasapi)?;
        let input = find(&host, &options.input)?;
        // This command takes a mic, never the desktop because a render ID was pasted here.
        if !host.input_devices()?.any(|device| device == input) {
            return Err("input must be a capture endpoint listed under INPUT".into());
        }
        let input_config = input.default_input_config()?;
        validate_config(&input_config)?;
        if options.input_channel > input_config.channels() {
            return Err(format!("input channel {} exceeds this endpoint's {} channels", options.input_channel, input_config.channels()).into());
        }
        let auxiliary = options.aux_input.as_deref().map(|id| -> Result<AuxiliaryEndpoint> {
            let device = find(&host, id)?;
            if !host.input_devices()?.any(|candidate| candidate == device) {
                return Err("AUX must be a capture endpoint listed under INPUT, not a playback endpoint".into());
            }
            let config = if device == input { input_config } else { device.default_input_config()? };
            validate_config(&config)?;
            let map = InputMap::new(&options.aux_channels, config.channels())?;
            if device == input && map.contains(usize::from(options.input_channel - 1)) {
                return Err("microphone and AUX channels overlap on the resolved input endpoint".into());
            }
            Ok(AuxiliaryEndpoint { device, config, map })
        }).transpose()?;
        let vocal = VocalChain::new(input_config.sample_rate(), options.vocal_config())?;
        let mut outputs: Vec<OutputEndpoint> = Vec::new();
        let mut add = |id: &str, channels: &[u16], mix: usize| -> Result<()> {
            let device = find(&host, id)?;
            // Group the resolved device identity, not just the user-supplied ID spelling.
            if let Some(endpoint) = outputs.iter_mut().find(|endpoint| endpoint.device == device) {
                let map = OutputMap::new(channels, endpoint.config.channels())?;
                if endpoint.routes.iter().any(|(_, existing)| existing.overlaps(map)) {
                    return Err("output and send channels must not overlap on the same endpoint".into());
                }
                endpoint.routes.push((mix, map));
                return Ok(());
            }
            if !host.output_devices()?.any(|candidate| candidate == device) {
                return Err("output must be a playback endpoint listed under OUTPUT".into());
            }
            let config = device.default_output_config()?;
            validate_config(&config)?;
            let map = OutputMap::new(channels, config.channels())?;
            outputs.push(OutputEndpoint { device, config, routes: vec![(mix, map)] });
            Ok(())
        };
        add(&options.output, &options.output_channels, 0)?;
        if let Some(id) = &options.send { add(id, &options.send_channels, 1)?; }
        Ok(Self { input, input_config, input_channel: usize::from(options.input_channel - 1), vocal,
            auxiliary, aux_mix: options.aux_input.as_ref().map(|_| (options.aux_gain_db, options.aux_to)),
            output_gain_db: options.output_gain_db, send_gain_db: options.send.as_ref().map(|_| options.send_gain_db),
            queue_ms: options.queue_ms, outputs })
    }

    pub fn describe(&self) -> String {
        let mut text = format!("WASAPI input: {}\n  {:?}; channel {}\nvocal: {:?}\ninitial master attenuation: {} dB\nplayback streams: {}\n", self.input, self.input_config, self.input_channel + 1, self.vocal.config(), self.output_gain_db, self.outputs.len());
        for (index, endpoint) in self.outputs.iter().enumerate() {
            text.push_str(&format!("stream {index}: {}\n  {:?}; one queue/resampler for {} mix(es)\n", endpoint.device, endpoint.config, endpoint.routes.len()));
            for (mix, map) in &endpoint.routes {
                let gain = if *mix == 0 { 0.0 } else { self.send_gain_db.unwrap_or(0.0) };
                text.push_str(&format!("  mix {mix}: map {map:?} (zero-based); initial additional send gain {gain} dB\n"));
            }
        }
        if let Some(aux) = &self.auxiliary {
            text.push_str(&format!("AUX input: {}\n  {:?}; channels {},{}; mix settings {:?}\n  {}\n",
                aux.device, aux.config, aux.map.left + 1, aux.map.right + 1, self.aux_mix,
                if aux.device == self.input { "shares the microphone capture stream and clock" }
                else { "separate capture stream; buffered/resampled to the microphone clock (additional AUX delay)" }));
            text.push_str("AUX must carry only the intended MR/line audio, not MixLive output or call return. No app filtering or external feedback detection. Enable output/both only when AUX is not already heard through another path.\n");
        }
        text.push_str("Vocal preset/thresholds and device bindings are fixed for this run. --interactive enables live source/output mute and master/send gain with 5 ms ramps. Output mute acts after MixLive queues, not on audio already sent to the OS or on hardware direct monitoring.\nNo automatic makeup gain or lookahead buffer; IIR phase/group delay still exists. Check validates configuration, not device operation or sound quality. --vst3 is an optional in-process development path; ASIO and driver installation are not implemented.\n");
        text
    }

    pub fn start(self, plugin: &PluginArgs) -> Result<Active> {
        let health = Arc::new(Health::default());
        let controls = Arc::new(LiveControls::new(self.output_gain_db, self.send_gain_db, self.aux_mix.is_some())?);
        let mut streams = Vec::new();
        let mut writers = Vec::new();
        let mut stats = Vec::new();
        let mut timing = Vec::new();
        for (index, endpoint) in self.outputs.into_iter().enumerate() {
            let (writer, reader) = bridge::channel(self.input_config.sample_rate(), endpoint.config.sample_rate(), self.queue_ms, endpoint.routes.len(), health.clone())?;
            timing.push(format!("stream {index}: {} mix(es), shared queue target {:.2} ms + resampler delay {:.2} ms (excludes hardware and OS; NOT round-trip latency)", endpoint.routes.len(), reader.queue_ms, reader.resampler_delay_ms));
            stats.push(reader.stats.clone());
            let mixes: Vec<usize> = endpoint.routes.iter().map(|(mix, _)| *mix).collect();
            let gains = PlaybackGains::new(endpoint.config.sample_rate(), &mixes, controls.clone())?;
            writers.push((writer, mixes));
            let maps: Vec<OutputMap> = endpoint.routes.iter().map(|(_, map)| *map).collect();
            let format = endpoint.config.sample_format();
            let config = endpoint.config.config();
            let stream = match format {
                SampleFormat::F32 => output::<f32>(&endpoint.device, config, maps, reader, gains, health.clone())?,
                SampleFormat::F64 => output::<f64>(&endpoint.device, config, maps, reader, gains, health.clone())?,
                SampleFormat::I16 => output::<i16>(&endpoint.device, config, maps, reader, gains, health.clone())?,
                SampleFormat::I32 => output::<i32>(&endpoint.device, config, maps, reader, gains, health.clone())?,
                SampleFormat::U16 => output::<u16>(&endpoint.device, config, maps, reader, gains, health.clone())?,
                _ => return Err("sample format changed after validation".into()),
            };
            timing.push(format!("output driver buffer query: {:?} frames", stream.buffer_size()));
            streams.push(stream);
        }
        let mut aux_stream = None;
        let mut aux_clock_stats = None;
        let aux_feed = match self.auxiliary {
            None => AuxFeed::Disabled,
            Some(aux) if aux.device == self.input => AuxFeed::Shared(aux.map),
            Some(aux) => {
                let (writer, reader) = bridge::channel(aux.config.sample_rate(), self.input_config.sample_rate(), self.queue_ms, 1, health.clone())?;
                timing.push(format!("AUX input boundary: queue target {:.2} ms + resampler delay {:.2} ms BEFORE output queues (not acoustic/round-trip latency)", reader.queue_ms, reader.resampler_delay_ms));
                aux_clock_stats = Some(reader.stats.clone());
                let config = aux.config.config();
                let stream = match aux.config.sample_format() {
                    SampleFormat::F32 => input_aux::<f32>(&aux.device, config, aux.map, writer, health.clone())?,
                    SampleFormat::F64 => input_aux::<f64>(&aux.device, config, aux.map, writer, health.clone())?,
                    SampleFormat::I16 => input_aux::<i16>(&aux.device, config, aux.map, writer, health.clone())?,
                    SampleFormat::I32 => input_aux::<i32>(&aux.device, config, aux.map, writer, health.clone())?,
                    SampleFormat::U16 => input_aux::<u16>(&aux.device, config, aux.map, writer, health.clone())?,
                    _ => return Err("AUX sample format changed after validation".into()),
                };
                timing.push(format!("AUX driver buffer query: {:?} frames", stream.buffer_size()));
                aux_stream = Some(stream);
                AuxFeed::Independent(Box::new(reader))
            }
        };
        let capture = Capture::new(self.vocal, self.input_config.sample_rate(), self.aux_mix, writers, health.clone(), controls.clone())?;
        let vocal_stats = capture.vocal_stats.clone();
        let (capture, plugin_worker) = native::connect(capture, plugin, self.input_config.sample_rate(), self.queue_ms, self.aux_mix.is_some(), health.clone())?;
        if let Some(worker) = &plugin_worker { timing.push(worker.description.clone()); }
        let format = self.input_config.sample_format();
        let config = self.input_config.config();
        let input_stream = match format {
            SampleFormat::F32 => input::<f32>(&self.input, config, self.input_channel, capture, aux_feed, health.clone())?,
            SampleFormat::F64 => input::<f64>(&self.input, config, self.input_channel, capture, aux_feed, health.clone())?,
            SampleFormat::I16 => input::<i16>(&self.input, config, self.input_channel, capture, aux_feed, health.clone())?,
            SampleFormat::I32 => input::<i32>(&self.input, config, self.input_channel, capture, aux_feed, health.clone())?,
            SampleFormat::U16 => input::<u16>(&self.input, config, self.input_channel, capture, aux_feed, health.clone())?,
            _ => return Err("sample format changed after validation".into()),
        };
        timing.push(format!("input driver buffer query: {:?} frames", input_stream.buffer_size()));
        // Output emits silence until each queue is primed. Any startup error drops every stream.
        for stream in &streams { stream.play()?; }
        if let Some(stream) = &aux_stream { stream.play()?; }
        input_stream.play()?;
        Ok(Active { _input: input_stream, _aux_input: aux_stream, _outputs: streams, plugin_worker, health, outputs: stats,
            controls, aux_enabled: self.aux_mix.is_some(), aux_clock_stats, vocal_stats, timing })
    }
}

pub struct Active {
    // Drop devices, then join processing and release native plugin state on the loading thread.
    _input: Stream,
    _aux_input: Option<Stream>,
    _outputs: Vec<Stream>,
    plugin_worker: Option<PluginWorker>,
    pub health: Arc<Health>,
    pub controls: Arc<LiveControls>,
    /// One set per physical playback endpoint, shared by its channel routes.
    pub outputs: Vec<Arc<OutputStats>>,
    aux_enabled: bool,
    aux_clock_stats: Option<Arc<OutputStats>>,
    pub vocal_stats: Arc<VocalStats>,
    pub timing: Vec<String>,
}

fn input<T>(device: &Device, config: StreamConfig, selected: usize, mut capture: CaptureSink, mut aux: AuxFeed, health: Arc<Health>) -> Result<Stream>
where T: SizedSample + Send + 'static, f32: cpal::FromSample<T> {
    let channels = usize::from(config.channels);
    let error_health = health.clone();
    let mut mono = [0.0_f32; CAPTURE_BLOCK];
    let mut auxiliary = [[0.0_f32; 2]; CAPTURE_BLOCK];
    let has_aux = !matches!(&aux, AuxFeed::Disabled);
    Ok(device.build_input_stream::<T, _, _>(config, move |data, _| {
        if health.failed() { return; }
        if data.len() % channels != 0 { health.fail(Fault::Input); return; }
        for part in data.chunks(channels * CAPTURE_BLOCK) {
            let frames = part.len() / channels;
            for (index, frame) in part.chunks_exact(channels).enumerate() {
                mono[index] = f32::from_sample(frame[selected]);
                auxiliary[index] = match &mut aux {
                    AuxFeed::Disabled => [0.0; 2],
                    AuxFeed::Shared(map) => map.read(frame).map(f32::from_sample),
                    AuxFeed::Independent(reader) => reader.next_source_frame(),
                };
            }
            capture.push(&mono[..frames], has_aux.then_some(&auxiliary[..frames]));
        }
    }, move |_| error_health.fail(Fault::Device), None)?)
}

// An independent AUX device produces only its selected raw source; voice DSP runs elsewhere.
fn input_aux<T>(device: &Device, config: StreamConfig, map: InputMap, mut writer: OutputWriter, health: Arc<Health>) -> Result<Stream>
where T: SizedSample + Send + 'static, f32: cpal::FromSample<T> {
    let channels = usize::from(config.channels);
    let error_health = health.clone();
    Ok(device.build_input_stream::<T, _, _>(config, move |data, _| {
        if health.failed() { return; }
        if data.len() % channels != 0 { health.fail(Fault::Input); return; }
        for frame in data.chunks_exact(channels) {
            let [left, right] = map.read(frame).map(f32::from_sample);
            writer.push([left, right, 0.0, 0.0]);
        }
    }, move |_| error_health.fail(Fault::Device), None)?)
}

fn output<T>(device: &Device, config: StreamConfig, maps: Vec<OutputMap>, mut reader: OutputReader, mut gains: PlaybackGains, health: Arc<Health>) -> Result<Stream>
where T: SizedSample + cpal::FromSample<f32> + Send + 'static {
    let channels = usize::from(config.channels);
    let error_health = health.clone();
    Ok(device.build_output_stream::<T, _, _>(config, move |data, _| {
        data.fill(T::EQUILIBRIUM);
        if health.failed() { return; }
        if data.len() % channels != 0 { health.fail(Fault::Routing); return; }
        gains.update();
        for frame in data.chunks_exact_mut(channels) {
            // Consume even while muted. An output toggle must not replay a stale queue on unmute.
            let packed = reader.next_frame(gains.next_gains());
            for (channel, sample) in frame.iter_mut().enumerate() { *sample = T::from_sample(output_sample(&maps, channel, &packed)); }
        }
        if health.failed() { data.fill(T::EQUILIBRIUM); }
    }, move |_| error_health.fail(Fault::Device), None)?)
}

impl Active {
    /// Service fixed-configuration plugin requests on the loading/control thread.
    pub fn poll_plugin(&self) -> Result<()> {
        if let Some(worker) = &self.plugin_worker { worker.poll()?; }
        Ok(())
    }
    /// UI/CLI thread only. Peaks and gain reduction maxima are since the previous read.
    pub fn report(&self) -> String {
        let vocal = self.vocal_stats.take();
        let mut text = format!("captured={} input_peak={:.3} vocal_peak={:.3} comp_gr={:.1}dB deess_gr={:.1}dB", self.health.captured_frames.load(Ordering::Relaxed), f32::from_bits(self.health.input_peak.swap(0, Ordering::Relaxed)), vocal.output_peak, vocal.compression_db, vocal.deessing_db);
        if self.aux_enabled {
            text.push_str(&format!(" aux_peak={:.3}", f32::from_bits(self.health.aux_peak.swap(0, Ordering::Relaxed))));
            if let Some(stats) = &self.aux_clock_stats {
                text.push_str(&format!(" aux_queue={} frames aux_correction={}ppm aux_underruns={}",
                    stats.queue_frames.load(Ordering::Relaxed), stats.correction_ppm.load(Ordering::Relaxed),
                    stats.underruns.load(Ordering::Relaxed)));
            }
        }
        for (index, stats) in self.outputs.iter().enumerate() {
            text.push_str(&format!(" | stream{index}: peak={:.3} queue={} frames correction={}ppm underruns={} clipped={}", f32::from_bits(stats.peak.swap(0, Ordering::Relaxed)), stats.queue_frames.load(Ordering::Relaxed), stats.correction_ppm.load(Ordering::Relaxed), stats.underruns.load(Ordering::Relaxed), stats.clipped_frames.load(Ordering::Relaxed)));
        }
        text
    }
}
