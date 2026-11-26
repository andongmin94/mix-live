//! One worker for the ordered vocal chain; no native plugin calls in device callbacks.
use super::{ChainEdit, ChainFile, EffectSpec, LiveParameter, ResolvedPlugins};
use crate::{bridge::{Fault, Health, OutputReader}, capture::{Capture, CaptureOutput, PreparedOutputChange, PreparedSourceChange, PreparedPitchChange, PreparedVoiceDelay, VoiceDelayLayout, CAPTURE_BLOCK}, config::{AuxSource, OutputBinding, PitchConfig, PitchScale, MAX_APPS, validate_output_bindings}, control::{AppControls, GainRamp, LiveControls, RouteMix}};
use mixlive_effects::{PreparedVocalChange, VocalConfig, VocalSettings};
use mixlive_vst3::editor::flush_editor_changes;
use rtrb::{Consumer, Producer, RingBuffer};
use std::{path::Path, sync::{Arc, Mutex, TryLockError, atomic::{AtomicBool, Ordering}}, thread::{self, JoinHandle}, time::Duration};
use vst3_host::{Parameter, Plugin, PluginWindow, Vst3Host, audio::BusAudioBuffers, plugin::ProcessMode};

// Device samples travel together; app clock readers belong to the processing worker,
// so a live app edit never changes the meaning of samples already in this queue.
struct CaptureFrame {
    mic: f32,
    aux: [f32; 2],
    aux_tag: u64,
}

pub(crate) struct QueuedInput {
    writer: Producer<CaptureFrame>,
    health: Arc<Health>,
}
impl QueuedInput {
    fn push(&mut self, mic: &[f32], aux: Option<(u64, &[[f32; 2]])>) {
        if self.health.failed() { return; }
        if aux.is_some_and(|(_, frames)| frames.len() != mic.len()) {
            self.health.fail(Fault::Input); return;
        }
        for (i, &voice) in mic.iter().enumerate() {
            let (aux_tag, music) = aux.map_or((0, [0.0; 2]), |(tag, frames)| (tag, frames[i]));
            let frame = CaptureFrame { mic: voice, aux: music, aux_tag };
            // Obsolete shared AUX pairs are ignored by the worker. Validate only
            // the selected current source there, so stale NaNs cannot fail a new binding.
            if !voice.is_finite() {
                self.health.fail(Fault::Input); return;
            }
            if self.writer.push(frame).is_err() {
                self.health.fail(Fault::Overflow); return;
            }
        }
    }
}

pub(crate) struct CaptureSink(QueuedInput);
impl CaptureSink {
    pub fn push(&mut self, mic: &[f32], aux: Option<(u64, &[[f32; 2]])>) {
        self.0.push(mic, aux);
    }
}

pub(crate) enum AuxFeed {
    Disabled,
    Shared(u64),
    Independent { reader: Box<OutputReader>, health: Arc<Health>, enabled: Arc<AtomicBool> },
}
impl AuxFeed {
    fn has_aux(&self) -> bool { !matches!(self, Self::Disabled) }

    fn error(&self) -> Option<&'static str> {
        match self { Self::Independent { health, .. } => health.error(), _ => None }
    }

    fn enable(&self, active: bool) {
        if let Self::Independent { enabled, .. } = self { enabled.store(active, Ordering::Release); }
    }

    fn propagate_health(&self, route: &Health) {
        if let Self::Independent { health, .. } = self { health.propagate_to(route); }
    }

    fn next_frame(&mut self, frame: &CaptureFrame) -> [f32; 2] {
        match self {
            Self::Shared(tag) if *tag == frame.aux_tag => frame.aux,
            Self::Independent { reader, .. } => reader.next_source_frame(),
            _ => [0.0; 2],
        }
    }
}

pub(crate) struct SourceFeeds {
    pub aux: AuxFeed,
    pub aux_source: Option<AuxSource>,
    pub apps: Vec<OutputReader>,
    pub output_bindings: Vec<OutputBinding>,
}

struct Slot {
    plugin: Arc<Mutex<Plugin>>,
    editor: Option<PluginWindow>,
    bypassed: Arc<AtomicBool>,
    // Initialized only on explicit panel access. Feedback replaces values, never records history.
    parameters: Option<Vec<CachedParameter>>,
    // Native metadata copied on the loading thread; never query it from UI render.
    spec: EffectSpec,
    name: String,
    latency: u32,
}

struct CachedParameter {
    info: Parameter,
    // Feedback may come from the processor and need not be mirrored to its controller.
    feedback: bool,
}

// VST3 ParameterInfo::ParameterFlags::kIsHidden (pinned vst3 0.3.0).
const PARAMETER_HIDDEN: u32 = 1 << 4;

fn update_parameter_feedback(parameters: &mut Option<Vec<CachedParameter>>, changes: Vec<(u32, f64)>) {
    if let Some(parameters) = parameters {
        for (id, value) in changes {
            if let Some(parameter) = parameters.iter_mut().find(|parameter| parameter.info.id == id) {
                parameter.info.value = value;
                parameter.feedback = true;
            }
        }
    }
}

fn parameter_snapshot(plugin: &Plugin, parameter: &Parameter) -> Result<LiveParameter, String> {
    if !(0.0..=1.0).contains(&parameter.value) {
        return Err(format!("VST3 parameter {} reported an invalid normalized value", parameter.id));
    }
    Ok(LiveParameter { id: parameter.id, name: parameter.name.clone(), unit: parameter.unit.clone(),
        normalized: parameter.value, display: plugin.format_parameter(parameter.id, parameter.value).map_err(|e| e.to_string())?,
        step_count: parameter.step_count.max(0) as u32, read_only: parameter.is_read_only })
}

impl Slot {
    fn new(plugin: Plugin, bypassed: Arc<AtomicBool>, latency: u32) -> Self {
        let spec = EffectSpec { plugin: plugin.info().path.clone(), class_id: Some(plugin.info().uid.clone()),
            state: None, bypassed: bypassed.load(Ordering::Relaxed) };
        let name = plugin.info().name.clone();
        Self { plugin: Arc::new(Mutex::new(plugin)), editor: None, bypassed, parameters: None, spec, name, latency }
    }

