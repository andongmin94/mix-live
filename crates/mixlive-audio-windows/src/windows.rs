//! WASAPI adapter. Endpoint selection, stream setup and text formatting stay off callbacks.
use std::{error::Error, sync::{Arc, atomic::{AtomicBool, AtomicU64, Ordering}}};
use cpal::{traits::{DeviceTrait, HostTrait, StreamTrait}, Device, Host, Sample, SampleFormat, SizedSample, Stream, StreamConfig, SupportedStreamConfig};
use mixlive_effects::{VocalChain, VocalSettings};
use crate::devices::{AppInfo, DeviceCatalog, DeviceConfig, DeviceInfo, ProcessTarget, resolve_apps};
use crate::process_capture::{self, ProcessCapture};
use crate::pitch::{self, PitchMonitor, PitchReading, PitchTap};
use crate::{bridge::{self, Fault, Health, LimiterReading, OutputReader, OutputStats, OutputWriter}, capture::{Capture, VocalStats, CAPTURE_BLOCK}, config::{AppSource, AuxSource, InputMap, OutputMap, PitchConfig, PitchScale, RouteOptions, MAX_APPS, MAX_MIXES, output_sample}, control::{AppControls, LiveControls, PlaybackGains, RouteMix, RouteSource}, plugin::{ResolvedPlugins, native::{self, AuxFeed, CaptureSink, PluginWorker, SourceFeeds}}};

pub type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

/// Query endpoint IDs and default configurations without choosing a default device or
/// creating audio streams. A per-device configuration error remains in the catalog.
pub fn device_catalog() -> Result<DeviceCatalog> {
    let host = cpal::host_from_id(cpal::HostId::Wasapi)?;
    let mut catalog = DeviceCatalog::default();
    for device in host.input_devices()? {
        catalog.inputs.push(device_info(&device, device.default_input_config())?);
    }
    for device in host.output_devices()? {
        catalog.outputs.push(device_info(&device, device.default_output_config())?);
    }
    catalog.apps = process_capture::app_catalog();
    Ok(catalog)
}

/// Running application paths and process-tree roots; no native audio activation.
pub fn list_apps() -> Vec<AppInfo> { process_capture::app_catalog() }

fn device_info(device: &Device, config: std::result::Result<SupportedStreamConfig, cpal::Error>) -> Result<DeviceInfo> {
    let config = config.map_err(|error| error.to_string()).and_then(|config| {
        validate_config(&config).map_err(|error| error.to_string())?;
        Ok(DeviceConfig {
            channels: config.channels(),
            sample_rate: config.sample_rate(),
            sample_format: config.sample_format().to_string(),
        })
    });
    Ok(DeviceInfo {
        id: device.id()?.to_string(),
        name: device.description()?.name().to_owned(),
        config,
    })
}

pub fn list_devices() -> Result<Vec<String>> {
    let catalog = device_catalog()?;
    let mut rows = Vec::new();
    for (direction, devices) in [("INPUT ", catalog.inputs), ("OUTPUT", catalog.outputs)] {
        for device in devices {
            let config = match device.config {
                Ok(config) => format!("{} channels, {} Hz, {}", config.channels, config.sample_rate, config.sample_format),
                Err(error) => format!("unavailable: {error}"),
            };
            rows.push(format!("{direction} {}\n  id: {}\n  default: {config}", device.name, device.id));
        }
    }
    Ok(rows)
}

// Each entry owns exactly one device stream/clock bridge; routes index the independent mixes.
struct OutputEndpoint { device: Device, config: SupportedStreamConfig, routes: Vec<(usize, OutputMap)> }

struct AuxiliaryEndpoint { source: AuxSource, device: Device, config: SupportedStreamConfig, map: InputMap }

struct AuxiliaryStream { stream: Stream, enabled: Arc<AtomicBool>, stats: Arc<OutputStats> }
impl Drop for AuxiliaryStream {
    fn drop(&mut self) { self.enabled.store(false, Ordering::Release); }
}

struct PreparedAux { feed: AuxFeed, stream: Option<AuxiliaryStream>, description: String }

