use std::sync::{Arc, atomic::{AtomicU32, Ordering}};
use mixlive_core::{ChannelLayout, Engine, Input, Mix, Scene, Send, Source};
#[cfg(any(windows, test))]
use mixlive_core::SendDelays;
use mixlive_effects::{Meters, VocalChain, VocalParameter};
#[cfg(any(windows, test))]
use mixlive_effects::{PreparedVocalChange, VocalConfig};
use crate::{bridge::{Fault, Health, OutputWriter}, config::{MAX_APPS, MAX_MIXES, PitchConfig}, pitch_correction::PitchCorrector, control::{AppControls, GainRamp, LiveControls, RouteMix, RouteSource, MAX_SOURCES, MAX_ROUTES}};

pub const CAPTURE_BLOCK: usize = 256;

/// Immutable routing information copied before Capture moves to the worker.
/// Preparing a replacement only allocates on the control thread.
#[cfg(any(windows, test))]
#[derive(Clone)]
pub(crate) struct VoiceDelayLayout {
    compensated: Vec<bool>,
}

#[cfg(any(windows, test))]
pub(crate) struct PreparedVoiceDelay {
    delays: SendDelays,
    latency: usize,
    compensation: usize,
}

#[cfg(any(windows, test))]
impl PreparedVoiceDelay {
    #[cfg(test)]
    pub(crate) fn latency(&self) -> usize { self.latency }
    pub(crate) fn compensation(&self) -> usize { self.compensation }
}

#[cfg(any(windows, test))]
impl VoiceDelayLayout {
    fn from_sources(source_slots: &[usize], mix_slots: &[usize]) -> Self {
        let compensated = source_slots.iter().flat_map(|source| {
            mix_slots.iter().map(move |mix| *source != 0 && *mix != 0)
        }).collect();
        Self { compensated }
    }

    pub(crate) fn prepare(&self, total_voice_latency: u64, rate: u32) -> Result<PreparedVoiceDelay, String> {
        if total_voice_latency > u64::from(rate) * 2 {
            return Err("voice chain latency exceeds the supported 2-second outgoing compensation".into());
        }
        let latency = total_voice_latency as usize;
        let frames: Vec<_> = self.compensated.iter().map(|&enabled| if enabled { latency } else { 0 }).collect();
        let compensation = if self.compensated.contains(&true) { latency } else { 0 };
        Ok(PreparedVoiceDelay { delays: SendDelays::new(&frames), latency, compensation })
    }
}

/// Prepare only an off/on transition's DSP storage on the control thread.
/// Settings within the same enabled state keep the existing processing history.
#[cfg(any(windows, test))]
pub(crate) struct PreparedPitchChange {
    config: PitchConfig,
    replacement: Option<Option<PitchCorrector>>,
}

#[cfg(any(windows, test))]
impl PreparedPitchChange {
    pub(crate) fn new(rate: u32, current: PitchConfig, next: PitchConfig) -> Result<Self, String> {
        next.validate()?;
        if !(8000..=192_000).contains(&rate) { return Err("unsupported pitch correction sample rate".into()); }
        let replacement = if (current.strength == 0.0) != (next.strength == 0.0) {
            Some(PitchCorrector::new(rate, next)?)
        } else { None };
        Ok(Self { config: next, replacement })
    }

    /// Resulting pitch delay before application; after application the plan owns
    /// any retired corrector and must be returned to the control thread.
    pub(crate) fn latency(&self, current_latency: usize) -> usize {
        self.replacement.as_ref().map_or(current_latency,
            |pitch| pitch.as_ref().map_or(0, PitchCorrector::latency))
    }
}