    // Loading/control thread only, outside GPUI borrows. Native calls still share the
    // existing worker mutex; this is not a lock-free or hard-real-time host interface.
    fn parameter_snapshot(&mut self, edit: Option<(u32, f64)>) -> Result<Vec<LiveParameter>, String> {
        let mut plugin = self.plugin.lock().map_err(|_| "VST3 processing state was poisoned")?;
        if self.parameters.is_none() {
            self.parameters = Some(plugin.get_parameters().map_err(|e| e.to_string())?.into_iter()
                .map(|info| CachedParameter { info, feedback: false }).collect());
        }
        // Processor/editor feedback was already consumed by DSP. Never replay it as an edit.
        update_parameter_feedback(&mut self.parameters, plugin.get_parameter_changes());
        let parameters = self.parameters.as_mut().expect("parameter metadata initialized above");
        // Explicit Refresh also reads controller-only values (e.g. editor instrumentation).
        // Once a parameter has feedback, its latest reported value wins over a potentially
        // stale controller getter. No continuous meter sampling or event history is inferred.
        for parameter in parameters.iter_mut().filter(|parameter| !parameter.feedback) {
            parameter.info.value = plugin.get_parameter(parameter.info.id).map_err(|e| e.to_string())?;
        }
        if let Some((id, value)) = edit {
            let parameter = parameters.iter_mut().find(|parameter| parameter.info.id == id && parameter.info.flags & PARAMETER_HIDDEN == 0)
                .ok_or("VST3 parameter ID is not in the visible parameter list")?;
            let value = parameter_snapshot(&plugin, &parameter.info)?.validate_edit(value)?;
            // One explicit UI apply, not a parameter batch. The upstream pending queue is
            // bounded at 4096 and can silently drop on overflow; no delivery guarantee is added.
            plugin.set_parameter(id, value).map_err(|e| e.to_string())?;
            parameter.info.value = plugin.get_parameter(id).map_err(|e| e.to_string())?;
            parameter.feedback = false;
        }
        parameters.iter().filter(|parameter| parameter.info.flags & PARAMETER_HIDDEN == 0)
            .map(|parameter| parameter_snapshot(&plugin, &parameter.info)).collect()
    }
}

// Always advance the aligned dry history, even when listening to the wet path.
// No processing instance, buffer allocation or topology change during a comparison.
struct DryBlend {
    delay: Vec<[f32; 2]>,
    cursor: usize,
    samples: [[f32; 2]; CAPTURE_BLOCK],
    ramp: GainRamp,
}
impl DryBlend {
    fn new(rate: u32, latency: usize, bypassed: bool) -> Self {
        Self { delay: vec![[0.0; 2]; latency], cursor: 0, samples: [[0.0; 2]; CAPTURE_BLOCK],
            ramp: GainRamp::new(rate, if bypassed { 1.0 } else { 0.0 }) }
    }

    fn prepare(&mut self, left: &[f32], right: &[f32]) {
        for (sample, (&l, &r)) in self.samples.iter_mut().zip(left.iter().zip(right)) {
            let input = [l, r];
            if self.delay.is_empty() {
                *sample = input;
            } else {
                *sample = std::mem::replace(&mut self.delay[self.cursor], input);
                self.cursor = (self.cursor + 1) % self.delay.len();
            }
        }
    }

    fn mix(&mut self, left: &mut [f32], right: &mut [f32], bypassed: bool) {
        self.ramp.set(if bypassed { 1.0 } else { 0.0 });
        for ((l, r), dry) in left.iter_mut().zip(right).zip(&self.samples) {
            let gain = self.ramp.next_gain();
            if gain == 1.0 {
                *l = dry[0]; *r = dry[1];
            } else if gain != 0.0 {
                *l = *l * (1.0 - gain) + dry[0] * gain;
                *r = *r * (1.0 - gain) + dry[1] * gain;
            }
        }
    }
}

struct Processor {
    plugin: Arc<Mutex<Plugin>>,
    buffers: BusAudioBuffers,
    bypassed: Arc<AtomicBool>,
    dry: DryBlend,
}
impl Processor {
    fn new(plugin: Arc<Mutex<Plugin>>, buffers: BusAudioBuffers, bypassed: Arc<AtomicBool>,
        rate: u32, latency: u32) -> Self {
        let dry = DryBlend::new(rate, latency as usize, bypassed.load(Ordering::Relaxed));
        Self { plugin, buffers, bypassed, dry }
    }
}

// One prepared edit is in flight at a time. Both vectors and the replacement delay
// bank are allocated on the loading thread. The worker only moves/swaps ownership.
struct ChainChange {
    processors: Vec<Option<Processor>>,
    retained: Vec<(usize, usize)>,
    delays: Option<PreparedVoiceDelay>,
}
impl ChainChange {
    fn apply(&mut self, capture: &mut Capture, processors: &mut Vec<Option<Processor>>) {
        for &(from, to) in &self.retained { self.processors[to] = processors[from].take(); }
        std::mem::swap(&mut self.processors, processors);
        if let Some(delays) = &mut self.delays { capture.swap_voice_delay(delays); }
    }
}

struct AppChange {
    capture: Box<PreparedSourceChange>,
    readers: Vec<Option<OutputReader>>,
    retained: Vec<(usize, usize)>,
    stop_removed: Option<Arc<AtomicBool>>,
}
impl AppChange {
    fn apply(&mut self, capture: &mut Capture, readers: &mut Vec<Option<OutputReader>>) {
        // Stop the retired producer at this same boundary. It must not fill an
        // abandoned queue while the control thread receives and joins its owner.
        if let Some(stop) = &self.stop_removed { stop.store(true, Ordering::Relaxed); }
        for &(from, to) in &self.retained { self.readers[to] = readers[from].take(); }
        std::mem::swap(&mut self.readers, readers);
        capture.apply_source_change(&mut self.capture);
    }
}

