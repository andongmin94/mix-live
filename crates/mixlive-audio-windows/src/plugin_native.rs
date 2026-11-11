//! One worker for the ordered vocal chain; no native plugin calls in device callbacks.
use super::{ChainFile, EffectSpec, PluginArgs};
use crate::{bridge::{Fault, Health}, capture::{Capture, CAPTURE_BLOCK}};
use mixlive_vst3::editor::{flush_editor_changes, pump_editor_messages};
use rtrb::{Producer, RingBuffer};
use std::{path::Path, sync::{Arc, Mutex, TryLockError, atomic::{AtomicBool, Ordering}}, thread::{self, JoinHandle}, time::Duration};
use vst3_host::{Plugin, PluginWindow, Vst3Host, audio::BusAudioBuffers, plugin::ProcessMode};

// Mic and clock-converted AUX travel together. The chain only processes the mic.
pub(crate) struct QueuedInput {
    writer: Producer<[f32; 3]>,
    has_aux: bool,
    health: Arc<Health>,
}
impl QueuedInput {
    fn push(&mut self, mic: &[f32], aux: Option<&[[f32; 2]]>) {
        if self.health.failed() { return; }
        if aux.is_some() != self.has_aux || aux.is_some_and(|a| a.len() != mic.len()) {
            self.health.fail(Fault::Input); return;
        }
        for (i, &voice) in mic.iter().enumerate() {
            let music = aux.map_or([0.0; 2], |a| a[i]);
            let frame = [voice, music[0], music[1]];
            if frame.iter().any(|s| !s.is_finite()) {
                self.health.fail(Fault::Input); return;
            }
            if self.writer.push(frame).is_err() {
                self.health.fail(Fault::Overflow); return;
            }
        }
    }
}

pub(crate) enum CaptureSink { Direct(Box<Capture>), Queued(QueuedInput) }
impl CaptureSink {
    pub fn push(&mut self, mic: &[f32], aux: Option<&[[f32; 2]]>) {
        match self { Self::Direct(capture) => capture.push(mic, aux), Self::Queued(input) => input.push(mic, aux) }
    }
}

struct Slot { plugin: Arc<Mutex<Plugin>>, editor: Option<PluginWindow> }
struct Processor { plugin: Arc<Mutex<Plugin>>, buffers: BusAudioBuffers }