/// A new source routing plan prepared without touching the running processing state.
/// Retained source identities carry their atomic targets, ramps and delay histories.
/// After application this plan owns the retired engine/storage for control-thread drop.
#[cfg(any(windows, test))]
pub(crate) struct PreparedSourceChange {
    engine: Engine,
    aux_enabled: bool,
    reset_aux: bool,
    aux_changed: bool,
    app_controls: Vec<Arc<AppControls>>,
    source_slots: Vec<usize>,
    source_mutes: [GainRamp; MAX_SOURCES],
    source_trims: [GainRamp; MAX_SOURCES],
    route_count: usize,
    route_ramps: [GainRamp; MAX_ROUTES],
    retained_sources: Vec<(usize, usize)>,
    retained_routes: Vec<(usize, usize)>,
    retained_delays: Vec<(usize, usize)>,
    layout: VoiceDelayLayout,
    voice_latency: usize,
    compensation: usize,
}

#[cfg(any(windows, test))]
impl PreparedSourceChange {
    pub(crate) fn apps(controls: &LiveControls, current: &[Arc<AppControls>],
        next: Vec<Arc<AppControls>>, rate: u32, total_voice_latency: usize) -> Result<Self, String> {
        let has_aux = controls.has_aux();
        Self::prepare(controls, (has_aux, current), (has_aux, next), rate, total_voice_latency, true)
    }

    /// An AUX request always creates a new capture identity, including a change
    /// between channels on the same device. Its old delay history must end here.
    pub(crate) fn aux(controls: &LiveControls, apps: &[Arc<AppControls>],
        next_aux: bool, rate: u32, total_voice_latency: usize) -> Result<Self, String> {
        Self::prepare(controls, (controls.has_aux(), apps), (next_aux, apps.to_vec()), rate, total_voice_latency, false)
    }

    fn prepare(controls: &LiveControls, current: (bool, &[Arc<AppControls>]),
        next: (bool, Vec<Arc<AppControls>>), rate: u32, total_voice_latency: usize, keep_aux_delay: bool) -> Result<Self, String> {
        let (current_aux, current) = current;
        let (next_aux, next) = next;
        if !(8_000..=192_000).contains(&rate) { return Err("unsupported capture sample rate".into()); }
        if current.len() > MAX_APPS || next.len() > MAX_APPS { return Err("too many application sources".into()); }
        if total_voice_latency as u64 > u64::from(rate) * 2 {
            return Err("voice chain latency exceeds the supported 2-second outgoing compensation".into());
        }
        for apps in [current, next.as_slice()] {
            if apps.iter().enumerate().any(|(index, app)| apps[..index].iter().any(|other| Arc::ptr_eq(app, other))) {
                return Err("application routing contains a duplicate source identity".into());
            }
        }
        let mix_slots: Vec<_> = controls.mixes().map(RouteMix::index).collect();
        let mix_count = mix_slots.len();
        let source_slots = source_slots(next_aux, next.len());
        let route_count = source_slots.len() * mix_count;
        let mut retained_sources = vec![(0, 0)];
        if current_aux && next_aux { retained_sources.push((1, 1)); }
        let first_current_app = 1 + usize::from(current_aux);
        let first_next_app = 1 + usize::from(next_aux);
        for (index, app) in next.iter().enumerate() {
            if let Some(old) = current.iter().position(|other| Arc::ptr_eq(app, other)) {
                retained_sources.push((first_next_app + index, first_current_app + old));
            }
        }
        let retained_routes: Vec<_> = retained_sources.iter().flat_map(|&(next, old)| {
            (0..mix_count).map(move |mix| (next * mix_count + mix, old * mix_count + mix))
        }).collect();
        let retained_delays: Vec<_> = retained_routes.iter().copied().filter(|&(next, _)| {
            keep_aux_delay || source_slots[next / mix_count] != RouteSource::Aux.index()
        }).collect();
        let layout = VoiceDelayLayout::from_sources(&source_slots, &mix_slots);
        let compensation = if layout.compensated.contains(&true) { total_voice_latency } else { 0 };
        let frames: Vec<_> = layout.compensated.iter().enumerate().map(|(index, &compensated)| {
            // Retained lines are swapped in on the worker; only new or replaced
            // sources need delay storage prepared on the control thread.
            if compensated && !retained_delays.iter().any(|&(next, _)| next == index) { total_voice_latency } else { 0 }
        }).collect();
        let mut engine = routing_engine(next_aux, next.len(), mix_count)?;
        engine.swap_send_delays(&mut SendDelays::new(&frames)).map_err(|error| error.to_string())?;
        let reset_aux = !current_aux && next_aux;
        let mut targets = controls.route_targets(&next);
        let aux = RouteSource::Aux.index();
        if reset_aux { targets[aux * MAX_MIXES..(aux + 1) * MAX_MIXES].fill(0.0); }
        let route_ramps = std::array::from_fn(|index| GainRamp::new(rate,
            if index < route_count { targets[source_slots[index / mix_count] * MAX_MIXES + mix_slots[index % mix_count]] } else { 0.0 }));
        let mut source_mutes = controls.source_targets(&next).map(|gain| GainRamp::new(rate, gain));
        let mut source_trims = controls.trim_targets(&next).map(|gain| GainRamp::new(rate, gain));
        if reset_aux {
            source_mutes[aux] = GainRamp::new(rate, 1.0);
            source_trims[aux] = GainRamp::new(rate, 10.0_f32.powf(-12.0 / 20.0));
        }
        Ok(Self { engine, aux_enabled: next_aux, reset_aux, aux_changed: !keep_aux_delay, app_controls: next, source_slots, source_mutes, source_trims,
            route_count, route_ramps, retained_sources, retained_routes, retained_delays, layout,
            voice_latency: total_voice_latency, compensation })
    }

