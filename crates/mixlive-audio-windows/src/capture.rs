use std::sync::{Arc, atomic::{AtomicU32, Ordering}};
use mixlive_core::{ChannelLayout, Engine, Input, Mix, Scene, Send, Source};
use mixlive_effects::{Meters, VocalChain};
use crate::{bridge::{Fault, Health, OutputWriter}, config::{MAX_APPS, MAX_MIXES, PitchConfig}, pitch_correction::PitchCorrector, control::{GainRamp, LiveControls, RouteMix, RouteSource, MAX_SOURCES, MAX_ROUTES}};

pub const CAPTURE_BLOCK: usize = 256;

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
    vocal: VocalChain,
    pitch: Option<PitchCorrector>,
    corrected_mic: [f32; CAPTURE_BLOCK],
    voice_left: [f32; CAPTURE_BLOCK],
    voice_right: [f32; CAPTURE_BLOCK],
    aux_enabled: bool,
    aux_left: [f32; CAPTURE_BLOCK],
    aux_right: [f32; CAPTURE_BLOCK],
    app_count: usize,
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
        let mut scene = Scene {
            // The built-in chain produces stereo, and VST3 preserves both channels.
            sources: vec![Source { name: "Processed mic".into(), channels: ChannelLayout::Stereo, gain_db: 0.0, muted: false }],
            mixes: (0..mix_count).map(|index| Mix {
                name: format!("Output {index}"), gain_db: 0.0, muted: false, excluded_sources: vec![],
            }).collect(),
            sends: (0..mix_count).map(|mix| Send { source: 0, mix, gain_db: 0.0, enabled: true }).collect(),
        };
        if controls.has_aux() {
            scene.sources.push(Source { name: "AUX".into(), channels: ChannelLayout::Stereo, gain_db: 0.0, muted: false });
            scene.sends.extend((0..mix_count).map(|mix| Send { source: 1, mix, gain_db: 0.0, enabled: true }));
        }
        for index in 0..controls.app_count() {
            let source = scene.sources.len();
            scene.sources.push(Source { name: format!("App {}", index + 1), channels: ChannelLayout::Stereo, gain_db: 0.0, muted: false });
            scene.sends.extend((0..mix_count).map(|mix| Send { source, mix, gain_db: 0.0, enabled: true }));
        }
        let engine = Engine::new(&scene, CAPTURE_BLOCK).map_err(|e| e.to_string())?;
        let source_slots: Vec<_> = controls.sources().map(RouteSource::index).collect();
        let route_count = scene.sends.len();
        let targets = controls.route_targets();
        let route_ramps = std::array::from_fn(|index| GainRamp::new(sample_rate,
            if index < route_count { targets[source_slots[index / mix_count] * MAX_MIXES + mix_slots[index % mix_count]] } else { 0.0 }));
        let source_mutes = controls.source_targets().map(|gain| GainRamp::new(sample_rate, gain));
        let source_trims = controls.trim_targets().map(|gain| GainRamp::new(sample_rate, gain));
        Ok(Self { engine, vocal, pitch: None, corrected_mic: [0.0; CAPTURE_BLOCK], voice_left: [0.0; CAPTURE_BLOCK], voice_right: [0.0; CAPTURE_BLOCK],
            aux_enabled: controls.has_aux(), aux_left: [0.0; CAPTURE_BLOCK], aux_right: [0.0; CAPTURE_BLOCK],
            app_count: controls.app_count(), app_buffers: [[[0.0; CAPTURE_BLOCK]; 2]; MAX_APPS], outputs,
            controls, source_slots, mix_slots, source_mutes, source_trims, route_count, route_ramps, route_envelopes: [[0.0; CAPTURE_BLOCK]; MAX_ROUTES], health,
            vocal_stats: Arc::new(VocalStats::default()) })
    }
    #[cfg(windows)]
    pub(crate) fn app_count(&self) -> usize { self.app_count }

    /// Prepare the opt-in pitch stage before opening the microphone stream.
    pub fn prepare_pitch(&mut self, rate: u32, config: PitchConfig) -> Result<usize, String> {
        self.pitch = PitchCorrector::new(rate, config)?;
        Ok(self.pitch_latency())
    }

    pub fn pitch_latency(&self) -> usize {
        self.pitch.as_ref().map_or(0, PitchCorrector::latency)
    }

    /// Add the built-in pitch delay to the final VST3 latency on the mic clock.
    /// Only outgoing supplemental sources are delayed; never delay them twice.
    #[cfg(any(windows, test))]
    pub(crate) fn prepare_voice_delay(&mut self, vst3_latency: u64, rate: u32) -> Result<usize, String> {
        let latency = vst3_latency + self.pitch_latency() as u64;
        // At the maximum rate and source count the two-second cap bounds storage
        // for all compensated stereo sends to about 53 MiB.
        if latency > u64::from(rate) * 2 {
            return Err("voice chain latency exceeds the supported 2-second outgoing compensation; remove high-latency effects and start again".into());
        }
        if latency == 0 || self.source_slots.len() == 1 || self.mix_slots.len() == 1 { return Ok(0); }
        let frames = latency as usize;
        let mix_count = self.mix_slots.len();
        let delays: Vec<_> = (0..self.route_count).map(|index| {
            if self.source_slots[index / mix_count] != 0 && self.mix_slots[index % mix_count] != 0 {
                frames
            } else { 0 }
        }).collect();
        self.engine.prepare_send_delays(&delays).map_err(|error| error.to_string())?;
        Ok(frames)
    }

    /// Built-in-only route. All supplemental sources are converted to the mic clock.
    pub fn push(&mut self, samples: &[f32], aux: Option<&[[f32; 2]]>, apps: &[&[[f32; 2]]]) {
        self.push_with_effect(samples, aux, apps, &mut |_, _| true);
    }
    /// External effect seam used by the dedicated VST3 worker, never by a device callback.
    /// The effect transforms the built-in stereo voice in place. False/nonfinite output latches a fault.
    pub(crate) fn push_with_effect(&mut self, samples: &[f32], aux: Option<&[[f32; 2]]>, apps: &[&[[f32; 2]]],
        effect: &mut impl FnMut(&mut [f32], &mut [f32]) -> bool) {
        if self.health.failed() { return; }
        if aux.is_some() != self.aux_enabled || aux.is_some_and(|frames| frames.len() != samples.len())
            || apps.len() != self.app_count || apps.iter().any(|frames| frames.len() != samples.len()) {
            self.health.fail(Fault::Input); return;
        }
        for (block_index, block) in samples.chunks(CAPTURE_BLOCK).enumerate() {
            for parameter in self.controls.vocal_parameters() {
                if self.vocal.set_parameter(parameter, self.controls.vocal_value(parameter)).is_err() {
                    self.health.fail(Fault::Routing); return;
                }
            }
            for (ramp, target) in self.source_mutes.iter_mut().zip(self.controls.source_targets()) { ramp.set(target); }
            for (ramp, target) in self.source_trims.iter_mut().zip(self.controls.trim_targets()) { ramp.set(target); }
            let targets = self.controls.route_targets();
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
                self.health.app_peaks[slot].fetch_max(peak.to_bits(), Ordering::Relaxed);
            }
            for (slot, buffer) in self.app_buffers.iter().take(self.app_count).enumerate() {
                inputs[next + slot] = Input::Stereo { left: &buffer[0][..block.len()], right: &buffer[1][..block.len()] };
            }
            let rendered = self.engine.render_with_send_gains(block.len(), &inputs[..self.source_slots.len()], envelopes);
            if rendered.is_err() { self.health.fail(Fault::Input); return; }
            let peak = block.iter().fold(0.0_f32, |p, x| p.max(x.abs()));
            self.health.input_peak.fetch_max(peak.to_bits(), Ordering::Relaxed);
            self.health.captured_frames.fetch_add(block.len() as u64, Ordering::Relaxed);
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
        }
    }
}

#[cfg(test)]
#[path = "capture_tests.rs"]
mod tests;