struct AuxChange {
    capture: Box<PreparedSourceChange>,
    feed: AuxFeed,
    applied: bool,
}
impl AuxChange {
    fn apply(&mut self, capture: &mut Capture, feed: &mut AuxFeed) {
        if self.feed.error().is_some() { return; }
        feed.enable(false);
        std::mem::swap(feed, &mut self.feed);
        capture.apply_source_change(&mut self.capture);
        feed.enable(true);
        self.applied = true;
    }
}

// Explicit chain, pitch and vocal edits share one serialized boundary handoff.
// Large DSP plans are boxed on the control thread; those same boxes bring retired
// processing storage back for control-thread drop.
enum ProcessingChange {
    Chain(ChainChange),
    Pitch { pitch: Box<PreparedPitchChange>, delays: Option<PreparedVoiceDelay> },
    Vocal(Box<PreparedVocalChange>),
    Apps(AppChange),
    Aux(AuxChange),
    Outputs(PreparedOutputChange),
}
impl ProcessingChange {
    fn apply(&mut self, capture: &mut Capture, processors: &mut Vec<Option<Processor>>, apps: &mut Vec<Option<OutputReader>>, aux: &mut AuxFeed) {
        match self {
            Self::Chain(change) => change.apply(capture, processors),
            Self::Pitch { pitch, delays } => {
                capture.apply_pitch_change(pitch);
                if let Some(delays) = delays { capture.swap_voice_delay(delays); }
            }
            Self::Vocal(change) => capture.apply_vocal_change(change),
            Self::Apps(change) => change.apply(capture, apps),
            Self::Aux(change) => change.apply(capture, aux),
            Self::Outputs(change) => capture.apply_output_change(change),
        }
    }
}

struct ChainControl {
    requests: Producer<ProcessingChange>,
    retired: Consumer<ProcessingChange>,
    layout: VoiceDelayLayout,
    rate: u32,
    capacity: usize,
    latency: u64,
    pitch_latency: usize,
    compensation: usize,
}

// The join result returns every owned audio allocation to the loading thread,
// including an unapplied request when the route stops before its boundary.
struct ProcessingState {
    capture: Capture,
    processors: Vec<Option<Processor>>,
    apps: Vec<Option<OutputReader>>,
    aux: AuxFeed,
    input: Consumer<CaptureFrame>,
    requests: Consumer<ProcessingChange>,
    retired: Producer<ProcessingChange>,
    unreturned: Option<ProcessingChange>,
}

/// Loaded/polled/dropped on the main thread. Join ONE processing thread before native teardown.
/// Native crashes/hangs are not isolated; editor/control calls still share the plugin mutexes.
pub(crate) struct PluginWorker {
    thread: Option<JoinHandle<ProcessingState>>,
    stop: Arc<AtomicBool>,
    slots: Vec<Slot>,
    stopped: bool,
    health: Arc<Health>,
    chain: Option<ChainControl>,
    pitch_config: PitchConfig,
    vocal_settings: VocalSettings,
    apps: Vec<Arc<AppControls>>,
    aux_source: Option<AuxSource>,
    output_bindings: Vec<OutputBinding>,
    output_groups: Vec<Vec<usize>>,
    pub description: String,
}

fn load_effect(spec: &EffectSpec, rate: u32) -> Result<Plugin, String> {
    // Save stable absolute paths, not paths relative to a future output directory/cwd.
    let path = spec.plugin.canonicalize().map_err(|e| e.to_string())?;
    let mut host = Vst3Host::builder().build().map_err(|e| e.to_string())?;
    let mut plugin = match spec.class_id.as_deref() {
        Some(id) => host.load_plugin_class(&path, id),
        None => host.load_plugin(&path),
    }.map_err(|e| e.to_string())?;
    if spec.class_id.as_ref().is_some_and(|id| !id.eq_ignore_ascii_case(&plugin.info().uid)) {
        return Err("loaded VST3 class differs from the requested class".into());
    }
    plugin.reconfigure(f64::from(rate), CAPTURE_BLOCK).map_err(|e| e.to_string())?;
    plugin.set_process_mode(ProcessMode::Realtime).map_err(|e| e.to_string())?;
    if let Some(state) = &spec.state { plugin.load_state(state).map_err(|e| e.to_string())?; }
    Ok(plugin)
}

pub fn connect(capture: Capture, setup: &ResolvedPlugins, rate: u32, queue_ms: u32, health: Arc<Health>, controls: &LiveControls, feeds: SourceFeeds)
    -> Result<(CaptureSink, PluginWorker), String> {
    setup.validate()?;
    // Selection was resolved once, from CLI/chain or an embedded scene. Native loading happens here.
    let plugins = setup.effects.iter().enumerate().map(|(index, spec)| {
        let plugin = load_effect(spec, rate).map_err(|e| format!("VST3 slot {}: {e}", index + 1))?;
        Ok::<_, String>((plugin, controls.effect_flag(index)?))
    }).collect::<Result<Vec<_>, _>>()?;
    let (input, mut worker) = spawn(capture, plugins, rate, queue_ms, health, feeds)?;
    for &index in &setup.editors {
        worker.set_editor_open(index as usize - 1, true)?;
    }
    Ok((CaptureSink(input), worker))
}

fn open_editor(plugin: Arc<Mutex<Plugin>>) -> Result<PluginWindow, String> {
    if !plugin.lock().map_err(|_| "VST3 processing state was poisoned")?.has_editor() {
        return Err("this VST3 has no native editor".into());
    }
    let mut window = PluginWindow::new(plugin);
    window.open().map_err(|e| e.to_string())?;
    Ok(window)
}

/// Stereo remains stereo between every stage. Scratch storage is allocated at preparation only.
fn process_chain<'a>(processors: impl IntoIterator<Item = &'a mut Processor>, left: &mut [f32], right: &mut [f32]) -> bool {
    let frames = left.len();
    for processor in processors {
        let bypassed = processor.bypassed.load(Ordering::Relaxed);
        processor.dry.prepare(left, right);
        let buffers = &mut processor.buffers;
        buffers.block_size = frames;
        buffers.clear();
        buffers.inputs[0].channels[0][..frames].copy_from_slice(left);
        buffers.inputs[0].channels[1][..frames].copy_from_slice(right);
        let Ok(mut plugin) = processor.plugin.lock() else { return false; };
        if plugin.process_bus_audio(buffers).is_err() { return false; }
        left.copy_from_slice(&buffers.outputs[0].channels[0][..frames]);
        right.copy_from_slice(&buffers.outputs[0].channels[1][..frames]);
        // An invalid intermediate stage must not be concealed by a later effect.
        if !left.iter().chain(right.iter()).all(|s| s.is_finite()) { return false; }
        processor.dry.mix(left, right, bypassed);
        if !left.iter().chain(right.iter()).all(|s| s.is_finite()) { return false; }
    }
    true
}