    pub(crate) fn delay_layout(&self) -> VoiceDelayLayout { self.layout.clone() }
    pub(crate) fn compensation(&self) -> usize { self.compensation }
}

fn source_slots(has_aux: bool, app_count: usize) -> Vec<usize> {
    [0].into_iter().chain(has_aux.then_some(1)).chain((0..app_count).map(|index| 2 + index)).collect()
}

fn routing_engine(has_aux: bool, app_count: usize, mix_count: usize) -> Result<Engine, String> {
    let mut scene = Scene {
        sources: vec![Source { name: "Processed mic".into(), channels: ChannelLayout::Stereo, gain_db: 0.0, muted: false }],
        mixes: (0..mix_count).map(|index| Mix {
            name: format!("Output {index}"), gain_db: 0.0, muted: false, excluded_sources: vec![],
        }).collect(),
        sends: (0..mix_count).map(|mix| Send { source: 0, mix, gain_db: 0.0, enabled: true }).collect(),
    };
    if has_aux {
        scene.sources.push(Source { name: "AUX".into(), channels: ChannelLayout::Stereo, gain_db: 0.0, muted: false });
        scene.sends.extend((0..mix_count).map(|mix| Send { source: 1, mix, gain_db: 0.0, enabled: true }));
    }
    for index in 0..app_count {
        let source = scene.sources.len();
        scene.sources.push(Source { name: format!("App {}", index + 1), channels: ChannelLayout::Stereo, gain_db: 0.0, muted: false });
        scene.sends.extend((0..mix_count).map(|mix| Send { source, mix, gain_db: 0.0, enabled: true }));
    }
    Engine::new(&scene, CAPTURE_BLOCK).map_err(|error| error.to_string())
}

#[derive(Default)]
pub struct VocalStats {
    peak: AtomicU32,
    compression: AtomicU32,
    deessing: AtomicU32,
}
impl VocalStats {
    fn record(&self, meters: Meters) {
        self.peak.fetch_max(meters.output_peak.to_bits(), Ordering::Relaxed);
        self.compression.fetch_max(meters.compression_db.to_bits(), Ordering::Relaxed);
        self.deessing.fetch_max(meters.deessing_db.to_bits(), Ordering::Relaxed);
    }
    /// UI/control thread: built-in DSP maxima before optional VST3, not loudness.
    pub fn take(&self) -> Meters {
        Meters { output_peak: f32::from_bits(self.peak.swap(0, Ordering::Relaxed)),
            compression_db: f32::from_bits(self.compression.swap(0, Ordering::Relaxed)),
            deessing_db: f32::from_bits(self.deessing.swap(0, Ordering::Relaxed)) }
    }
}