pub struct Prepared {
    input: Device,
    input_config: SupportedStreamConfig,
    input_channel: usize,
    auxiliary: Option<AuxiliaryEndpoint>,
    apps: Vec<(AppSource, ProcessTarget, InputMap)>,
    vocal: VocalChain,
    pitch: PitchConfig,
    output_gain_db: f32,
    mix_gains_db: [f32; MAX_MIXES],
    queue_ms: u32,
    controls: LiveControls,
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

fn resolve_aux(host: &Host, source: AuxSource, input: &Device, input_config: SupportedStreamConfig,
    input_channel: usize) -> Result<AuxiliaryEndpoint> {
    if source.input.trim().is_empty() { return Err("AUX endpoint ID must not be blank".into()); }
    let device = find(host, &source.input)?;
    if !host.input_devices()?.any(|candidate| candidate == device) {
        return Err("AUX must be a capture endpoint listed under INPUT, not a playback endpoint".into());
    }
    let config = if &device == input { input_config } else { device.default_input_config()? };
    validate_config(&config)?;
    let map = InputMap::new(&source.channels, config.channels())?;
    if &device == input && map.contains(input_channel) {
        return Err("microphone and AUX channels overlap on the resolved input endpoint".into());
    }
    Ok(AuxiliaryEndpoint { source, device, config, map })
}

fn shared_aux_tag(generation: u64, map: InputMap) -> u64 {
    (generation << 16) | (map.left as u64) | ((map.right as u64) << 8)
}

fn shared_aux_map(tag: u64) -> InputMap {
    InputMap { left: (tag & 255) as usize, right: ((tag >> 8) & 255) as usize }
}

fn prepare_aux(endpoint: Option<AuxiliaryEndpoint>, input: &Device, rate: u32, queue_ms: u32,
    shared_tag: u64) -> Result<PreparedAux> {
    let Some(aux) = endpoint else {
        return Ok(PreparedAux { feed: AuxFeed::Disabled, stream: None, description: "Current AUX: disabled".into() });
    };
    let mut description = format!("Current AUX: {}\n  {:?}; channels {:?}\n", aux.source.input, aux.config, aux.source.channels);
    if &aux.device == input {
        description.push_str("Shares the microphone stream/clock; changed shared selections discard queued samples from the old selection.\n");
        return Ok(PreparedAux { feed: AuxFeed::Shared(shared_tag), stream: None, description });
    }
    let health = Arc::new(Health::default());
    let enabled = Arc::new(AtomicBool::new(false));
    let (writer, reader) = bridge::channel(aux.config.sample_rate(), rate, queue_ms, 1, health.clone())?;
    description.push_str(&format!("Independent AUX queue target {:.2} ms + resampler delay {:.2} ms (excludes OS/hardware).\n", reader.queue_ms, reader.resampler_delay_ms));
    let stats = reader.stats.clone();
    let config = aux.config.config();
    let stream = match aux.config.sample_format() {
        SampleFormat::F32 => input_aux::<f32>(&aux.device, config, aux.map, writer, health.clone(), enabled.clone())?,
        SampleFormat::F64 => input_aux::<f64>(&aux.device, config, aux.map, writer, health.clone(), enabled.clone())?,
        SampleFormat::I16 => input_aux::<i16>(&aux.device, config, aux.map, writer, health.clone(), enabled.clone())?,
        SampleFormat::I32 => input_aux::<i32>(&aux.device, config, aux.map, writer, health.clone(), enabled.clone())?,
        SampleFormat::U16 => input_aux::<u16>(&aux.device, config, aux.map, writer, health.clone(), enabled.clone())?,
        _ => return Err("AUX sample format changed after validation".into()),
    };
    description.push_str(&format!("AUX driver buffer query: {:?} frames", stream.buffer_size()));
    Ok(PreparedAux { feed: AuxFeed::Independent { reader: Box::new(reader), health, enabled: enabled.clone() },
        stream: Some(AuxiliaryStream { stream, enabled, stats }), description })
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
        let auxiliary = options.aux_input.as_ref().map(|id| resolve_aux(&host,
            AuxSource { input: id.clone(), channels: options.aux_channels.clone() },
            &input, input_config, usize::from(options.input_channel - 1))).transpose()?;
        let apps = if options.apps.is_empty() { Vec::new() } else {
            let targets = resolve_apps(&options.apps, &process_capture::app_catalog())?;
            options.apps.iter().zip(targets).map(|(app, target)| -> Result<_> {
                Ok((app.clone(), target, InputMap::new(&app.channels, 2)?))
            }).collect::<Result<Vec<_>>>()?
        };
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
        for (mix, id, channels) in options.playback_bindings() { add(id, channels, mix)?; }
        Ok(Self { input, input_config, input_channel: usize::from(options.input_channel - 1), vocal,
            pitch: options.pitch_config(),
            auxiliary, apps,
            output_gain_db: options.output_gain_db, mix_gains_db: [options.monitor_gain_db, options.send_gain_db, options.broadcast_gain_db],
            queue_ms: options.queue_ms, controls: LiveControls::from_route(options)?, outputs })
    }