fn prepare_plugin(mut plugin: Plugin, bypassed: Arc<AtomicBool>, rate: u32, index: usize)
    -> Result<(Slot, Processor), String> {
    let layout = plugin.audio_bus_layout().map_err(|e| e.to_string())?;
    for buses in [&layout.inputs, &layout.outputs] {
        if buses.len() != 1 || !buses[0].active || buses[0].channel_count != 2 {
            return Err(format!("VST3 slot {} requires exactly one active stereo input and output bus; sidechains/multibus are not omitted", index + 1));
        }
    }
    let buffers = BusAudioBuffers::new(&layout, CAPTURE_BLOCK, f64::from(rate));
    let _ = plugin.take_restart_flags();
    let _ = plugin.take_host_notifications();
    let _ = plugin.take_parameter_edits();
    plugin.start_processing().map_err(|e| format!("VST3 slot {}: {e}", index + 1))?;
    // Read the report only after the final rate, state and processing activation.
    let latency = plugin.latency_samples();
    if u64::from(latency) > u64::from(rate) * 2 {
        let _ = plugin.stop_processing();
        return Err("VST3 latency exceeds the supported 2-second live bypass/compensation bound".into());
    }
    let slot = Slot::new(plugin, bypassed.clone(), latency);
    let processor = Processor::new(slot.plugin.clone(), buffers, bypassed, rate, latency);
    Ok((slot, processor))
}

fn spawn(mut capture: Capture, plugins: Vec<(Plugin, Arc<AtomicBool>)>, rate: u32, queue_ms: u32, health: Arc<Health>, feeds: SourceFeeds)
    -> Result<(QueuedInput, PluginWorker), String> {
    if !(8_000..=192_000).contains(&rate) || !(10..=100).contains(&queue_ms) {
        return Err("invalid VST3 rate/queue capacity".into());
    }
    let capacity = (rate as usize * queue_ms as usize / 1000).max(CAPTURE_BLOCK * 2);
    if capture.app_count() != feeds.apps.len() || capture.has_aux() != feeds.aux.has_aux()
        || feeds.aux_source.is_some() != feeds.aux.has_aux() {
        return Err("input feeds do not match the capture sources".into());
    }
    if let Some(error) = feeds.aux.error() { return Err(error.into()); }
    let (writer, reader) = RingBuffer::<CaptureFrame>::new(capacity);
    let (requests, changes) = RingBuffer::new(1);
    let (retired, acknowledgments) = RingBuffer::new(1);
    let layout = capture.delay_layout();
    let pitch_config = capture.pitch_config();
    let pitch_latency = capture.pitch_latency();
    let vocal_settings = capture.vocal_config().settings();
    let app_controls = capture.app_controls();
    let output_groups = capture.output_groups();
    validate_output_bindings(&feeds.output_bindings,
        RouteMix::ALL.map(|mix| output_groups.iter().any(|group| group.contains(&mix.index()))))?;
    let mut owner = PluginWorker { thread: None, stop: Arc::new(AtomicBool::new(false)), slots: Vec::new(),
        stopped: false, health: health.clone(), chain: Some(ChainControl { requests, retired: acknowledgments,
            layout, rate, capacity, latency: 0, pitch_latency, compensation: 0 }), pitch_config, vocal_settings,
        apps: app_controls, aux_source: feeds.aux_source, output_bindings: feeds.output_bindings,
        output_groups, description: String::new() };
    let mut processors = Vec::new();
    let mut total_latency = 0_u64;
    for (index, (plugin, bypassed)) in plugins.into_iter().enumerate() {
        let (slot, processor) = prepare_plugin(plugin, bypassed, rate, index)?;
        total_latency += u64::from(slot.latency);
        owner.slots.push(slot);
        processors.push(Some(processor));
    }
    let chain = owner.chain.as_mut().expect("prepared chain control");
    let mut delays = chain.layout.prepare(total_latency + pitch_latency as u64, rate)?;
    chain.latency = total_latency;
    chain.compensation = delays.compensation();
    capture.swap_voice_delay(&mut delays);
    drop(delays);
    owner.refresh_description();
    let stop = owner.stop.clone();
    let worker_health = health.clone();
    capture.set_outputs_enabled(true);
    capture.propagate_output_health();
    if let Some(error) = health.error() {
        capture.set_outputs_enabled(false);
        return Err(error.into());
    }
    feeds.aux.enable(true);
    let mut state = ProcessingState { capture, processors, apps: feeds.apps.into_iter().map(Some).collect(),
        aux: feeds.aux, input: reader, requests: changes, retired, unreturned: None };
    owner.thread = Some(thread::Builder::new().name("mixlive-vst3-audio".into()).spawn(move || {
        let mut mic = [0.0; CAPTURE_BLOCK];
        let mut aux = [[0.0; 2]; CAPTURE_BLOCK];
        let mut app_buffers = [[[0.0; 2]; CAPTURE_BLOCK]; MAX_APPS];
        while !stop.load(Ordering::Relaxed) && !worker_health.failed() {
            state.aux.propagate_health(&worker_health);
            state.capture.propagate_output_health();
            if worker_health.failed() { break; }
            // An idle queue is still a valid boundary. Native loading and allocation
            // are already complete, and the PDC bank changes with the processor order.
            if let Ok(mut change) = state.requests.pop() {
                change.apply(&mut state.capture, &mut state.processors, &mut state.apps, &mut state.aux);
                if let Err(rtrb::PushError::Full(change)) = state.retired.push(change) {
                    // One edit is in flight, so this is a broken handoff invariant.
                    // Retain ownership for join instead of freeing buffers here.
                    state.unreturned = Some(change);
                    worker_health.fail(Fault::Routing);
                    break;
                }
            }
            let count = state.input.slots().min(CAPTURE_BLOCK);
            if count == 0 { thread::sleep(Duration::from_millis(1)); continue; }
            for i in 0..count {
                let Ok(frame) = state.input.pop() else { worker_health.fail(Fault::Routing); break; };
                mic[i] = frame.mic; aux[i] = state.aux.next_frame(&frame);
                for (buffer, reader) in app_buffers.iter_mut().zip(&mut state.apps) {
                    buffer[i] = reader.as_mut().expect("prepared app reader").next_source_frame();
                }
            }
            state.aux.propagate_health(&worker_health);
            if worker_health.failed() { break; }
            let apps: [&[[f32; 2]]; MAX_APPS] = std::array::from_fn(|slot| &app_buffers[slot][..count]);
            state.capture.push_with_effect(&mic[..count], state.aux.has_aux().then_some(&aux[..count]), &apps[..state.apps.len()], &mut |left, right| {
                process_chain(state.processors.iter_mut().map(|processor| processor.as_mut().expect("prepared processor slot")), left, right)
            });
        }
        state.aux.enable(false);
        state.capture.set_outputs_enabled(false);
        state
    }).map_err(|e| e.to_string())?);
    Ok((QueuedInput { writer, health }, owner))
}