/// Process the voice once, then render independent destination mixes on the input timeline.
pub struct Capture {
    engine: Engine,
    voice_latency: usize,
    compensation: usize,
    vocal: VocalChain,
    pitch: Option<PitchCorrector>,
    pitch_config: PitchConfig,
    corrected_mic: [f32; CAPTURE_BLOCK],
    voice_left: [f32; CAPTURE_BLOCK],
    voice_right: [f32; CAPTURE_BLOCK],
    aux_enabled: bool,
    aux_left: [f32; CAPTURE_BLOCK],
    aux_right: [f32; CAPTURE_BLOCK],
    app_controls: Vec<Arc<AppControls>>,
    app_buffers: [[[f32; CAPTURE_BLOCK]; 2]; MAX_APPS],
    controls: Arc<LiveControls>,
    // Core indices are compact; control slots retain mic/AUX/app identity.
    source_slots: Vec<usize>,
    mix_slots: Vec<usize>,
    source_mutes: [GainRamp; MAX_SOURCES],
    source_trims: [GainRamp; MAX_SOURCES],
    route_count: usize,
    route_ramps: [GainRamp; MAX_ROUTES],
    route_envelopes: [[f32; CAPTURE_BLOCK]; MAX_ROUTES],
    outputs: Vec<(OutputWriter, Vec<usize>)>,
    health: Arc<Health>,
    pub vocal_stats: Arc<VocalStats>,
}
impl Capture {
    pub fn new(vocal: VocalChain, sample_rate: u32, mut outputs: Vec<(OutputWriter, Vec<usize>)>, health: Arc<Health>, controls: Arc<LiveControls>) -> Result<Self, String> {
        let mix_slots: Vec<_> = controls.mixes().map(RouteMix::index).collect();
        let mix_count = mix_slots.len();
        if !(8_000..=192_000).contains(&sample_rate) {
            return Err("unsupported capture sample rate".into());
        }
        if vocal.config() != controls.vocal_config() {
            return Err("vocal controls do not match the prepared processing configuration".into());
        }
        if outputs.is_empty() || outputs.iter().any(|(writer, mixes)| {
            mixes.len() != writer.mix_count() || mixes.iter().any(|mix| !mix_slots.contains(mix))
        }) { return Err("endpoint mix bindings do not match their clock bridges".into()); }
        // Map stable logical output slots to the compact core indices once, before callbacks.
        for (_, bindings) in &mut outputs {
            for mix in bindings {
                *mix = mix_slots.iter().position(|slot| *slot == *mix).expect("validated output mix");
            }
        }
        let app_controls = controls.app_controls();
        let engine = routing_engine(controls.has_aux(), app_controls.len(), mix_count)?;
        let source_slots = source_slots(controls.has_aux(), app_controls.len());
        let route_count = source_slots.len() * mix_count;
        let targets = controls.route_targets(&app_controls);
        let route_ramps = std::array::from_fn(|index| GainRamp::new(sample_rate,
            if index < route_count { targets[source_slots[index / mix_count] * MAX_MIXES + mix_slots[index % mix_count]] } else { 0.0 }));
        let source_mutes = controls.source_targets(&app_controls).map(|gain| GainRamp::new(sample_rate, gain));
        let source_trims = controls.trim_targets(&app_controls).map(|gain| GainRamp::new(sample_rate, gain));
        Ok(Self { engine, voice_latency: 0, compensation: 0, vocal, pitch: None, pitch_config: PitchConfig::default(), corrected_mic: [0.0; CAPTURE_BLOCK], voice_left: [0.0; CAPTURE_BLOCK], voice_right: [0.0; CAPTURE_BLOCK],
            aux_enabled: controls.has_aux(), aux_left: [0.0; CAPTURE_BLOCK], aux_right: [0.0; CAPTURE_BLOCK],
            app_controls, app_buffers: [[[0.0; CAPTURE_BLOCK]; 2]; MAX_APPS], outputs,
            controls, source_slots, mix_slots, source_mutes, source_trims, route_count, route_ramps, route_envelopes: [[0.0; CAPTURE_BLOCK]; MAX_ROUTES], health,
            vocal_stats: Arc::new(VocalStats::default()) })
    }
    #[cfg(windows)]
    pub(crate) fn app_count(&self) -> usize { self.app_controls.len() }