    pub fn describe(&self) -> String {
        let mut text = format!("WASAPI input: {}\n  {:?}; channel {}\nvocal: {:?}\ninitial master attenuation: {} dB\nplayback streams: {}\n", self.input, self.input_config, self.input_channel + 1, self.vocal.config(), self.output_gain_db, self.outputs.len());
        text.push_str(&format!("Initial pitch reference: {} {}; correction {:.0}%; retune {:.1} ms\n", PitchScale::NAMES[usize::from(self.pitch.key)], self.pitch.scale.name(), self.pitch.strength * 100.0, self.pitch.retune_ms));
        for (index, endpoint) in self.outputs.iter().enumerate() {
            text.push_str(&format!("stream {index}: {}\n  {:?}; one queue/resampler for {} mix(es)\n", endpoint.device, endpoint.config, endpoint.routes.len()));
            for (mix, map) in &endpoint.routes {
                let gain = self.mix_gains_db[*mix];
                text.push_str(&format!("  mix {mix}: map {map:?} (zero-based); initial mix attenuation {gain} dB\n"));
            }
        }
        if let Some(aux) = &self.auxiliary {
            text.push_str(&format!("Initial AUX input: {}\n  {:?}; channels {},{}; trim {:.1} dB\n  {}\n",
                aux.device, aux.config, aux.map.left + 1, aux.map.right + 1, self.controls.source_trim_db(RouteSource::Aux).expect("configured AUX"),
                if aux.device == self.input { "shares the microphone capture stream and clock" }
                else { "separate capture stream; buffered/resampled to the microphone clock (additional AUX delay)" }));
            text.push_str("AUX must carry only the intended MR/line audio, not MixLive output or call return. No app filtering or external feedback detection. Enable output/both only when AUX is not already heard through another path.\n");
        }
        for (index, (app, target, map)) in self.apps.iter().enumerate() {
            let gain = self.controls.source_trim_db(RouteSource::App(index)).expect("configured app");
            text.push_str(&format!("Initial app {}: {}\n  PID {} and descendants; 48000 Hz float32 stereo, channels {},{}; trim {gain} dB\n  Independent clock bridge. Original application playback is unchanged; no automatic rebind or whole-desktop capture.\n",
                index + 1, app.path, target.pid, map.left + 1, map.right + 1));
        }
        text.push_str(&format!("Initial mixer: {}\n", self.controls.describe()));
        text.push_str("Microphone and output device bindings are fixed for this run. GPUI can add, replace or remove physical AUX; its new capture/PDC history starts empty and changed shared selections briefly silence AUX. Live vocal preset, reverb pre-delay, echo time and pitch settings are also available. --interactive enables live mixer controls (5 ms ramps) and vocal trim/strength/EQ/dynamics/reverb blend/decay/echo blend/feedback targets (10 ms ramps). Output mute acts after MixLive queues, not on audio already sent to the OS or on hardware direct monitoring.\nNo automatic makeup gain; optional pitch correction adds one analysis window of latency, and IIR phase/group delay still exists. Check validates configuration, not device operation or sound quality. --vst3 is an optional in-process development path; ASIO and driver installation are not implemented.\n");
        text
    }

