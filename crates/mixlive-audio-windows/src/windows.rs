//! WASAPI adapter. Endpoint selection, stream setup and text formatting stay off callbacks.
use std::{error::Error, sync::{Arc, atomic::Ordering}};
use cpal::{traits::{DeviceTrait, HostTrait, StreamTrait}, Device, Host, Sample, SampleFormat, SizedSample, Stream, StreamConfig, SupportedStreamConfig};
use mixlive_effects::VocalChain;
use crate::devices::{AppInfo, DeviceCatalog, DeviceConfig, DeviceInfo, ProcessTarget, resolve_apps};
use crate::process_capture::{self, ProcessCapture};
use crate::{bridge::{self, Fault, Health, OutputReader, OutputStats, OutputWriter}, capture::{Capture, VocalStats, CAPTURE_BLOCK}, config::{AuxDestination, InputMap, OutputMap, RouteOptions, MAX_APPS, MAX_MIXES, output_sample}, control::{LiveControls, PlaybackGains}, plugin::{ResolvedPlugins, native::{self, CaptureSink, PluginWorker}}};

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

struct AuxiliaryEndpoint { device: Device, config: SupportedStreamConfig, map: InputMap }

// Shared input channels need neither a second stream nor another clock queue.
enum AuxFeed { Disabled, Shared(InputMap), Independent(Box<OutputReader>) }

pub struct Prepared {
    input: Device,
    input_config: SupportedStreamConfig,
    input_channel: usize,
    auxiliary: Option<AuxiliaryEndpoint>,
    apps: Vec<(String, ProcessTarget, InputMap, f32)>,
    aux_mix: Option<(f32, AuxDestination)>,
    vocal: VocalChain,
    output_gain_db: f32,
    mix_gains_db: [f32; MAX_MIXES],
    queue_ms: u32,
    controls: Arc<LiveControls>,
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
        let apps = if options.apps.is_empty() { Vec::new() } else {
            let targets = resolve_apps(&options.apps, &process_capture::app_catalog())?;
            options.apps.iter().zip(targets).map(|(app, target)| -> Result<_> {
                Ok((app.path.clone(), target, InputMap::new(&app.channels, 2)?, app.gain_db))
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
            auxiliary, apps, aux_mix: options.has_aux().then_some((options.aux_gain_db, options.aux_to)),
            output_gain_db: options.output_gain_db, mix_gains_db: [options.monitor_gain_db, options.send_gain_db, options.broadcast_gain_db],
            queue_ms: options.queue_ms, controls: Arc::new(LiveControls::from_route(options)?), outputs })
    }

    pub fn describe(&self) -> String {
        let mut text = format!("WASAPI input: {}\n  {:?}; channel {}\nvocal: {:?}\ninitial master attenuation: {} dB\nplayback streams: {}\n", self.input, self.input_config, self.input_channel + 1, self.vocal.config(), self.output_gain_db, self.outputs.len());
        for (index, endpoint) in self.outputs.iter().enumerate() {
            text.push_str(&format!("stream {index}: {}\n  {:?}; one queue/resampler for {} mix(es)\n", endpoint.device, endpoint.config, endpoint.routes.len()));
            for (mix, map) in &endpoint.routes {
                let gain = self.mix_gains_db[*mix];
                text.push_str(&format!("  mix {mix}: map {map:?} (zero-based); initial mix attenuation {gain} dB\n"));
            }
        }
        if let Some(aux) = &self.auxiliary {
            text.push_str(&format!("AUX input: {}\n  {:?}; channels {},{}; mix settings {:?}\n  {}\n",
                aux.device, aux.config, aux.map.left + 1, aux.map.right + 1, self.aux_mix,
                if aux.device == self.input { "shares the microphone capture stream and clock" }
                else { "separate capture stream; buffered/resampled to the microphone clock (additional AUX delay)" }));
            text.push_str("AUX must carry only the intended MR/line audio, not MixLive output or call return. No app filtering or external feedback detection. Enable output/both only when AUX is not already heard through another path.\n");
        }
        for (index, (path, target, map, gain)) in self.apps.iter().enumerate() {
            text.push_str(&format!("App {}: {path}\n  PID {} and descendants; 48000 Hz float32 stereo, channels {},{}; trim {gain} dB\n  Independent clock bridge. Original application playback is unchanged; no automatic rebind or whole-desktop capture.\n",
                index + 1, target.pid, map.left + 1, map.right + 1));
        }
        text.push_str(&format!("Initial mixer: {}\n", self.controls.describe()));
        text.push_str("Vocal preset, reverb pre-delay and device bindings are fixed for this run. --interactive enables live mixer controls (5 ms ramps) and vocal trim/strength/threshold/reverb blend/decay targets (10 ms ramps). Output mute acts after MixLive queues, not on audio already sent to the OS or on hardware direct monitoring.\nNo automatic makeup gain or lookahead buffer; IIR phase/group delay still exists. Check validates configuration, not device operation or sound quality. --vst3 is an optional in-process development path; ASIO and driver installation are not implemented.\n");
        text
    }