/// Worker is already joined. Always attempt stop, including when the final flush fails.
fn stop_and_snapshot(plugin: &mut Plugin, save: bool, bypassed: bool) -> Result<Option<EffectSpec>, String> {
    let flushed = if save { flush_editor_changes(plugin).map(|_| ()).map_err(|e| e.to_string()) } else { Ok(()) };
    let stopped = plugin.stop_processing().map_err(|e| e.to_string());
    flushed?;
    stopped?;
    if !save { return Ok(None); }
    if plugin.take_restart_flags() != Default::default() {
        return Err("VST3 requested a restart during final state flush".into());
    }
    Ok(Some(EffectSpec { plugin: plugin.info().path.clone(), class_id: Some(plugin.info().uid.clone()),
        state: Some(plugin.save_state().map_err(|e| e.to_string())?), bypassed }))
}

fn retire_slot(mut slot: Slot) -> Result<(), String> {
    drop(slot.editor.take());
    let mut plugin = slot.plugin.lock().map_err(|_| "VST3 processing state was poisoned")?;
    stop_and_snapshot(&mut plugin, false, slot.bypassed.load(Ordering::Relaxed)).map(|_| ())
}

fn reject_added(added: Option<Slot>, error: String) -> String {
    if let Some(slot) = added {
        if let Err(cleanup) = retire_slot(slot) { return format!("{error}; new VST3 teardown failed: {cleanup}"); }
    }
    error
}

impl PluginWorker {
    fn refresh_description(&mut self) {
        let Some(chain) = &self.chain else { return; };
        self.description = format!("Vocal: {}; reverb pre-delay {:.1} ms; echo time {:.1} ms.\nPitch: {} {}; correction {:.0}%; retune {:.1} ms; nominal pitch delay {} samples ({:.2} ms).\n",
            self.vocal_settings.preset, self.vocal_settings.reverb_predelay_ms, self.vocal_settings.echo_time_ms,
            PitchScale::NAMES[usize::from(self.pitch_config.key)], self.pitch_config.scale.name(),
            self.pitch_config.strength * 100.0, self.pitch_config.retune_ms, chain.pitch_latency,
            chain.pitch_latency as f64 * 1000.0 / f64::from(chain.rate));
        self.description.extend(self.slots.iter().enumerate().map(|(index, slot)| {
            format!("VST3 slot {}: {} [{}]; reported latency {} samples.\n", index + 1, slot.name,
                slot.spec.class_id.as_deref().expect("loaded class ID"), slot.latency)
        }));
        self.description.push_str(&format!("Serial chain: {} effects, total reported latency {} samples ({:.2} ms).\nOutgoing AUX/app send compensation: {} samples ({:.2} ms); monitor adds no compensation. Includes {} samples of built-in pitch latency plus reported VST3 latency, not device or clock alignment.\nOne processing thread/queue, input queue CAPACITY {:.2} ms (not a target delay). Scheduling delay is unmeasured. Chain edits and outgoing compensation change at one block boundary; new plugin/delay histories start empty and removed tails end. Explicit slot bypass blends with latency-aligned dry audio; effects still process. No automatic dry fallback or crash isolation. Restart requests stop the route.",
            self.slots.len(), chain.latency, chain.latency as f64 * 1000.0 / f64::from(chain.rate),
            chain.compensation, chain.compensation as f64 * 1000.0 / f64::from(chain.rate), chain.pitch_latency,
            chain.capacity as f64 * 1000.0 / f64::from(chain.rate)));
    }

    pub fn effects(&self) -> Vec<EffectSpec> {
        self.slots.iter().map(|slot| EffectSpec {
            bypassed: slot.bypassed.load(Ordering::Relaxed), ..slot.spec.clone()
        }).collect()
    }

    pub fn effect_flags(&self) -> Vec<Arc<AtomicBool>> {
        self.slots.iter().map(|slot| slot.bypassed.clone()).collect()
    }

    pub fn app_controls(&self) -> Vec<Arc<AppControls>> { self.apps.clone() }

    pub fn app_count(&self) -> usize { self.apps.len() }

    pub fn aux_source(&self) -> Option<&AuxSource> { self.aux_source.as_ref() }

    pub fn output_bindings(&self) -> &[OutputBinding] { &self.output_bindings }