    #[cfg(windows)]
    pub(crate) fn has_aux(&self) -> bool { self.aux_enabled }

    #[cfg(any(windows, test))]
    pub(crate) fn app_controls(&self) -> Vec<Arc<AppControls>> { self.app_controls.clone() }

    /// The worker applies source identity, routing and histories at one block boundary.
    /// Retained ramps are copied before swapping arrays so compacting indices cannot
    /// overwrite a later source. The prepared plan takes every retired allocation.
    #[cfg(any(windows, test))]
    pub(crate) fn apply_source_change(&mut self, prepared: &mut PreparedSourceChange) {
        assert_eq!(self.voice_latency, prepared.voice_latency, "prepared source routing voice latency");
        prepared.engine.swap_retained_send_delays(&mut self.engine, &prepared.retained_delays)
            .expect("prepared retained source mapping");
        for &(next, old) in &prepared.retained_sources {
            let next = prepared.source_slots[next];
            let old = self.source_slots[old];
            prepared.source_mutes[next] = self.source_mutes[old];
            prepared.source_trims[next] = self.source_trims[old];
        }
        for &(next, old) in &prepared.retained_routes { prepared.route_ramps[next] = self.route_ramps[old]; }
        if prepared.reset_aux { self.controls.reset_aux_for_add(); }
        if prepared.aux_changed { self.health.aux_peak.store(0, Ordering::Relaxed); }
        std::mem::swap(&mut self.engine, &mut prepared.engine);
        std::mem::swap(&mut self.aux_enabled, &mut prepared.aux_enabled);
        std::mem::swap(&mut self.app_controls, &mut prepared.app_controls);
        std::mem::swap(&mut self.source_slots, &mut prepared.source_slots);
        std::mem::swap(&mut self.source_mutes, &mut prepared.source_mutes);
        std::mem::swap(&mut self.source_trims, &mut prepared.source_trims);
        std::mem::swap(&mut self.route_count, &mut prepared.route_count);
        std::mem::swap(&mut self.route_ramps, &mut prepared.route_ramps);
        std::mem::swap(&mut self.compensation, &mut prepared.compensation);
    }

    #[cfg(any(windows, test))]
    pub(crate) fn vocal_config(&self) -> VocalConfig { self.vocal.config() }

    /// Prepared module changes share the worker boundary with other live edits.
    /// The plan retains removed processing storage for control-thread retirement.
    #[cfg(any(windows, test))]
    pub(crate) fn apply_vocal_change(&mut self, prepared: &mut PreparedVocalChange) {
        self.vocal.apply_settings(prepared);
    }

    /// Prepare the opt-in pitch stage before opening the microphone stream.
    pub fn prepare_pitch(&mut self, rate: u32, config: PitchConfig) -> Result<usize, String> {
        self.pitch = PitchCorrector::new(rate, config)?;
        self.pitch_config = config;
        Ok(self.pitch_latency())
    }

    pub(crate) fn pitch_config(&self) -> PitchConfig { self.pitch_config }

    /// Apply together with the prepared total voice delay at a block boundary.
    /// A removed corrector remains owned by the plan for control-thread retirement.
    #[cfg(any(windows, test))]
    pub(crate) fn apply_pitch_change(&mut self, prepared: &mut PreparedPitchChange) {
        if let Some(replacement) = &mut prepared.replacement {
            std::mem::swap(&mut self.pitch, replacement);
        } else if let Some(pitch) = &mut self.pitch {
            pitch.set_config(prepared.config);
        }
        self.pitch_config = prepared.config;
    }

    pub fn pitch_latency(&self) -> usize {
        self.pitch.as_ref().map_or(0, PitchCorrector::latency)
    }