/// Loaded/polled/dropped on the main thread. Join ONE processing thread before native teardown.
/// Native crashes/hangs are not isolated; editor/control calls still share the plugin mutexes.
pub(crate) struct PluginWorker {
    thread: Option<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    slots: Vec<Slot>,
    stopped: bool,
    health: Arc<Health>,
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

pub fn connect(capture: Capture, args: &PluginArgs, rate: u32, queue_ms: u32, has_aux: bool, health: Arc<Health>)
    -> Result<(CaptureSink, Option<PluginWorker>), String> {
    let specs = args.resolve()?;
    if specs.is_empty() { return Ok((CaptureSink::Direct(Box::new(capture)), None)); }
    // All files/classes/states are checked before any capture/playback stream starts.
    let plugins = specs.iter().enumerate().map(|(index, spec)| {
        load_effect(spec, rate).map_err(|e| format!("VST3 slot {}: {e}", index + 1))
    }).collect::<Result<Vec<_>, _>>()?;
    let (input, mut worker) = spawn(capture, plugins, rate, queue_ms, has_aux, health)?;
    for &index in &args.vst3_editor {
        let slot = &mut worker.slots[index as usize - 1];
        slot.editor = Some(open_editor(slot.plugin.clone()).map_err(|e| format!("VST3 slot {index}: {e}"))?);
    }
    Ok((CaptureSink::Queued(input), Some(worker)))
}

fn open_editor(plugin: Arc<Mutex<Plugin>>) -> Result<PluginWindow, String> {
    if !plugin.lock().map_err(|_| "VST3 processing state was poisoned")?.has_editor() {
        return Err("this VST3 has no native editor; omit this --vst3-editor slot".into());
    }
    let mut window = PluginWindow::new(plugin);
    window.open().map_err(|e| e.to_string())?;
    Ok(window)
}

/// Stereo remains stereo between every stage. Scratch storage is allocated at preparation only.
fn process_chain(processors: &mut [Processor], left: &mut [f32], right: &mut [f32]) -> bool {
    let frames = left.len();
    for processor in processors {
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
    }
    true
}

fn spawn(mut capture: Capture, plugins: Vec<Plugin>, rate: u32, queue_ms: u32, has_aux: bool, health: Arc<Health>)
    -> Result<(QueuedInput, PluginWorker), String> {
    if plugins.is_empty() || !(8_000..=192_000).contains(&rate) || !(10..=100).contains(&queue_ms) {
        return Err("invalid VST3 chain/rate/queue capacity".into());
    }
    let capacity = (rate as usize * queue_ms as usize / 1000).max(CAPTURE_BLOCK * 2);
    let (writer, mut reader) = RingBuffer::<[f32; 3]>::new(capacity);
    let mut owner = PluginWorker { thread: None, stop: Arc::new(AtomicBool::new(false)), slots: Vec::new(),
        stopped: false, health: health.clone(), description: String::new() };
    let mut processors = Vec::new();
    let mut total_latency = 0_u64;
    for (index, mut plugin) in plugins.into_iter().enumerate() {
        let layout = plugin.audio_bus_layout().map_err(|e| e.to_string())?;
        for buses in [&layout.inputs, &layout.outputs] {
            if buses.len() != 1 || !buses[0].active || buses[0].channel_count != 2 {
                return Err(format!("VST3 slot {} requires exactly one active stereo input and output bus; sidechains/multibus are not omitted", index + 1));
            }
        }
        let latency = plugin.latency_samples();
        total_latency += u64::from(latency);
        owner.description.push_str(&format!("VST3 slot {}: {} [{}]; reported latency {} samples.\n",
            index + 1, plugin.info().name, plugin.info().uid, latency));
        let buffers = BusAudioBuffers::new(&layout, CAPTURE_BLOCK, f64::from(rate));
        let _ = plugin.take_restart_flags();
        let _ = plugin.take_host_notifications();
        let _ = plugin.take_parameter_edits();
        plugin.start_processing().map_err(|e| format!("VST3 slot {}: {e}", index + 1))?;
        let plugin = Arc::new(Mutex::new(plugin));
        processors.push(Processor { plugin: plugin.clone(), buffers });
        owner.slots.push(Slot { plugin, editor: None });
    }
    owner.description.push_str(&format!("Serial chain: {} effects, total reported latency {} samples ({:.2} ms).\nOne processing thread/queue, input queue CAPACITY {:.2} ms (not a target delay). Scheduling delay is unmeasured. No dry fallback, delay compensation or crash isolation. Restart requests stop the route.",
        processors.len(), total_latency, total_latency as f64 * 1000.0 / f64::from(rate), capacity as f64 * 1000.0 / f64::from(rate)));
    let stop = owner.stop.clone();
    let worker_health = health.clone();
    owner.thread = Some(thread::Builder::new().name("mixlive-vst3-audio".into()).spawn(move || {
        let mut mic = [0.0; CAPTURE_BLOCK];
        let mut aux = [[0.0; 2]; CAPTURE_BLOCK];
        while !stop.load(Ordering::Relaxed) && !worker_health.failed() {
            let count = reader.slots().min(CAPTURE_BLOCK);
            if count == 0 { thread::sleep(Duration::from_millis(1)); continue; }
            for i in 0..count {
                let Ok(frame) = reader.pop() else { worker_health.fail(Fault::Routing); return; };
                mic[i] = frame[0]; aux[i] = [frame[1], frame[2]];
            }
            capture.push_with_effect(&mic[..count], has_aux.then_some(&aux[..count]), &mut |voice, left, right| {
                left.copy_from_slice(voice);
                right.copy_from_slice(voice);
                process_chain(&mut processors, left, right)
            });
        }
    }).map_err(|e| e.to_string())?);
    Ok((QueuedInput { writer, has_aux, health }, owner))
}

/// Worker is already joined. Always attempt stop, including when the final flush fails.
fn stop_and_snapshot(plugin: &mut Plugin, save: bool) -> Result<Option<EffectSpec>, String> {
    let flushed = if save { flush_editor_changes(plugin).map(|_| ()).map_err(|e| e.to_string()) } else { Ok(()) };
    let stopped = plugin.stop_processing().map_err(|e| e.to_string());
    flushed?;
    stopped?;
    if !save { return Ok(None); }
    if plugin.take_restart_flags() != Default::default() {
        return Err("VST3 requested a restart during final state flush".into());
    }
    Ok(Some(EffectSpec { plugin: plugin.info().path.clone(), class_id: Some(plugin.info().uid.clone()),
        state: Some(plugin.save_state().map_err(|e| e.to_string())?) }))
}

impl PluginWorker {
    pub fn poll(&mut self) -> Result<(), String> {
        // Pump once per host iteration, never with a plugin mutex held.
        if self.slots.iter().any(|slot| slot.editor.is_some()) && !pump_editor_messages() {
            self.health.fail(Fault::Routing);
            return Err("native editor message loop was interrupted".into());
        }
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if let Some(editor) = &mut slot.editor {
                if let Err(error) = editor.service_platform_events() {
                    self.health.fail(Fault::Routing);
                    return Err(format!("VST3 slot {}: {error}", index + 1));
                }
                if editor.closed_by_user() { drop(slot.editor.take()); }
            }
        }
        if self.thread.as_ref().is_some_and(|worker| worker.is_finished()) && !self.stop.load(Ordering::Relaxed) && !self.health.failed() {
            self.health.fail(Fault::Routing);
            return Err("VST3 processing thread stopped unexpectedly".into());
        }
        for (index, slot) in self.slots.iter().enumerate() {
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
            let _ = plugin.get_parameter_changes();
            let _ = plugin.take_host_notifications();
            let _ = plugin.take_parameter_edits();
        }
        Ok(())
    }

    fn shutdown(&mut self, save: bool) -> Result<Option<ChainFile>, String> {
        if self.stopped { return Ok(None); }
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.thread.take() {
            if worker.join().is_err() { self.health.fail(Fault::Routing); }
        }
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
            match stop_and_snapshot(&mut plugin, save && !self.health.failed()) {
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
        Ok(save.then_some(ChainFile { schema: 1, effects }))
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