    /// Prepare only endpoint producers. Source, mix, voice and native processing
    /// retain their current storage and histories through this same worker boundary.
    pub fn set_output_bindings(&mut self, next: Vec<OutputBinding>, outputs: Vec<Option<CaptureOutput>>,
        retained: Vec<(usize, usize)>, controls: &LiveControls) -> Result<(), String> {
        if self.health.failed() || self.stopped { return Err("audio path has stopped; cannot edit outputs".into()); }
        validate_output_bindings(&next, RouteMix::ALL.map(|mix| controls.has_mix(mix)))?;
        if self.thread.is_none() || self.chain.is_none() { return Err("no running audio processor".into()); }
        let change = PreparedOutputChange::new(controls, &self.output_groups, outputs, retained)?;
        let groups = change.output_groups();
        let ProcessingChange::Outputs(retired) = self.exchange_change(ProcessingChange::Outputs(change))? else {
            unreachable!("serialized output change acknowledgment");
        };
        if !retired.applied() {
            return Err(format!("new output failed before attachment: {}",
                retired.error().unwrap_or("output change was not applied")));
        }
        drop(retired);
        // A runtime fault after application still leaves these as the actual bindings.
        self.output_bindings = next;
        self.output_groups = groups;
        if let Some(error) = self.health.error() { return Err(error.into()); }
        Ok(())
    }

    /// Publish binding metadata only when the worker accepted both the new feed
    /// and its prepared routing plan. A later runtime error does not undo a commit.
    pub fn set_aux_source(&mut self, next: Option<AuxSource>, feed: AuxFeed, controls: &LiveControls) -> Result<(), String> {
        if self.health.failed() || self.stopped { return Err("audio path has stopped; cannot edit AUX".into()); }
        if next == self.aux_source { return Ok(()); }
        if next.is_some() != feed.has_aux() || matches!(feed, AuxFeed::Shared(0)) {
            return Err("AUX binding and prepared feed do not match".into());
        }
        let chain = self.chain.as_ref().ok_or("no running audio processor")?;
        if self.thread.is_none() { return Err("no running audio processor".into()); }
        let latency = usize::try_from(chain.latency + chain.pitch_latency as u64)
            .map_err(|_| "combined voice latency is too large")?;
        let capture = PreparedSourceChange::aux(controls, &self.apps, next.is_some(), chain.rate, latency)?;
        let layout = capture.delay_layout();
        let compensation = capture.compensation();
        let change = AuxChange { capture: Box::new(capture), feed, applied: false };
        let ProcessingChange::Aux(retired) = self.exchange_change(ProcessingChange::Aux(change))? else {
            unreachable!("serialized AUX change acknowledgment");
        };
        if !retired.applied {
            return Err(format!("new AUX capture failed before attachment: {}",
                retired.feed.error().unwrap_or("AUX change was not applied")));
        }
        drop(retired);
        self.aux_source = next;
        let chain = self.chain.as_mut().expect("prepared chain control");
        chain.layout = layout;
        chain.compensation = compensation;
        self.refresh_description();
        if let Some(error) = self.health.error() { return Err(error.into()); }
        Ok(())
    }

    pub fn add_app(&mut self, reader: OutputReader, app: Arc<AppControls>, controls: &LiveControls) -> Result<(), String> {
        if self.apps.len() == MAX_APPS { return Err("at most eight application sources are supported".into()); }
        let mut next = self.apps.clone();
        next.push(app);
        let mut readers: Vec<_> = std::iter::repeat_with(|| None).take(self.apps.len()).collect();
        readers.push(Some(reader));
        let retained = (0..self.apps.len()).map(|index| (index, index)).collect();
        self.edit_apps(controls, next, readers, retained, None)
    }

    pub fn remove_app(&mut self, index: usize, stop: Arc<AtomicBool>, controls: &LiveControls) -> Result<(), String> {
        if index >= self.apps.len() { return Err("application source is outside the running list".into()); }
        let mut next = self.apps.clone();
        next.remove(index);
        let readers = std::iter::repeat_with(|| None).take(next.len()).collect();
        let retained = (0..self.apps.len()).filter(|&from| from != index).enumerate()
            .map(|(to, from)| (from, to)).collect();
        self.edit_apps(controls, next, readers, retained, Some(stop))
    }

    fn edit_apps(&mut self, controls: &LiveControls, next: Vec<Arc<AppControls>>,
        readers: Vec<Option<OutputReader>>, retained: Vec<(usize, usize)>, stop_removed: Option<Arc<AtomicBool>>) -> Result<(), String> {
        if self.health.failed() || self.stopped { return Err("audio path has stopped; cannot edit application sources".into()); }
        let chain = self.chain.as_ref().ok_or("no running audio processor")?;
        if self.thread.is_none() { return Err("no running audio processor".into()); }
        let latency = usize::try_from(chain.latency + chain.pitch_latency as u64)
            .map_err(|_| "combined voice latency is too large")?;
        let capture = PreparedSourceChange::apps(controls, &self.apps, next.clone(), chain.rate, latency)?;
        let layout = capture.delay_layout();
        let compensation = capture.compensation();
        let change = AppChange { capture: Box::new(capture), readers, retained, stop_removed };
        let retired = self.exchange_change(ProcessingChange::Apps(change))?;
        // Every removed reader, delay line, routing buffer and old staging vector
        // leaves the worker in this acknowledgment and is released here.
        drop(retired);
        self.apps = next;
        let chain = self.chain.as_mut().expect("prepared chain control");
        chain.layout = layout;
        chain.compensation = compensation;
        self.refresh_description();
        if let Some(error) = self.health.error() { return Err(error.into()); }
        Ok(())
    }

    pub fn pitch_config(&self) -> PitchConfig { self.pitch_config }

    pub fn vocal_settings(&self) -> VocalSettings { self.vocal_settings }

    /// Prepare only the requested vocal modules. The target snapshot initializes
    /// newly enabled processing; it never replaces the independent atomic controls.
    pub fn set_vocal_settings(&mut self, next: VocalSettings, mut targets: VocalConfig) -> Result<(), String> {
        if self.health.failed() || self.stopped { return Err("audio path has stopped; cannot edit vocal settings".into()); }
        let chain = self.chain.as_ref().ok_or("no running audio processor")?;
        if self.thread.is_none() { return Err("no running audio processor".into()); }
        if next == self.vocal_settings { return Ok(()); }
        next.apply_to(&mut targets);
        let change = PreparedVocalChange::new(chain.rate, self.vocal_settings, targets).map_err(|error| error.to_string())?;
        let retired = self.exchange_change(ProcessingChange::Vocal(Box::new(change)))?;
        drop(retired);
        self.vocal_settings = next;
        self.refresh_description();
        if let Some(error) = self.health.error() { return Err(error.into()); }
        Ok(())
    }