    #[cfg(any(windows, test))]
    pub(crate) fn delay_layout(&self) -> VoiceDelayLayout {
        VoiceDelayLayout::from_sources(&self.source_slots, &self.mix_slots)
    }

    /// Swap together with the VST3 chain at a block boundary. The caller takes the
    /// old allocation back to the control thread. Equal latency retains its history.
    #[cfg(any(windows, test))]
    pub(crate) fn swap_voice_delay(&mut self, prepared: &mut PreparedVoiceDelay) {
        if self.voice_latency == prepared.latency { return; }
        self.engine.swap_send_delays(&mut prepared.delays).expect("prepared capture routing layout");
        std::mem::swap(&mut self.voice_latency, &mut prepared.latency);
        std::mem::swap(&mut self.compensation, &mut prepared.compensation);
    }

    /// Test setup uses the same control preparation and worker swap as a live edit.
    #[cfg(test)]
    pub(crate) fn prepare_voice_delay(&mut self, vst3_latency: u64, rate: u32) -> Result<usize, String> {
        let total_latency = vst3_latency.checked_add(self.pitch_latency() as u64)
            .ok_or("voice chain latency exceeds the supported 2-second outgoing compensation")?;
        let mut prepared = self.delay_layout().prepare(total_latency, rate)?;
        let compensation = prepared.compensation();
        self.swap_voice_delay(&mut prepared);
        Ok(compensation)
    }