    pub fn start(self, plugin: &ResolvedPlugins) -> Result<Active> {
        plugin.validate()?;
        let health = Arc::new(Health::default());
        let mut controls = self.controls;
        controls.prepare_effects(plugin.effects.iter().map(|effect| effect.bypassed));
        let controls = Arc::new(controls);
        let mut streams = Vec::new();
        let mut writers = Vec::new();
        let mut stats = Vec::new();
        let mut timing = Vec::new();
        for (index, endpoint) in self.outputs.into_iter().enumerate() {
            let (writer, reader) = bridge::channel(self.input_config.sample_rate(), endpoint.config.sample_rate(), self.queue_ms, endpoint.routes.len(), health.clone())?;
            timing.push(format!("stream {index}: {} mix(es), shared queue target {:.2} ms + resampler delay {:.2} ms (excludes hardware and OS; NOT round-trip latency)", endpoint.routes.len(), reader.queue_ms, reader.resampler_delay_ms));
            let mixes: Vec<usize> = endpoint.routes.iter().map(|(mix, _)| *mix).collect();
            stats.push(PlaybackStats { mixes: mixes.clone(), stats: reader.stats.clone() });
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
        let aux_source = self.auxiliary.as_ref().map(|aux| aux.source.clone());
        let aux_generation = u64::from(self.auxiliary.as_ref().is_some_and(|aux| aux.device == self.input));
        let shared_tag = self.auxiliary.as_ref().filter(|aux| aux.device == self.input)
            .map_or(0, |aux| shared_aux_tag(aux_generation, aux.map));
        let shared_aux = Arc::new(AtomicU64::new(shared_tag));
        let prepared_aux = prepare_aux(self.auxiliary, &self.input, self.input_config.sample_rate(), self.queue_ms, shared_tag)?;
        let aux_timing = timing.len();
        timing.push(prepared_aux.description);
        let aux_stream = prepared_aux.stream;
        // Each app retains its own clock/queue. Prepare all clients before starting any of them.
        let mut app_readers = Vec::with_capacity(self.apps.len());
        let mut apps = Vec::with_capacity(self.apps.len());
        let mut app_captures = Vec::with_capacity(self.apps.len());
        let app_timing = timing.len();
        timing.push(String::new());
        for (app, target, map) in self.apps {
            let (writer, reader) = bridge::channel(process_capture::RATE, self.input_config.sample_rate(), self.queue_ms, 1, health.clone())?;
            let capture = ProcessCapture::prepare(app.path.clone(), target, map, writer, health.clone())?;
            app_captures.push(ActiveAppCapture { capture, target, stats: reader.stats.clone(),
                queue_ms: reader.queue_ms, resampler_delay_ms: reader.resampler_delay_ms });
            apps.push(app);
            app_readers.push(reader);
        }
        let rate = self.input_config.sample_rate();
        let mut capture = Capture::new(self.vocal, rate, writers, health.clone(), controls.clone())?;
        capture.prepare_pitch(rate, self.pitch)?;
        let vocal_stats = capture.vocal_stats.clone();
        let (capture, plugin_worker) = native::connect(capture, plugin, self.input_config.sample_rate(), self.queue_ms, health.clone(), &controls,
            SourceFeeds { aux: prepared_aux.feed, aux_source, apps: app_readers })?;
        let plugin_timing = timing.len();
        timing.push(plugin_worker.description.clone());
        let format = self.input_config.sample_format();
        let config = self.input_config.config();
        let (pitch_tap, pitch_monitor) = pitch::channel(self.input_config.sample_rate())?;
        let feeds = InputFeeds { shared_aux: shared_aux.clone(), pitch: pitch_tap };
        let input_stream = match format {
            SampleFormat::F32 => input::<f32>(&self.input, config, self.input_channel, capture, feeds, health.clone())?,
            SampleFormat::F64 => input::<f64>(&self.input, config, self.input_channel, capture, feeds, health.clone())?,
            SampleFormat::I16 => input::<i16>(&self.input, config, self.input_channel, capture, feeds, health.clone())?,
            SampleFormat::I32 => input::<i32>(&self.input, config, self.input_channel, capture, feeds, health.clone())?,
            SampleFormat::U16 => input::<u16>(&self.input, config, self.input_channel, capture, feeds, health.clone())?,
            _ => return Err("sample format changed after validation".into()),
        };
        timing.push(format!("input driver buffer query: {:?} frames", input_stream.buffer_size()));
        // Activation of every app completed above. Start them only now, alongside the device streams.
        for app in &mut app_captures { app.capture.start()?; }
        for app in &mut app_captures { app.capture.attach()?; }
        // Output emits silence until each queue is primed. Any startup error drops every stream.
        for stream in &streams { stream.play()?; }
        if let Some(stream) = &aux_stream { stream.stream.play()?; }
        input_stream.play()?;
        let mut active = Active { _input: input_stream, aux_input: aux_stream, _outputs: streams, app_captures, apps,
            input_device: self.input, input_config: self.input_config, input_channel: self.input_channel,
            shared_aux, aux_generation, queue_ms: self.queue_ms, plugin_worker, health, outputs: stats,
            controls, vocal_stats, pitch_monitor, plugin_timing, aux_timing, app_timing, timing };
        active.refresh_app_timing();
        Ok(active)
    }
}

struct PlaybackStats {
    mixes: Vec<usize>,
    stats: Arc<OutputStats>,
}

struct ActiveAppCapture {
    capture: ProcessCapture,
    target: ProcessTarget,
    stats: Arc<OutputStats>,
    queue_ms: f64,
    resampler_delay_ms: f64,
}

/// One control-thread read of actual sample maxima; absent optional sources/mixes stay absent.
#[derive(Clone, Copy, Debug)]
pub struct LevelReadings {
    pub mic_input: f32,
    pub aux_input: Option<f32>,
    pub app_inputs: [Option<f32>; MAX_APPS],
    pub vocal: mixlive_effects::Meters,
    pub voice: f32,
    pub outputs: [Option<f32>; MAX_MIXES],
    pub limiters: [Option<LimiterReading>; MAX_MIXES],
    pub pitch: Option<PitchReading>,
}

pub struct Active {
    // Drop devices, then join processing and release native plugin state on the loading thread.
    _input: Stream,
    aux_input: Option<AuxiliaryStream>,
    _outputs: Vec<Stream>,
    app_captures: Vec<ActiveAppCapture>,
    apps: Vec<AppSource>,
    input_device: Device,
    input_config: SupportedStreamConfig,
    input_channel: usize,
    shared_aux: Arc<AtomicU64>,
    aux_generation: u64,
    queue_ms: u32,
    plugin_worker: PluginWorker,
    pub health: Arc<Health>,
    pub controls: Arc<LiveControls>,
    /// One set per physical playback endpoint, shared by its channel routes.
    outputs: Vec<PlaybackStats>,
    pub vocal_stats: Arc<VocalStats>,
    pitch_monitor: PitchMonitor,
    plugin_timing: usize,
    aux_timing: usize,
    app_timing: usize,
    pub timing: Vec<String>,
}

struct InputFeeds {
    shared_aux: Arc<AtomicU64>,
    pitch: PitchTap,
}

fn input<T>(device: &Device, config: StreamConfig, selected: usize, mut capture: CaptureSink, mut feeds: InputFeeds, health: Arc<Health>) -> Result<Stream>
where T: SizedSample + Send + 'static, f32: cpal::FromSample<T> {
    let channels = usize::from(config.channels);
    let error_health = health.clone();
    let mut mono = [0.0_f32; CAPTURE_BLOCK];
    let mut auxiliary = [[0.0_f32; 2]; CAPTURE_BLOCK];
    Ok(device.build_input_stream::<T, _, _>(config, move |data, _| {
        if health.failed() { return; }
        if data.len() % channels != 0 { health.fail(Fault::Input); return; }
        for part in data.chunks(channels * CAPTURE_BLOCK) {
            let frames = part.len() / channels;
            let aux_tag = feeds.shared_aux.load(Ordering::Acquire);
            let map = shared_aux_map(aux_tag);
            for (index, frame) in part.chunks_exact(channels).enumerate() {
                mono[index] = f32::from_sample(frame[selected]);
                auxiliary[index] = if aux_tag == 0 { [0.0; 2] } else { map.read(frame).map(f32::from_sample) };
            }
            feeds.pitch.push(&mono[..frames]);
            capture.push(&mono[..frames], (aux_tag != 0).then_some((aux_tag, &auxiliary[..frames])));
        }
    }, move |_| error_health.fail(Fault::Device), None)?)
}