    /// All pitch settings form one committed configuration. Off/on changes prepare
    /// and retire FFT storage with the matching PDC bank; enabled edits keep history.
    pub fn set_pitch_config(&mut self, config: PitchConfig) -> Result<(), String> {
        if self.health.failed() || self.stopped { return Err("audio path has stopped; cannot edit pitch settings".into()); }
        let chain = self.chain.as_ref().ok_or("no running audio processor")?;
        if self.thread.is_none() { return Err("no running audio processor".into()); }
        if config == self.pitch_config { return Ok(()); }
        let pitch = PreparedPitchChange::new(chain.rate, self.pitch_config, config)?;
        let pitch_latency = pitch.latency(chain.pitch_latency);
        let delays = if pitch_latency != chain.pitch_latency {
            Some(chain.layout.prepare(chain.latency + pitch_latency as u64, chain.rate)?)
        } else { None };
        let compensation = delays.as_ref().map_or(chain.compensation, PreparedVoiceDelay::compensation);
        let retired = self.exchange_change(ProcessingChange::Pitch { pitch: Box::new(pitch), delays })?;
        // The old corrector and delay bank, if any, are owned by this acknowledgment.
        drop(retired);
        self.pitch_config = config;
        let chain = self.chain.as_mut().expect("prepared chain control");
        chain.pitch_latency = pitch_latency;
        chain.compensation = compensation;
        self.refresh_description();
        if let Some(error) = self.health.error() { return Err(error.into()); }
        Ok(())
    }

    /// Loading thread only. A rejected preparation leaves the active chain intact.
    /// The acknowledged order remains authoritative if retiring a removed plugin fails.
    pub fn edit_chain(&mut self, edit: ChainEdit) -> Result<(), String> {
        if self.health.failed() || self.stopped { return Err("audio path has stopped; cannot edit the VST3 chain".into()); }
        let chain = self.chain.as_ref().ok_or("no running audio processor")?;
        if self.thread.is_none() { return Err("no running audio processor".into()); }
        let mut order: Vec<_> = (0..self.slots.len()).collect();
        let mut added = None;
        let mut added_processor = None;
        match &edit {
            ChainEdit::Add(spec) => {
                super::validate_effects(std::slice::from_ref(spec))?;
                let plugin = load_effect(spec, chain.rate)?;
                let (slot, processor) = prepare_plugin(plugin, Arc::new(AtomicBool::new(spec.bypassed)), chain.rate, self.slots.len())?;
                added = Some(slot);
                added_processor = Some(processor);
            }
            ChainEdit::Remove { index } => {
                if *index >= order.len() { return Err("VST3 slot is outside the active chain".into()); }
                order.remove(*index);
            }
            ChainEdit::Move { from, to } => {
                if *from >= order.len() || *to >= order.len() { return Err("VST3 slot is outside the active chain".into()); }
                if from == to { return Ok(()); }
                let moved = order.remove(*from);
                order.insert(*to, moved);
            }
        }
        let latency = order.iter().map(|&index| u64::from(self.slots[index].latency)).sum::<u64>()
            + added.as_ref().map_or(0, |slot| u64::from(slot.latency));
        // A reorder or a zero-latency add/remove keeps the existing PDC history.
        let delays = if latency == chain.latency { None } else {
            match chain.layout.prepare(latency + chain.pitch_latency as u64, chain.rate) {
                Ok(delays) => Some(delays),
                Err(error) => {
                    drop(added_processor);
                    return Err(reject_added(added, error));
                }
            }
        };
        let compensation = delays.as_ref().map_or(chain.compensation, PreparedVoiceDelay::compensation);
        let mut processors: Vec<_> = std::iter::repeat_with(|| None).take(order.len()).collect();
        if let Some(processor) = added_processor { processors.push(Some(processor)); }
        let retained = order.into_iter().enumerate().map(|(to, from)| (from, to)).collect();
        let change = ChainChange { processors, retained, delays };
        let retired = match self.exchange_change(ProcessingChange::Chain(change)) {
            Ok(retired) => retired,
            Err(error) => return Err(reject_added(added, error)),
        };
        // Removed processors, old PDC banks and all staging vectors are freed here.
        drop(retired);
        let removed = match edit {
            ChainEdit::Add(_) => { self.slots.push(added.expect("prepared added slot")); None }
            ChainEdit::Remove { index } => Some(self.slots.remove(index)),
            ChainEdit::Move { from, to } => { let slot = self.slots.remove(from); self.slots.insert(to, slot); None }
        };
        let chain = self.chain.as_mut().expect("prepared chain control");
        chain.latency = latency;
        chain.compensation = compensation;
        self.refresh_description();
        if let Some(slot) = removed {
            if let Err(error) = retire_slot(slot) {
                self.health.fail(Fault::Routing);
                return Err(format!("VST3 chain changed, but removed plugin teardown failed: {error}"));
            }
        }
        if let Some(error) = self.health.error() { return Err(error.into()); }
        Ok(())
    }