    /// Device-free fixture input. Production uses the single worker's effect seam.
    #[cfg(test)]
    pub fn push(&mut self, samples: &[f32], aux: Option<&[[f32; 2]]>, apps: &[&[[f32; 2]]]) {
        self.push_with_effect(samples, aux, apps, &mut |_, _| true);
    }
    /// External effect seam used by the dedicated VST3 worker, never by a device callback.
    /// The effect transforms the built-in stereo voice in place. False/nonfinite output latches a fault.
    pub(crate) fn push_with_effect(&mut self, samples: &[f32], aux: Option<&[[f32; 2]]>, apps: &[&[[f32; 2]]],
        effect: &mut impl FnMut(&mut [f32], &mut [f32]) -> bool) {
        if self.health.failed() { return; }
        if aux.is_some() != self.aux_enabled || aux.is_some_and(|frames| frames.len() != samples.len())
            || apps.len() != self.app_controls.len() || apps.iter().any(|frames| frames.len() != samples.len()) {
            self.health.fail(Fault::Input); return;
        }
        for (block_index, block) in samples.chunks(CAPTURE_BLOCK).enumerate() {
            // The control/UI settings are published only after acknowledgment.
            // Use the preset already applied here, never its mutex or stale preset.
            let preset = self.vocal.config().preset;
            for parameter in VocalParameter::ALL.into_iter().filter(|parameter| parameter.available_for(preset)) {
                if self.vocal.set_parameter(parameter, self.controls.vocal_value(parameter)).is_err() {
                    self.health.fail(Fault::Routing); return;
                }
            }
            for (ramp, target) in self.source_mutes.iter_mut().zip(self.controls.source_targets(&self.app_controls)) { ramp.set(target); }
            for (ramp, target) in self.source_trims.iter_mut().zip(self.controls.trim_targets(&self.app_controls)) { ramp.set(target); }
            let targets = self.controls.route_targets(&self.app_controls);
            let mix_count = self.mix_slots.len();
            for (index, ramp) in self.route_ramps[..self.route_count].iter_mut().enumerate() {
                ramp.set(targets[self.source_slots[index / mix_count] * MAX_MIXES + self.mix_slots[index % mix_count]]);
                for gain in &mut self.route_envelopes[index][..block.len()] { *gain = ramp.next_gain(); }
            }
            // Source mute is shared by all of its sends, and acts on the
            // rendered timeline after each prepared send delay. Advance once per
            // source sample, not once per destination.
            for (source_index, &slot) in self.source_slots.iter().enumerate() {
                let first = source_index * mix_count;
                for frame in 0..block.len() {
                    let gain = self.source_mutes[slot].next_gain();
                    for envelope in &mut self.route_envelopes[first..first + mix_count] {
                        envelope[frame] *= gain;
                    }
                }
            }
            let envelopes: [&[f32]; MAX_ROUTES] = std::array::from_fn(|index| &self.route_envelopes[index][..block.len()]);
            let envelopes = &envelopes[..self.route_count];
            let left = &mut self.voice_left[..block.len()];
            let right = &mut self.voice_right[..block.len()];
            let mic = if let Some(pitch) = &mut self.pitch {
                let corrected = &mut self.corrected_mic[..block.len()];
                if !pitch.process(block, corrected) { self.health.fail(Fault::Input); return; }
                &*corrected
            } else { block };
            let Ok(meters) = self.vocal.process(mic, left, right) else { self.health.fail(Fault::Input); return; };
            if !effect(left, right) || left.iter().chain(right.iter()).any(|s| !s.is_finite()) {
                self.health.fail(Fault::Routing); return;
            }
            // Keep pitch and both DSP stages running while source-muted, including tail/history.
            let voice_peak = left.iter().chain(right.iter()).fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
            self.health.voice_peak.fetch_max(voice_peak.to_bits(), Ordering::Relaxed);
            let mut inputs = [Input::Mono(&[]); MAX_SOURCES];
            inputs[0] = Input::Stereo { left, right };
            let start = block_index * CAPTURE_BLOCK;
            let mut next = 1;
            if let Some(aux) = aux {
                let mut peak = 0.0_f32;
                for (index, frame) in aux[start..start + block.len()].iter().enumerate() {
                    // Trim belongs to the source timeline, before per-send delay and mute.
                    let gain = self.source_trims[RouteSource::Aux.index()].next_gain();
                    self.aux_left[index] = frame[0] * gain;
                    self.aux_right[index] = frame[1] * gain;
                    peak = peak.max(frame[0].abs()).max(frame[1].abs());
                }
                self.health.aux_peak.fetch_max(peak.to_bits(), Ordering::Relaxed);
                inputs[next] = Input::Stereo {
                    left: &self.aux_left[..block.len()], right: &self.aux_right[..block.len()],
                };
                next += 1;
            }
            for (slot, (buffer, app)) in self.app_buffers.iter_mut().zip(apps).enumerate() {
                let mut peak = 0.0_f32;
                for (index, frame) in app[start..start + block.len()].iter().enumerate() {
                    let gain = self.source_trims[RouteSource::App(slot).index()].next_gain();
                    buffer[0][index] = frame[0] * gain;
                    buffer[1][index] = frame[1] * gain;
                    peak = peak.max(frame[0].abs()).max(frame[1].abs());
                }
                self.app_controls[slot].record_peak(peak);
            }
            for (slot, buffer) in self.app_buffers.iter().take(self.app_controls.len()).enumerate() {
                inputs[next + slot] = Input::Stereo { left: &buffer[0][..block.len()], right: &buffer[1][..block.len()] };
            }
            let rendered = self.engine.render_with_send_gains(block.len(), &inputs[..self.source_slots.len()], envelopes);
            if rendered.is_err() { self.health.fail(Fault::Input); return; }
            let peak = block.iter().fold(0.0_f32, |p, x| p.max(x.abs()));
            self.health.input_peak.fetch_max(peak.to_bits(), Ordering::Relaxed);
            self.vocal_stats.record(meters);
            for (writer, mixes) in &mut self.outputs {
                for frame in 0..block.len() {
                    let mut packed = [0.0; MAX_MIXES * 2];
                    for (lane, mix) in mixes.iter().enumerate() {
                        let Some(output) = self.engine.output(*mix) else { self.health.fail(Fault::Routing); return; };
                        packed[lane * 2] = output.left[frame]; packed[lane * 2 + 1] = output.right[frame];
                    }
                    writer.push(packed);
                }
            }
            // Publish completion only after every output received this block.
            self.health.captured_frames.fetch_add(block.len() as u64, Ordering::Release);
        }
    }
}

#[cfg(test)]
#[path = "capture_tests.rs"]
mod tests;