// An independent AUX device produces only its selected raw source; voice DSP runs elsewhere.
fn input_aux<T>(device: &Device, config: StreamConfig, map: InputMap, mut writer: OutputWriter, health: Arc<Health>, enabled: Arc<AtomicBool>) -> Result<Stream>
where T: SizedSample + Send + 'static, f32: cpal::FromSample<T> {
    let channels = usize::from(config.channels);
    let error_health = health.clone();
    Ok(device.build_input_stream::<T, _, _>(config, move |data, _| {
        if health.failed() || !enabled.load(Ordering::Acquire) { return; }
        if data.len() % channels != 0 { health.fail(Fault::Input); return; }
        for frame in data.chunks_exact(channels) {
            if !enabled.load(Ordering::Acquire) { break; }
            let [left, right] = map.read(frame).map(f32::from_sample);
            writer.push([left, right, 0.0, 0.0, 0.0, 0.0]);
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
    pub fn aux_source(&self) -> Option<&AuxSource> { self.plugin_worker.aux_source() }

    pub fn set_aux_source(&mut self, next: Option<AuxSource>) -> Result<()> {
        if self.aux_source() == next.as_ref() { return Ok(()); }
        if self.health.failed() { return Err("audio path has failed; stop before editing AUX".into()); }
        let endpoint = next.clone().map(|source| {
            let host = cpal::host_from_id(cpal::HostId::Wasapi)?;
            resolve_aux(&host, source, &self.input_device, self.input_config, self.input_channel)
        }).transpose()?;
        let mut generation = self.aux_generation;
        let shared_tag = if let Some(aux) = endpoint.as_ref().filter(|aux| aux.device == self.input_device) {
            generation = generation.checked_add(1).filter(|value| *value <= u64::MAX >> 16)
                .ok_or("shared AUX selection generation is exhausted")?;
            shared_aux_tag(generation, aux.map)
        } else { 0 };
        let prepared = prepare_aux(endpoint, &self.input_device, self.input_config.sample_rate(), self.queue_ms, shared_tag)?;
        // The new independent callback drains its device but leaves its bounded
        // queue empty until the worker accepts the source and opens its gate.
        if let Some(stream) = &prepared.stream { stream.stream.play()?; }
        let result = self.plugin_worker.set_aux_source(next.clone(), prepared.feed, &self.controls);
        if self.aux_source() == next.as_ref() {
            // Any queued pair with the previous tag stays invalid for this binding.
            self.shared_aux.store(shared_tag, Ordering::Release);
            self.aux_generation = generation;
            let previous = std::mem::replace(&mut self.aux_input, prepared.stream);
            self.controls.publish_aux(next.is_some());
            self.timing[self.aux_timing] = prepared.description;
            self.timing[self.plugin_timing] = self.plugin_worker.description.clone();
            drop(previous);
        }
        result?;
        if let Some(error) = self.health.error() { return Err(error.into()); }
        Ok(())
    }

    pub fn apps(&self) -> &[AppSource] { &self.apps }

    /// Prepare an exact new process tree while the current path keeps running.
    /// Existing bindings are compared with this fresh snapshot, never rebound.
    pub fn add_app(&mut self, app: AppSource) -> Result<()> {
        if self.health.failed() { return Err("audio path has failed; stop before adding an application".into()); }
        if self.apps.len() == MAX_APPS { return Err("at most eight application sources are supported".into()); }
        app.validate(self.controls.has_mix(RouteMix::Send), self.controls.has_mix(RouteMix::Broadcast))?;
        let mut proposed = self.apps.clone();
        proposed.push(app.clone());
        let targets = resolve_apps(&proposed, &process_capture::app_catalog())?;
        if self.app_captures.iter().zip(&targets).any(|(current, target)| current.target != *target) {
            return Err("an existing application process changed; remove that source before adding another application".into());
        }
        let target = *targets.last().expect("newly resolved application");
        let map = InputMap::new(&app.channels, 2)?;
        let controls = Arc::new(AppControls::new(&app)?);
        let (writer, reader) = bridge::channel(process_capture::RATE, self.input_config.sample_rate(), self.queue_ms, 1, self.health.clone())?;
        let mut added = ActiveAppCapture {
            capture: ProcessCapture::prepare(app.path.clone(), target, map, writer, self.health.clone())?,
            target, stats: reader.stats.clone(), queue_ms: reader.queue_ms, resampler_delay_ms: reader.resampler_delay_ms,
        };
        // Start succeeds before the source is committed. The client parks before
        // writing, so rejected preparation leaves the running sources untouched.
        added.capture.start()?;
        let result = self.plugin_worker.add_app(reader, controls, &self.controls);
        if self.plugin_worker.app_count() == self.apps.len() + 1 {
            let attached = added.capture.attach();
            self.apps.push(app);
            self.app_captures.push(added);
            self.publish_apps();
            result?;
            attached?;
        } else { result?; }
        Ok(())
    }

    /// Removal uses the actual running slot even if its application has exited.
    pub fn remove_app(&mut self, index: usize) -> Result<()> {
        let stop = self.app_captures.get(index).ok_or("application source is outside the running list")?.capture.stop_signal();
        let result = self.plugin_worker.remove_app(index, stop, &self.controls);
        if self.plugin_worker.app_count() + 1 == self.apps.len() {
            self.apps.remove(index);
            let removed = self.app_captures.remove(index);
            self.publish_apps();
            drop(removed); // join on the control thread after the worker detached its reader
        }
        result?;
        if let Some(error) = self.health.error() { return Err(error.into()); }
        Ok(())
    }

    fn publish_apps(&mut self) {
        self.controls.publish_apps(self.plugin_worker.app_controls());
        self.timing[self.plugin_timing] = self.plugin_worker.description.clone();
        self.refresh_app_timing();
    }

    fn refresh_app_timing(&mut self) {
        let mut text = format!("Current application sources: {}. Original playback is unchanged.\n", self.apps.len());
        for (index, (app, capture)) in self.apps.iter().zip(&self.app_captures).enumerate() {
            text.push_str(&format!("App {}: {}\n  PID {} and descendants; channels {:?}; queue target {:.2} ms + resampler delay {:.2} ms (excludes OS/hardware).\n",
                index + 1, app.path, capture.target.pid, app.channels, capture.queue_ms, capture.resampler_delay_ms));
        }
        self.timing[self.app_timing] = text;
    }

    /// Service plugin/editor requests on the loading/control thread, outside UI borrows.
    /// The caller owns message dispatch. Return whether any editor was dismissed.
    pub fn poll_plugin(&mut self) -> Result<bool> {
        Ok(self.plugin_worker.poll()?)
    }

    pub fn editor_is_open(&self, index: usize) -> bool {
        self.plugin_worker.editor_is_open(index)
    }

    /// Use the existing live instance, never open a separate inspection/edit copy.
    pub fn set_editor_open(&mut self, index: usize, open: bool) -> Result<()> {
        Ok(self.plugin_worker.set_editor_open(index, open)?)
    }

    /// Explicit foreground query; never call native metadata/formatting from a UI render.
    pub fn effect_parameters(&mut self, index: usize) -> Result<Vec<crate::plugin::LiveParameter>> {
        Ok(self.plugin_worker.effect_parameters(index)?)
    }

    /// Edit the running instance; the next audio block consumes the normalized parameter.
    pub fn set_effect_parameter(&mut self, index: usize, id: u32, normalized: f64) -> Result<Vec<crate::plugin::LiveParameter>> {
        Ok(self.plugin_worker.set_effect_parameter(index, id, normalized)?)
    }

    /// Resolved identity and bypass of the current slots, without native state reads.
    pub fn effects(&self) -> Vec<crate::plugin::EffectSpec> {
        self.plugin_worker.effects()
    }

    /// Prepare and commit one running-chain edit outside any mutable UI borrow.
    /// Publish the acknowledged order even if later native retirement reports an error.
    pub fn edit_chain(&mut self, edit: crate::plugin::ChainEdit) -> Result<()> {
        let result = self.plugin_worker.edit_chain(edit);
        self.controls.replace_effects(self.plugin_worker.effect_flags());
        self.timing[self.plugin_timing] = self.plugin_worker.description.clone();
        Ok(result?)
    }

    pub fn pitch_config(&self) -> PitchConfig { self.plugin_worker.pitch_config() }

    /// Apply one validated pitch configuration and any changed PDC at a block boundary.
    /// Query authoritative state after an error too: a later audio fault may follow its commit.
    pub fn set_pitch_config(&mut self, config: PitchConfig) -> Result<()> {
        let result = self.plugin_worker.set_pitch_config(config);
        self.timing[self.plugin_timing] = self.plugin_worker.description.clone();
        Ok(result?)
    }

    pub fn vocal_settings(&self) -> VocalSettings { self.plugin_worker.vocal_settings() }

    /// Commit only preset and wet delay times; independent live targets stay intact.
    /// Publish acknowledged settings even if a later audio fault follows the change.
    pub fn set_vocal_settings(&mut self, settings: VocalSettings) -> Result<()> {
        let result = self.plugin_worker.set_vocal_settings(settings, self.controls.vocal_config());
        self.controls.publish_vocal_settings(self.plugin_worker.vocal_settings());
        self.timing[self.plugin_timing] = self.plugin_worker.description.clone();
        Ok(result?)
    }

    /// Explicit normal completion. Stop all devices before joining/snapshotting the plugin.
    /// Errors and implicit Drop still tear down everything, but never save plugin state.
    pub fn finish(self, state_out: Option<&std::path::Path>) -> Result<()> {
        let (worker, health) = self.stop_devices();
        worker.finish(state_out)?;
        if let Some(error) = health.error() { return Err(error.into()); }
        Ok(())
    }

    /// Normal explicit stop with a complete Scene snapshot, including every native slot.
    /// No file is published here; failure tears down the route and yields no partial Scene.
    pub fn finish_scene(self, mut route: RouteOptions) -> Result<crate::scene::AudioScene> {
        route.set_pitch_config(self.pitch_config());
        route.apps = self.apps.clone();
        route.aux_input = self.aux_source().map(|source| source.input.clone());
        if let Some(source) = self.aux_source() { route.aux_channels.clone_from(&source.channels); }
        let settings = self.controls.snapshot_into(&mut route);
        let (worker, health) = self.stop_devices();
        settings?;
        if let Some(error) = health.error() { return Err(error.into()); }
        let effects = worker.snapshot()?.effects;
        if let Some(error) = health.error() { return Err(error.into()); }
        crate::scene::AudioScene::new(route, effects)
    }

    fn stop_devices(self) -> (PluginWorker, Arc<Health>) {
        let Self { _input, aux_input, _outputs, app_captures, plugin_worker, health, .. } = self;
        drop(_input);
        drop(aux_input);
        drop(_outputs);
        drop(app_captures);
        (plugin_worker, health)
    }

    /// Single consumer on the control thread. Peak intervals are independent of callback blocks.
    pub fn take_levels(&mut self) -> LevelReadings {
        let pitch = self.pitch_config();
        let apps = self.controls.app_controls();
        let mut outputs = [None; MAX_MIXES];
        let mut limiters = [None; MAX_MIXES];
        for endpoint in &self.outputs {
            for ((&mix, peak), limiter) in endpoint.mixes.iter().zip(endpoint.stats.take_peaks()).zip(endpoint.stats.take_limiters()) {
                outputs[mix] = Some(peak);
                limiters[mix] = Some(limiter);
            }
        }
        LevelReadings {
            mic_input: f32::from_bits(self.health.input_peak.swap(0, Ordering::Relaxed)),
            aux_input: self.aux_source().is_some().then(|| f32::from_bits(self.health.aux_peak.swap(0, Ordering::Relaxed))),
            app_inputs: std::array::from_fn(|index| apps.get(index).map(|app| app.take_peak())),
            vocal: self.vocal_stats.take(),
            voice: f32::from_bits(self.health.voice_peak.swap(0, Ordering::Relaxed)),
            outputs, limiters, pitch: self.pitch_monitor.take(pitch.key, pitch.scale),
        }
    }

    /// Reuse the same readings for the diagnostic text; never drain the meter twice.
    pub fn report(&self, levels: &LevelReadings) -> String {
        let vocal = levels.vocal;
        let mut text = format!("captured={} input_peak={:.3} vocal_peak={:.3} voice_peak={:.3} comp_gr={:.1}dB deess_gr={:.1}dB", self.health.captured_frames.load(Ordering::Relaxed), levels.mic_input, vocal.output_peak, levels.voice, vocal.compression_db, vocal.deessing_db);
        if let Some(pitch) = levels.pitch {
            text.push_str(&format!(" pitch={}@{:.2}Hz({:+.1}c) target={}({:+.1}c) clarity={:.2}", pitch.note_name(), pitch.frequency_hz, pitch.cents, pitch.target_note_name(), pitch.target_cents, pitch.clarity));
        } else { text.push_str(" pitch=unvoiced"); }
        if let Some(aux) = levels.aux_input {
            text.push_str(&format!(" aux_peak={aux:.3}"));
            if let Some(aux) = &self.aux_input {
                let stats = &aux.stats;
                text.push_str(&format!(" aux_queue={} frames aux_correction={}ppm aux_underruns={}",
                    stats.queue_frames.load(Ordering::Relaxed), stats.correction_ppm.load(Ordering::Relaxed),
                    stats.underruns.load(Ordering::Relaxed)));
            }
        }
        for (index, app) in self.app_captures.iter().enumerate() {
            let stats = &app.stats;
            let peak = levels.app_inputs[index].unwrap_or(0.0);
            text.push_str(&format!(" | app{}: peak={peak:.3} queue={} frames correction={}ppm underruns={}",
                index + 1, stats.queue_frames.load(Ordering::Relaxed), stats.correction_ppm.load(Ordering::Relaxed), stats.underruns.load(Ordering::Relaxed)));
        }
        for (index, endpoint) in self.outputs.iter().enumerate() {
            let stats = &endpoint.stats;
            let peak = endpoint.mixes.iter().filter_map(|&mix| levels.outputs[mix]).fold(0.0_f32, f32::max);
            text.push_str(&format!(" | stream{index}: peak={peak:.3} queue={} frames correction={}ppm underruns={} limited={}", stats.queue_frames.load(Ordering::Relaxed), stats.correction_ppm.load(Ordering::Relaxed), stats.underruns.load(Ordering::Relaxed), stats.limited_frames.load(Ordering::Relaxed)));
            for &mix in &endpoint.mixes {
                if let Some(limiter) = levels.limiters[mix] {
                    text.push_str(&format!(" mix{mix}_pre_limit={:.3} limiter_gr={:.1}dB", limiter.input_peak, limiter.reduction_db));
                }
            }
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_aux_tag_carries_one_channel_pair_and_a_distinct_generation() {
        let frame: Vec<_> = (0..256).map(|index| index as f32).collect();
        for channels in [vec![1], vec![256], vec![256, 2], vec![3, 4]] {
            let map = InputMap::new(&channels, 256).unwrap();
            let first = shared_aux_tag(1, map);
            let next = shared_aux_tag(2, map);
            assert_ne!(first, 0);
            assert_ne!(first, next);
            assert_eq!(shared_aux_map(first), map);
            assert_eq!(shared_aux_map(next).read(&frame), map.read(&frame));
        }
    }
}