    pub fn start(self, plugin: &ResolvedPlugins) -> Result<Active> {
        plugin.validate()?;
        let health = Arc::new(Health::default());
        let controls = self.controls;
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
        // Each app retains its own clock/queue. Prepare all clients before starting any of them.
        let mut app_readers = Vec::with_capacity(self.apps.len());
        let mut app_clock_stats = Vec::with_capacity(self.apps.len());
        let mut app_captures = Vec::with_capacity(self.apps.len());
        let mut app_gains = Vec::with_capacity(self.apps.len());
        for (index, (path, target, map, gain)) in self.apps.into_iter().enumerate() {
            let (writer, reader) = bridge::channel(process_capture::RATE, self.input_config.sample_rate(), self.queue_ms, 1, health.clone())?;
            timing.push(format!("App {} boundary: queue target {:.2} ms + resampler delay {:.2} ms (excludes OS/hardware)", index + 1, reader.queue_ms, reader.resampler_delay_ms));
            app_clock_stats.push(reader.stats.clone());
            app_captures.push(ProcessCapture::prepare(path, target, map, writer, health.clone())?);
            app_readers.push(reader);
            app_gains.push(gain);
        }
        let capture = Capture::new(self.vocal, self.input_config.sample_rate(), self.aux_mix, &app_gains, writers, health.clone(), controls.clone())?;
        let vocal_stats = capture.vocal_stats.clone();
        let (capture, plugin_worker) = native::connect(capture, plugin, self.input_config.sample_rate(), self.queue_ms, self.aux_mix.is_some(), health.clone())?;
        if let Some(worker) = &plugin_worker { timing.push(worker.description.clone()); }
        let format = self.input_config.sample_format();
        let config = self.input_config.config();
        let feeds = InputFeeds { aux: aux_feed, apps: app_readers };
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
        for app in &mut app_captures { app.start()?; }
        // Output emits silence until each queue is primed. Any startup error drops every stream.
        for stream in &streams { stream.play()?; }
        if let Some(stream) = &aux_stream { stream.play()?; }
        input_stream.play()?;
        Ok(Active { _input: input_stream, _aux_input: aux_stream, _outputs: streams, _app_captures: app_captures, plugin_worker, health, outputs: stats,
            controls, aux_enabled: self.aux_mix.is_some(), aux_clock_stats, app_clock_stats, vocal_stats, timing })
    }
}

struct PlaybackStats {
    mixes: Vec<usize>,
    stats: Arc<OutputStats>,
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
}