    fn exchange_change(&mut self, change: ProcessingChange) -> Result<ProcessingChange, String> {
        if self.health.failed() { return Err("audio path failed before the processing change".into()); }
        if self.chain.as_mut().expect("prepared chain control").requests.push(change).is_err() {
            return Err("an audio processing change is already pending".into());
        }
        loop {
            if let Ok(retired) = self.chain.as_mut().expect("prepared chain control").retired.pop() { return Ok(retired); }
            if self.health.failed() || self.thread.as_ref().is_none_or(JoinHandle::is_finished) {
                if !self.health.failed() { self.health.fail(Fault::Routing); }
                if let Some(retired) = self.join_processing() { return Ok(retired); }
                return Err(self.health.error().unwrap_or("audio processor stopped before the processing change").into());
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn join_processing(&mut self) -> Option<ProcessingChange> {
        self.stop.store(true, Ordering::Relaxed);
        let joined = self.thread.take().map(JoinHandle::join);
        let acknowledged = self.chain.as_mut().and_then(|chain| chain.retired.pop().ok());
        match joined {
            Some(Ok(mut state)) => {
                let unreturned = state.unreturned.take();
                while let Ok(change) = state.requests.pop() { drop(change); }
                // Capture, resident processors, queue storage and any unapplied edit
                // leave the processing thread through this join result.
                drop(state);
                acknowledged.or(unreturned)
            }
            Some(Err(_)) => { self.health.fail(Fault::Routing); acknowledged }
            None => acknowledged,
        }
    }

    pub fn effect_parameters(&mut self, index: usize) -> Result<Vec<LiveParameter>, String> {
        if self.health.failed() { return Err("audio path has failed; stop before editing parameters".into()); }
        self.slots.get_mut(index).ok_or("VST3 slot is outside the active chain")?.parameter_snapshot(None)
    }

    pub fn set_effect_parameter(&mut self, index: usize, id: u32, normalized: f64) -> Result<Vec<LiveParameter>, String> {
        if self.health.failed() { return Err("audio path has failed; stop before editing parameters".into()); }
        self.slots.get_mut(index).ok_or("VST3 slot is outside the active chain")?.parameter_snapshot(Some((id, normalized)))
    }

    pub fn editor_is_open(&self, index: usize) -> bool {
        self.slots.get(index).and_then(|slot| slot.editor.as_ref()).is_some_and(PluginWindow::is_open)
    }

    /// Explicit request on the loading thread. Closing/reopening never reloads the effect.
    pub fn set_editor_open(&mut self, index: usize, open: bool) -> Result<(), String> {
        let slot = self.slots.get_mut(index).ok_or("VST3 slot is outside the active chain")?;
        if !open {
            drop(slot.editor.take());
            return Ok(());
        }
        if self.health.failed() { return Err("audio path has failed; stop before opening an editor".into()); }
        if slot.editor.as_ref().is_some_and(PluginWindow::is_open) { return Ok(()); }
        // A dismissed window still owns an attached view until it is dropped.
        drop(slot.editor.take());
        slot.editor = Some(open_editor(slot.plugin.clone()).map_err(|e| format!("VST3 slot {}: {e}", index + 1))?);
        Ok(())
    }

    /// The application owns message dispatch (GPUI's loop or the CLI's explicit pump).
    /// Service native requests outside UI borrows. Return whether any window was dismissed.
    pub fn poll(&mut self) -> Result<bool, String> {
        let mut dismissed = false;
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if let Some(editor) = &mut slot.editor {
                if let Err(error) = editor.service_platform_events() {
                    self.health.fail(Fault::Routing);
                    return Err(format!("VST3 slot {}: {error}", index + 1));
                }
                if editor.closed_by_user() {
                    drop(slot.editor.take());
                    dismissed = true;
                }
            }
        }
        if self.thread.as_ref().is_some_and(|worker| worker.is_finished()) && !self.stop.load(Ordering::Relaxed) && !self.health.failed() {
            self.health.fail(Fault::Routing);
            return Err("VST3 processing thread stopped unexpectedly".into());
        }
        for (index, slot) in self.slots.iter_mut().enumerate() {
            let mut plugin = match slot.plugin.try_lock() {
                Ok(plugin) => plugin,
                Err(TryLockError::WouldBlock) => continue,
                Err(TryLockError::Poisoned(_)) => {
                    self.health.fail(Fault::Routing); return Err(format!("VST3 slot {} was poisoned", index + 1));
                }
            };
            if plugin.take_restart_flags() != Default::default() {
                self.health.fail(Fault::Routing);
                return Err(format!("VST3 slot {} requested a restart; configure and start the route again", index + 1));
            }
            // Edits already reach process_bus_audio. Drain UI feedback, do not replay it.
            update_parameter_feedback(&mut slot.parameters, plugin.get_parameter_changes());
            let _ = plugin.take_host_notifications();
            let _ = plugin.take_parameter_edits();
        }
        Ok(dismissed)
    }

    fn shutdown(&mut self, save: bool) -> Result<Option<ChainFile>, String> {
        if self.stopped { return Ok(None); }
        drop(self.join_processing());
        for slot in &mut self.slots { drop(slot.editor.take()); }
        self.stopped = true;
        let mut effects = Vec::new();
        let mut first_error = None;
        // Finish EVERY slot even if one state cannot be saved; never publish a partial chain.
        for (index, slot) in self.slots.iter().enumerate() {
            let mut plugin = slot.plugin.lock().unwrap_or_else(|poisoned| {
                self.health.fail(Fault::Routing); poisoned.into_inner()
            });
            if plugin.take_restart_flags() != Default::default() { self.health.fail(Fault::Routing); }
            match stop_and_snapshot(&mut plugin, save && !self.health.failed(), slot.bypassed.load(Ordering::Relaxed)) {
                Ok(Some(effect)) => effects.push(effect),
                Ok(None) => {},
                Err(error) => {
                    self.health.fail(Fault::Routing);
                    first_error.get_or_insert_with(|| format!("VST3 slot {}: {error}", index + 1));
                }
            }
        }
        if let Some(error) = first_error { return Err(error); }
        if let Some(error) = self.health.error() { return Err(error.into()); }
        Ok(save.then_some(ChainFile { schema: 2, effects }))
    }

    /// Devices have stopped. Return all slot states only after native teardown.
    pub fn snapshot(mut self) -> Result<ChainFile, String> {
        let snapshot = self.shutdown(true)?.ok_or("no live VST3 snapshot")?;
        drop(self);
        Ok(snapshot)
    }

    /// Devices have stopped. Native teardown precedes a single all-or-nothing publication.
    pub fn finish(mut self, chain_out: Option<&Path>) -> Result<(), String> {
        let snapshot = self.shutdown(chain_out.is_some())?;
        drop(self);
        if let (Some(path), Some(snapshot)) = (chain_out, snapshot) { snapshot.save_new(path)?; }
        Ok(())
    }
}
impl Drop for PluginWorker {
    fn drop(&mut self) { let _ = self.shutdown(false); }
}

#[cfg(test)]
#[path = "plugin_native_tests.rs"]
mod tests;
