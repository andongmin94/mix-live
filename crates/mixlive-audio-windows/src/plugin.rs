//! One optional in-process vocal VST3. Device callbacks only publish bounded audio frames.
use clap::Args;
use std::path::PathBuf;

#[derive(Args, Clone, Debug, Default)]
pub struct PluginArgs {
    /// Apply one trusted stereo VST3 to the voice, after built-in DSP. No live editor.
    #[arg(long, global = true)]
    pub vst3: Option<PathBuf>,
    /// Load a MixLive VST3 state made by mixlive-plugins. Requires --vst3.
    #[arg(long, global = true, requires = "vst3")]
    pub vst3_state: Option<PathBuf>,
    /// Select the audio component by its 32-digit class ID. Requires --vst3.
    #[arg(long, global = true, requires = "vst3")]
    pub vst3_class: Option<String>,
}

impl PluginArgs {
    pub fn validate(&self) -> Result<(), String> {
        if self.vst3.is_none() && (self.vst3_state.is_some() || self.vst3_class.is_some()) {
            return Err("VST3 state/class requires an explicit plugin".into());
        }
        for path in [&self.vst3, &self.vst3_state].into_iter().flatten() {
            if path.as_os_str().is_empty() { return Err("VST3 paths must not be empty".into()); }
        }
        if self.vst3_class.as_ref().is_some_and(|id| id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit())) {
            return Err("VST3 class ID must be 32 hexadecimal digits".into());
        }
        Ok(())
    }
}

#[cfg(windows)]
pub(crate) mod native {
    use super::PluginArgs;
    use crate::{bridge::{Fault, Health}, capture::{Capture, CAPTURE_BLOCK}};
    use mixlive_vst3::StateFile;
    use rtrb::{Producer, RingBuffer};
    use std::{sync::{Arc, Mutex, TryLockError, atomic::{AtomicBool, Ordering}}, thread::{self, JoinHandle}, time::Duration};
    use vst3_host::{Plugin, Vst3Host, audio::BusAudioBuffers, plugin::ProcessMode};

    // Mic and the already clock-converted AUX travel together. Never queue them independently.
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

    pub(crate) enum CaptureSink {
        Direct(Box<Capture>),
        Queued(QueuedInput),
    }
    impl CaptureSink {
        pub fn push(&mut self, mic: &[f32], aux: Option<&[[f32; 2]]>) {
            match self { Self::Direct(capture) => capture.push(mic, aux), Self::Queued(input) => input.push(mic, aux) }
        }
    }

    /// Created, polled and dropped on the loading thread. Join processing before COM teardown.
    /// A native plugin can crash/hang the process: this is NOT process isolation or a timeout.
    pub(crate) struct PluginWorker {
        thread: Option<JoinHandle<()>>,
        stop: Arc<AtomicBool>,
        plugin: Arc<Mutex<Plugin>>,
        health: Arc<Health>,
        pub description: String,
    }

    fn load(args: &PluginArgs, rate: u32) -> Result<Plugin, String> {
        args.validate()?;
        let path = args.vst3.as_deref().ok_or("missing VST3 path")?;
        let saved = args.vst3_state.as_deref().map(StateFile::read).transpose().map_err(|e| e.to_string())?;
        let class = args.vst3_class.as_deref().or_else(|| saved.as_ref().map(|s| s.class_id.as_str()));
        let mut host = Vst3Host::builder().build().map_err(|e| e.to_string())?;
        let mut plugin = match class {
            Some(id) => host.load_plugin_class(path, id),
            None => host.load_plugin(path),
        }.map_err(|e| e.to_string())?;
        if class.is_some_and(|id| !id.eq_ignore_ascii_case(&plugin.info().uid)) {
            return Err("loaded VST3 class differs from the requested class".into());
        }
        plugin.reconfigure(f64::from(rate), CAPTURE_BLOCK).map_err(|e| e.to_string())?;
        plugin.set_process_mode(ProcessMode::Realtime).map_err(|e| e.to_string())?;
        if let Some(saved) = saved {
            if !saved.class_id.eq_ignore_ascii_case(&plugin.info().uid) {
                return Err("VST3 state belongs to a different audio class".into());
            }
            plugin.load_state(&saved.state).map_err(|e| e.to_string())?;
        }
        Ok(plugin)
    }

    pub fn connect(capture: Capture, args: &PluginArgs, rate: u32, queue_ms: u32, has_aux: bool, health: Arc<Health>)
        -> Result<(CaptureSink, Option<PluginWorker>), String> {
        args.validate()?;
        if args.vst3.is_none() { return Ok((CaptureSink::Direct(Box::new(capture)), None)); }
        // Initialization and state access happen before any capture/playback starts.
        let plugin = load(args, rate)?;
        let (input, worker) = spawn(capture, plugin, rate, queue_ms, has_aux, health)?;
        Ok((CaptureSink::Queued(input), Some(worker)))
    }