pub struct Active {
    // Drop devices, then join processing and release native plugin state on the loading thread.
    _input: Stream,
    _aux_input: Option<Stream>,
    _outputs: Vec<Stream>,
    _app_captures: Vec<ProcessCapture>,
    plugin_worker: Option<PluginWorker>,
    pub health: Arc<Health>,
    pub controls: Arc<LiveControls>,
    /// One set per physical playback endpoint, shared by its channel routes.
    outputs: Vec<PlaybackStats>,
    aux_enabled: bool,
    aux_clock_stats: Option<Arc<OutputStats>>,
    app_clock_stats: Vec<Arc<OutputStats>>,
    pub vocal_stats: Arc<VocalStats>,
    pub timing: Vec<String>,
}

struct InputFeeds {
    aux: AuxFeed,
    apps: Vec<OutputReader>,
}

fn input<T>(device: &Device, config: StreamConfig, selected: usize, mut capture: CaptureSink, mut feeds: InputFeeds, health: Arc<Health>) -> Result<Stream>
where T: SizedSample + Send + 'static, f32: cpal::FromSample<T> {
    let channels = usize::from(config.channels);
    let error_health = health.clone();
    let mut mono = [0.0_f32; CAPTURE_BLOCK];
    let mut auxiliary = [[0.0_f32; 2]; CAPTURE_BLOCK];
    let has_aux = !matches!(&feeds.aux, AuxFeed::Disabled);
    let app_count = feeds.apps.len();
    let mut applications = [[[0.0_f32; 2]; CAPTURE_BLOCK]; MAX_APPS];
    Ok(device.build_input_stream::<T, _, _>(config, move |data, _| {
        if health.failed() { return; }
        if data.len() % channels != 0 { health.fail(Fault::Input); return; }
        for part in data.chunks(channels * CAPTURE_BLOCK) {
            let frames = part.len() / channels;
            for (index, frame) in part.chunks_exact(channels).enumerate() {
                mono[index] = f32::from_sample(frame[selected]);
                auxiliary[index] = match &mut feeds.aux {
                    AuxFeed::Disabled => [0.0; 2],
                    AuxFeed::Shared(map) => map.read(frame).map(f32::from_sample),
                    AuxFeed::Independent(reader) => reader.next_source_frame(),
                };
                for (buffer, reader) in applications.iter_mut().zip(&mut feeds.apps) {
                    buffer[index] = reader.next_source_frame();
                }
            }
            let apps: [&[[f32; 2]]; MAX_APPS] = std::array::from_fn(|slot| &applications[slot][..frames]);
            capture.push(&mono[..frames], has_aux.then_some(&auxiliary[..frames]), &apps[..app_count]);
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
    /// Service plugin/editor requests on the loading/control thread, outside UI borrows.
    /// The caller owns message dispatch. Return whether any editor was dismissed.
    pub fn poll_plugin(&mut self) -> Result<bool> {
        match &mut self.plugin_worker {
            Some(worker) => Ok(worker.poll()?),
            None => Ok(false),
        }
    }

    pub fn editor_is_open(&self, index: usize) -> bool {
        self.plugin_worker.as_ref().is_some_and(|worker| worker.editor_is_open(index))
    }

    /// Use the existing live instance, never open a separate inspection/edit copy.
    pub fn set_editor_open(&mut self, index: usize, open: bool) -> Result<()> {
        let worker = self.plugin_worker.as_mut().ok_or("no active VST3 chain")?;
        Ok(worker.set_editor_open(index, open)?)
    }

    /// Explicit normal completion. Stop all devices before joining/snapshotting the plugin.
    /// Errors and implicit Drop still tear down everything, but never save plugin state.
    pub fn finish(self, state_out: Option<&std::path::Path>) -> Result<()> {
        let (worker, health) = self.stop_devices();
        if let Some(worker) = worker { worker.finish(state_out)?; }
        else if state_out.is_some() { return Err("no live VST3 instance to save".into()); }
        if let Some(error) = health.error() { return Err(error.into()); }
        Ok(())
    }

    /// Normal explicit stop with a complete Scene snapshot, including every native slot.
    /// No file is published here; failure tears down the route and yields no partial Scene.
    pub fn finish_scene(self, mut route: RouteOptions) -> Result<crate::scene::AudioScene> {
        let settings = self.controls.snapshot_into(&mut route);
        let (worker, health) = self.stop_devices();
        settings?;
        if let Some(error) = health.error() { return Err(error.into()); }
        let effects = match worker {
            Some(worker) => worker.snapshot()?.effects,
            None => Vec::new(),
        };
        if let Some(error) = health.error() { return Err(error.into()); }
        crate::scene::AudioScene::new(route, effects)
    }

    fn stop_devices(self) -> (Option<PluginWorker>, Arc<Health>) {
        let Self { _input, _aux_input, _outputs, _app_captures, plugin_worker, health, .. } = self;
        drop(_input);
        drop(_aux_input);
        drop(_outputs);
        drop(_app_captures);
        (plugin_worker, health)
    }

    /// Single consumer on the control thread. Peak intervals are independent of callback blocks.
    pub fn take_levels(&self) -> LevelReadings {
        let mut outputs = [None; MAX_MIXES];
        for endpoint in &self.outputs {
            for (&mix, peak) in endpoint.mixes.iter().zip(endpoint.stats.take_peaks()) {
                outputs[mix] = Some(peak);
            }
        }
        LevelReadings {
            mic_input: f32::from_bits(self.health.input_peak.swap(0, Ordering::Relaxed)),
            aux_input: self.aux_enabled.then(|| f32::from_bits(self.health.aux_peak.swap(0, Ordering::Relaxed))),
            app_inputs: std::array::from_fn(|index| (index < self.controls.app_count())
                .then(|| f32::from_bits(self.health.app_peaks[index].swap(0, Ordering::Relaxed)))),
            vocal: self.vocal_stats.take(),
            voice: f32::from_bits(self.health.voice_peak.swap(0, Ordering::Relaxed)),
            outputs,
        }
    }

    /// Reuse the same readings for the diagnostic text; never drain the meter twice.
    pub fn report(&self, levels: &LevelReadings) -> String {
        let vocal = levels.vocal;
        let mut text = format!("captured={} input_peak={:.3} vocal_peak={:.3} voice_peak={:.3} comp_gr={:.1}dB deess_gr={:.1}dB", self.health.captured_frames.load(Ordering::Relaxed), levels.mic_input, vocal.output_peak, levels.voice, vocal.compression_db, vocal.deessing_db);
        if let Some(aux) = levels.aux_input {
            text.push_str(&format!(" aux_peak={aux:.3}"));
            if let Some(stats) = &self.aux_clock_stats {
                text.push_str(&format!(" aux_queue={} frames aux_correction={}ppm aux_underruns={}",
                    stats.queue_frames.load(Ordering::Relaxed), stats.correction_ppm.load(Ordering::Relaxed),
                    stats.underruns.load(Ordering::Relaxed)));
            }
        }
        for (index, stats) in self.app_clock_stats.iter().enumerate() {
            let peak = levels.app_inputs[index].unwrap_or(0.0);
            text.push_str(&format!(" | app{}: peak={peak:.3} queue={} frames correction={}ppm underruns={}",
                index + 1, stats.queue_frames.load(Ordering::Relaxed), stats.correction_ppm.load(Ordering::Relaxed), stats.underruns.load(Ordering::Relaxed)));
        }
        for (index, endpoint) in self.outputs.iter().enumerate() {
            let stats = &endpoint.stats;
            let peak = endpoint.mixes.iter().filter_map(|&mix| levels.outputs[mix]).fold(0.0_f32, f32::max);
            text.push_str(&format!(" | stream{index}: peak={peak:.3} queue={} frames correction={}ppm underruns={} clipped={}", stats.queue_frames.load(Ordering::Relaxed), stats.correction_ppm.load(Ordering::Relaxed), stats.underruns.load(Ordering::Relaxed), stats.clipped_frames.load(Ordering::Relaxed)));
        }
        text
    }
}