    fn spawn(mut capture: Capture, mut plugin: Plugin, rate: u32, queue_ms: u32, has_aux: bool, health: Arc<Health>)
        -> Result<(QueuedInput, PluginWorker), String> {
        if !(8_000..=192_000).contains(&rate) || !(10..=100).contains(&queue_ms) {
            return Err("invalid VST3 worker rate/queue capacity".into());
        }
        let layout = plugin.audio_bus_layout().map_err(|e| e.to_string())?;
        for buses in [&layout.inputs, &layout.outputs] {
            if buses.len() != 1 || !buses[0].active || buses[0].channel_count != 2 {
                return Err("live VST3 requires exactly one active stereo input and output bus; sidechains/multibus are not omitted".into());
            }
        }
        let latency = plugin.latency_samples();
        let capacity = (rate as usize * queue_ms as usize / 1000).max(CAPTURE_BLOCK * 2);
        let description = format!("VST3: {} [{}]; reported latency {} samples ({:.2} ms).\nDedicated processing thread; input queue CAPACITY {:.2} ms (not a target delay). Scheduling delay is unmeasured. No dry fallback, live editor or plugin-delay compensation.",
            plugin.info().name, plugin.info().uid, latency, latency as f64 * 1000.0 / f64::from(rate), capacity as f64 * 1000.0 / f64::from(rate));
        let mut buffers = BusAudioBuffers::new(&layout, CAPTURE_BLOCK, f64::from(rate));
        let (writer, mut reader) = RingBuffer::<[f32; 3]>::new(capacity);
        // State/configuration notifications raised during setup describe the layout just checked.
        let _ = plugin.take_restart_flags();
        let _ = plugin.take_host_notifications();
        let _ = plugin.take_parameter_edits();
        plugin.start_processing().map_err(|e| e.to_string())?;
        let plugin = Arc::new(Mutex::new(plugin));
        let mut owner = PluginWorker { thread: None, stop: Arc::new(AtomicBool::new(false)), plugin: plugin.clone(), health: health.clone(), description };
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
                    buffers.block_size = voice.len();
                    buffers.clear();
                    buffers.inputs[0].channels[0][..voice.len()].copy_from_slice(voice);
                    buffers.inputs[0].channels[1][..voice.len()].copy_from_slice(voice);
                    // This may contend with brief main-thread service, NEVER with a device callback.
                    let Ok(mut plugin) = plugin.lock() else { return false; };
                    if plugin.process_bus_audio(&mut buffers).is_err() { return false; }
                    left.copy_from_slice(&buffers.outputs[0].channels[0][..voice.len()]);
                    right.copy_from_slice(&buffers.outputs[0].channels[1][..voice.len()]);
                    left.iter().chain(right.iter()).all(|s| s.is_finite())
                });
            }
        }).map_err(|e| e.to_string())?);
        Ok((QueuedInput { writer, has_aux, health }, owner))
    }

    impl PluginWorker {
        /// Main/control thread only. Do not reconfigure a running plugin from an audio callback.
        pub fn poll(&self) -> Result<(), String> {
            if self.thread.as_ref().is_some_and(|worker| worker.is_finished()) && !self.stop.load(Ordering::Relaxed) && !self.health.failed() {
                self.health.fail(Fault::Routing);
                return Err("VST3 processing thread stopped unexpectedly".into());
            }
            let mut plugin = match self.plugin.try_lock() {
                Ok(plugin) => plugin,
                Err(TryLockError::WouldBlock) => return Ok(()),
                Err(TryLockError::Poisoned(_)) => {
                    self.health.fail(Fault::Routing); return Err("VST3 processing state was poisoned".into());
                }
            };
            // This first live path has fixed state/buses/latency. Stop rather than silently
            // accepting a restart request or changing routing while samples are queued.
            if plugin.take_restart_flags() != Default::default() {
                self.health.fail(Fault::Routing);
                return Err("VST3 requested a restart; stop, configure and start the route again".into());
            }
            let _ = plugin.take_host_notifications();
            let _ = plugin.take_parameter_edits();
            Ok(())
        }
    }
    impl Drop for PluginWorker {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(worker) = self.thread.take() {
                if worker.join().is_err() { self.health.fail(Fault::Routing); }
            }
            // Even after a Rust panic, native state is released on its loading thread.
            let mut plugin = self.plugin.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if plugin.stop_processing().is_err() { self.health.fail(Fault::Routing); }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::{bridge::channel, config::AuxDestination, control::LiveControls};
        use mixlive_effects::{VocalChain, VocalConfig};
        use std::time::Instant;

        #[test]
        fn capture_queue_rejects_overflow_and_invalid_input_without_waiting() {
            let health = Arc::new(Health::default());
            let (writer, _reader) = RingBuffer::new(2);
            let mut input = QueuedInput { writer, has_aux: true, health: health.clone() };
            input.push(&[0.1; 3], Some(&[[0.2, -0.1]; 3]));
            assert_eq!(health.error(), Some("audio clock queue overflow; stopped instead of building unbounded latency"));
            let health = Arc::new(Health::default());
            let (writer, _reader) = RingBuffer::new(2);
            let mut input = QueuedInput { writer, has_aux: false, health: health.clone() };
            input.push(&[f32::NAN], None);
            assert!(health.failed());
        }

        #[test]
        #[ignore = "requires existing MIXLIVE_GAIN_VST3 and MIXLIVE_STATE_VST3 native fixtures"]
        fn native_worker_applies_vst3_only_to_voice_and_preserves_aux_and_teardown() {
            let args = PluginArgs { vst3: Some(std::env::var_os("MIXLIVE_GAIN_VST3").unwrap().into()), ..PluginArgs::default() };
            let mut plugin = load(&args, 48000).unwrap();
            plugin.set_parameter(0, 0.25).unwrap();
            let health = Arc::new(Health::default());
            let controls = Arc::new(LiveControls::new(0.0, Some(0.0), true).unwrap());
            let (writer, mut reader) = channel(48000, 48000, 10, 2, health.clone()).unwrap();
            let capture = Capture::new(VocalChain::new(48000, VocalConfig::default()).unwrap(), 48000,
                Some((0.0, AuxDestination::Send)), vec![(writer, vec![0, 1])], health.clone(), controls).unwrap();
            let (mut input, worker) = spawn(capture, plugin, 48000, 100, true, health.clone()).unwrap();
            for count in [1, 137, 256, 219, 923] { input.push(&vec![0.4; count], Some(&vec![[0.1, -0.2]; count])); }
            let deadline = Instant::now() + Duration::from_secs(3);
            while health.captured_frames.load(Ordering::Relaxed) < 1536 && !health.failed() && Instant::now() < deadline {
                worker.poll().unwrap(); thread::sleep(Duration::from_millis(1));
            }
            assert!(!health.failed(), "{:?}", health.error());
            assert_eq!(health.captured_frames.load(Ordering::Relaxed), 1536);
            for _ in 0..512 { reader.next_frame([1.0; 2]); }
            let frame = reader.next_frame([1.0; 2]);
            for (actual, expected) in frame.into_iter().zip([0.1, 0.1, 0.2, -0.1]) { assert!((actual - expected).abs() < 0.001); }
            drop(worker); // join, stop_processing, and native teardown on this loading thread
            assert!(!health.failed());
            let bad = PluginArgs { vst3: Some(std::env::var_os("MIXLIVE_STATE_VST3").unwrap().into()), ..PluginArgs::default() };
            let mut plugin = load(&bad, 48000).unwrap();
            plugin.start_processing().unwrap();
            plugin.set_parameter(0, 0.37).unwrap();
            mixlive_vst3::editor::flush_editor_changes(&mut plugin).unwrap();
            plugin.stop_processing().unwrap();
            let directory = tempfile::tempdir().unwrap();
            let state_path = directory.path().join("voice.json");
            StateFile { schema: 1, class_id: plugin.info().uid.clone(), state: plugin.save_state().unwrap() }
                .save_new(&state_path).unwrap();
            drop(plugin);
            let saved = PluginArgs { vst3_state: Some(state_path.clone()), ..bad };
            let plugin = load(&saved, 48000).unwrap();
            let value = plugin.get_parameters().unwrap().into_iter().find(|p| p.id == 0).unwrap().value;
            assert!((value - 0.37).abs() < 1e-6);
            assert!(load(&PluginArgs { vst3_state: Some(state_path), ..args }, 48000).is_err());
            let health = Arc::new(Health::default());
            let controls = Arc::new(LiveControls::new(0.0, None, false).unwrap());
            let (writer, _reader) = channel(48000, 48000, 30, 1, health.clone()).unwrap();
            let capture = Capture::new(VocalChain::new(48000, VocalConfig::default()).unwrap(), 48000,
                None, vec![(writer, vec![0])], health.clone(), controls).unwrap();
            assert!(spawn(capture, plugin, 48000, 30, false, health).is_err());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plugin_arguments_require_explicit_path_and_valid_class() {
        assert!(PluginArgs::default().validate().is_ok());
        let mut args = PluginArgs { vst3_state: Some("voice.json".into()), ..PluginArgs::default() };
        assert!(args.validate().is_err());
        args.vst3 = Some("effect.vst3".into());
        assert!(args.validate().is_ok());
        args.vst3_class = Some("not-a-class".into());
        assert!(args.validate().is_err());
        args.vst3_class = Some("0123456789abcdef0123456789ABCDEF".into());
        assert!(args.validate().is_ok());
    }
}
