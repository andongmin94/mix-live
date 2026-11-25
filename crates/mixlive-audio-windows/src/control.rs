//! Live session controls. Parsing/printing stays on the control thread; callbacks only load atomics.
use crate::config::{AuxDestination, RouteOptions, MAX_APPS, MAX_MIXES};
use serde::{Deserialize, Serialize};
use mixlive_effects::{VocalConfig, VocalParameter};
use std::{str::FromStr, sync::{Arc, Mutex, atomic::{AtomicBool, AtomicU32, Ordering}}};

const MIC: u32 = 1;
const AUX: u32 = 2;
const OUTPUT: u32 = 4;
const SEND: u32 = 8;
const ALL: u32 = 16;
const BROADCAST: u32 = 1 << (5 + MAX_APPS);

pub(crate) const MAX_SOURCES: usize = MAX_APPS + 2;
pub(crate) const MAX_ROUTES: usize = MAX_MIXES * MAX_SOURCES;

pub const HELP: &str = "Live commands: mute/unmute mic|aux|app1..app8|output|send|broadcast|all; gain master|monitor|send|broadcast <dB -96..0>; route mic|aux|appN output|send|broadcast on|off; level mic|aux|appN output|send|broadcast <dB -96..0>; trim aux|appN <dB -96..0>; bypass <VST3 slot> on|off; vocal trim|strength|eq-hpf|eq-body|eq-presence|eq-air|compressor|deesser|ratio|attack|release|deess-max|reverb|decay|echo|echo-feedback <value>; status; stop. Changes last for this run unless saved with --scene-out.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MuteTarget { Mic, Aux, App(usize), Output, Send, Broadcast, All }
impl MuteTarget {
    fn bit(self) -> Option<u32> {
        Some(match self { Self::Mic => MIC, Self::Aux => AUX, Self::Output => OUTPUT, Self::Send => SEND, Self::Broadcast => BROADCAST, Self::All => ALL,
            Self::App(index) if index < MAX_APPS => 1 << (5 + index), Self::App(_) => return None })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GainTarget { Master, Monitor, Send, Broadcast }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteSource { Mic, Aux, App(usize) }
impl RouteSource {
    pub(crate) fn index(self) -> usize { match self { Self::Mic => 0, Self::Aux => 1, Self::App(index) => 2 + index } }
    pub fn mute_target(self) -> MuteTarget {
        match self { Self::Mic => MuteTarget::Mic, Self::Aux => MuteTarget::Aux, Self::App(index) => MuteTarget::App(index) }
    }
    fn name(self) -> String {
        match self { Self::Mic => "mic".into(), Self::Aux => "aux".into(), Self::App(index) => format!("app{}", index + 1) }
    }
}

fn app_slot(word: &str) -> Option<usize> {
    word.strip_prefix("app")?.parse::<usize>().ok()?.checked_sub(1).filter(|&index| index < MAX_APPS)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteMix { Output, Send, Broadcast }
impl RouteMix {
    pub const ALL: [Self; MAX_MIXES] = [Self::Output, Self::Send, Self::Broadcast];
    pub fn index(self) -> usize { match self { Self::Output => 0, Self::Send => 1, Self::Broadcast => 2 } }
    pub fn mute_target(self) -> MuteTarget {
        match self { Self::Output => MuteTarget::Output, Self::Send => MuteTarget::Send, Self::Broadcast => MuteTarget::Broadcast }
    }
    pub fn gain_target(self) -> GainTarget {
        match self { Self::Output => GainTarget::Monitor, Self::Send => GainTarget::Send, Self::Broadcast => GainTarget::Broadcast }
    }
    fn name(self) -> &'static str {
        match self { Self::Output => "output", Self::Send => "send", Self::Broadcast => "broadcast" }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LiveCommand {
    Mute { target: MuteTarget, muted: bool },
    Gain { target: GainTarget, db: f32 },
    Route { source: RouteSource, mix: RouteMix, enabled: bool },
    RouteGain { source: RouteSource, mix: RouteMix, db: f32 },
    SourceTrim { source: RouteSource, db: f32 },
    EffectBypass { index: usize, bypassed: bool },
    Vocal { parameter: VocalParameter, value: f32 },
    Status,
    Stop,
}
impl FromStr for LiveCommand {
    type Err = String;
    fn from_str(line: &str) -> Result<Self, Self::Err> {
        let mut words = line.split_whitespace();
        let command = match words.next() {
            Some(verb @ ("mute" | "unmute")) => {
                let target = match words.next() {
                    Some("mic") => MuteTarget::Mic, Some("aux") => MuteTarget::Aux,
                    Some("output") => MuteTarget::Output, Some("send") => MuteTarget::Send, Some("broadcast") => MuteTarget::Broadcast,
                    Some("all") => MuteTarget::All,
                    Some(word) => MuteTarget::App(app_slot(word).ok_or(HELP)?), None => return Err(HELP.into()),
                };
                Self::Mute { target, muted: verb == "mute" }
            }
            Some("gain") => {
                let target = match words.next() {
                    Some("master") => GainTarget::Master, Some("monitor") => GainTarget::Monitor,
                    Some("send") => GainTarget::Send, Some("broadcast") => GainTarget::Broadcast,
                    _ => return Err(HELP.into()),
                };
                let db: f32 = words.next().ok_or(HELP)?.parse().map_err(|_| HELP)?;
                validate_db(db)?;
                Self::Gain { target, db }
            }
            Some(verb @ ("route" | "level")) => {
                let source = match words.next() {
                    Some("mic") => RouteSource::Mic, Some("aux") => RouteSource::Aux,
                    Some(word) => RouteSource::App(app_slot(word).ok_or(HELP)?), None => return Err(HELP.into()),
                };
                let mix = match words.next() {
                    Some("output") => RouteMix::Output, Some("send") => RouteMix::Send, Some("broadcast") => RouteMix::Broadcast,
                    _ => return Err(HELP.into()),
                };
                if verb == "route" {
                    let enabled = match words.next() {
                        Some("on") => true, Some("off") => false, _ => return Err(HELP.into()),
                    };
                    Self::Route { source, mix, enabled }
                } else {
                    let db: f32 = words.next().ok_or(HELP)?.parse().map_err(|_| HELP)?;
                    validate_db(db)?;
                    Self::RouteGain { source, mix, db }
                }
            }
            Some("trim") => {
                let source = match words.next() {
                    Some("aux") => RouteSource::Aux,
                    Some(word) => RouteSource::App(app_slot(word).ok_or(HELP)?), None => return Err(HELP.into()),
                };
                let db: f32 = words.next().ok_or(HELP)?.parse().map_err(|_| HELP)?;
                validate_db(db)?;
                Self::SourceTrim { source, db }
            }
            Some("bypass") => {
                let index = words.next().ok_or(HELP)?.parse::<usize>().map_err(|_| HELP)?.checked_sub(1).ok_or(HELP)?;
                let bypassed = match words.next() {
                    Some("on") => true, Some("off") => false, _ => return Err(HELP.into()),
                };
                Self::EffectBypass { index, bypassed }
            }
            Some("vocal") => {
                let name = words.next().ok_or(HELP)?;
                let parameter = VocalParameter::ALL.into_iter().find(|parameter| parameter.name() == name).ok_or(HELP)?;
                let value: f32 = words.next().ok_or(HELP)?.parse().map_err(|_| HELP)?;
                parameter.validate(value).map_err(|error| error.to_string())?;
                Self::Vocal { parameter, value }
            }
            Some("status") => Self::Status,
            Some("stop") => Self::Stop,
            _ => return Err(HELP.into()),
        };
        if words.next().is_some() { return Err(HELP.into()); }
        Ok(command)
    }
}

fn validate_db(db: f32) -> Result<(), String> {
    if !db.is_finite() || !(-96.0..=0.0).contains(&db) {
        return Err("live gain must be finite and between -96 and 0 dB".into());
    }
    Ok(())
}
fn gain(db: f32) -> f32 { 10.0_f32.powf(db / 20.0) }

/// Settings that cannot be represented by the device bindings and initial master/send gain.
/// Inactive optional routes retain their values without creating additional audio paths.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MixSettings {
    pub mute_mic: bool,
    pub mute_aux: bool,
    pub mute_output: bool,
    pub mute_send: bool,
    pub mute_broadcast: bool,
    pub mute_all: bool,
    pub mic_output_enabled: bool,
    pub mic_send_enabled: bool,
    pub mic_broadcast_enabled: bool,
    pub mic_output_gain_db: f32,
    pub mic_send_gain_db: f32,
    pub mic_broadcast_gain_db: f32,
    pub aux_output_gain_db: f32,
    pub aux_send_gain_db: f32,
    pub aux_broadcast_gain_db: f32,
}
impl Default for MixSettings {
    fn default() -> Self {
        Self { mute_mic: false, mute_aux: false, mute_output: false, mute_send: false, mute_broadcast: false, mute_all: false,
            mic_output_enabled: true, mic_send_enabled: true, mic_broadcast_enabled: true, mic_output_gain_db: 0.0,
            mic_send_gain_db: 0.0, mic_broadcast_gain_db: 0.0, aux_output_gain_db: 0.0, aux_send_gain_db: 0.0, aux_broadcast_gain_db: 0.0 }
    }
}
impl MixSettings {
    pub(crate) fn validate(&self) -> Result<(), String> {
        for db in [self.mic_output_gain_db, self.mic_send_gain_db, self.mic_broadcast_gain_db,
            self.aux_output_gain_db, self.aux_send_gain_db, self.aux_broadcast_gain_db] { validate_db(db)?; }
        Ok(())
    }
}

/// One session, initialized from its route. Neither commands nor runtime mutes rewrite a Scene.
/// Individual controls are independent atomic values, not an atomic multi-parameter Scene swap.
pub struct LiveControls {
    muted: AtomicU32,
    stop_requested: AtomicBool,
    master: AtomicU32,
    mix_gains: [AtomicU32; MAX_MIXES],
    enabled_mixes: [bool; MAX_MIXES],
    has_aux: bool,
    app_count: usize,
    // Fixed slot order: mic, physical AUX, application slots; three destinations per slot.
    // Enabled state is independent of fader level and global source/output mutes.
    routes_enabled: AtomicU32,
    route_levels: [AtomicU32; MAX_ROUTES],
    source_trims: [AtomicU32; MAX_SOURCES],
    // Only foreground/CLI code accesses this list. Processing keeps each flag's
    // Arc directly and never takes this lock when the slot order changes.
    effect_bypasses: Mutex<Vec<Arc<AtomicBool>>>,
    vocal_start: VocalConfig,
    vocal_targets: [AtomicU32; VocalParameter::ALL.len()],
}
impl LiveControls {
    pub fn new(master_db: f32, mix_db: [Option<f32>; MAX_MIXES], has_aux: bool, app_count: usize,
        vocal: VocalConfig) -> Result<Self, String> {
        vocal.validate().map_err(|error| error.to_string())?;
        if app_count > MAX_APPS { return Err("too many application sources".into()); }
        if mix_db[0].is_none() { return Err("a monitor mix is required".into()); }
        validate_db(master_db)?;
        for db in mix_db.into_iter().flatten() { validate_db(db)?; }
        let enabled_mixes = mix_db.map(|db| db.is_some());
        let mic_routes = enabled_mixes.iter().enumerate().fold(0, |bits, (index, enabled)| {
            if *enabled { bits | (1 << index) } else { bits }
        });
        Ok(Self { muted: AtomicU32::new(0), stop_requested: AtomicBool::new(false),
            master: AtomicU32::new(gain(master_db).to_bits()),
            mix_gains: mix_db.map(|db| AtomicU32::new(gain(db.unwrap_or(0.0)).to_bits())),
            enabled_mixes, has_aux, app_count,
            routes_enabled: AtomicU32::new(mic_routes),
            route_levels: std::array::from_fn(|_| AtomicU32::new(1.0_f32.to_bits())),
            source_trims: std::array::from_fn(|_| AtomicU32::new(1.0_f32.to_bits())),
            effect_bypasses: Mutex::new(Vec::new()),
            vocal_start: vocal,
            vocal_targets: VocalParameter::ALL.map(|parameter| AtomicU32::new(vocal.parameter(parameter).to_bits())) })
    }

    /// Restore requested controls before constructing capture or playback ramps.
    pub fn from_route(route: &RouteOptions) -> Result<Self, String> {
        route.validate()?;
        let controls = Self::new(route.output_gain_db, [Some(route.monitor_gain_db),
            route.send.as_ref().map(|_| route.send_gain_db),
            route.broadcast.as_ref().map(|_| route.broadcast_gain_db)], route.has_aux(), route.apps.len(), route.vocal_config())?;
        let settings = route.mix;
        for (target, muted) in [(MuteTarget::Mic, settings.mute_mic), (MuteTarget::All, settings.mute_all)] {
            controls.apply(LiveCommand::Mute { target, muted })?;
        }
        if controls.has_aux {
            controls.apply(LiveCommand::Mute { target: MuteTarget::Aux, muted: settings.mute_aux })?;
            controls.apply(LiveCommand::SourceTrim { source: RouteSource::Aux, db: route.aux_gain_db })?;
        }
        for (mix, muted) in [(RouteMix::Output, settings.mute_output), (RouteMix::Send, settings.mute_send),
            (RouteMix::Broadcast, settings.mute_broadcast)] {
            if controls.has_mix(mix) { controls.apply(LiveCommand::Mute { target: mix.mute_target(), muted })?; }
        }
        for (source, mix, enabled, db) in [
            (RouteSource::Mic, RouteMix::Output, settings.mic_output_enabled, settings.mic_output_gain_db),
            (RouteSource::Mic, RouteMix::Send, settings.mic_send_enabled, settings.mic_send_gain_db),
            (RouteSource::Mic, RouteMix::Broadcast, settings.mic_broadcast_enabled, settings.mic_broadcast_gain_db),
            (RouteSource::Aux, RouteMix::Output, route.aux_to.includes(0), settings.aux_output_gain_db),
            (RouteSource::Aux, RouteMix::Send, route.aux_to.includes(1), settings.aux_send_gain_db),
            (RouteSource::Aux, RouteMix::Broadcast, route.aux_to.includes(2), settings.aux_broadcast_gain_db),
        ] {
            if (source == RouteSource::Aux && !controls.has_aux) || !controls.has_mix(mix) { continue; }
            controls.apply(LiveCommand::Route { source, mix, enabled })?;
            controls.apply(LiveCommand::RouteGain { source, mix, db })?;
        }
        for (index, app) in route.apps.iter().enumerate() {
            let source = RouteSource::App(index);
            controls.apply(LiveCommand::Mute { target: source.mute_target(), muted: app.muted })?;
            controls.apply(LiveCommand::SourceTrim { source, db: app.gain_db })?;
            for (mix, db) in [(RouteMix::Output, app.output_gain_db), (RouteMix::Send, app.send_gain_db),
                (RouteMix::Broadcast, app.broadcast_gain_db)] {
                if !controls.has_mix(mix) { continue; }
                controls.apply(LiveCommand::Route { source, mix, enabled: app.to.includes(mix.index()) })?;
                controls.apply(LiveCommand::RouteGain { source, mix, db })?;
            }
        }
        Ok(controls)
    }

    /// Capture requested values with edits suspended, preserving inactive optional settings.
    pub fn snapshot_into(&self, route: &mut RouteOptions) -> Result<(), String> {
        if self.has_aux != route.has_aux() || self.enabled_mixes != [true, route.send.is_some(), route.broadcast.is_some()]
            || self.app_count != route.apps.len() {
            return Err("live controls do not match the Scene topology".into());
        }
        if route.vocal != self.vocal_start.preset || route.reverb_predelay_ms != self.vocal_start.reverb.predelay_ms
            || route.echo_time_ms != self.vocal_start.echo.time_ms {
            return Err("live vocal processing does not match the Scene topology".into());
        }
        let mut saved = route.clone();
        saved.set_vocal_config(self.vocal_config());
        let db = |value: f32| value.clamp(-96.0, 0.0);
        saved.output_gain_db = db(self.gain_db(GainTarget::Master)?);
        saved.monitor_gain_db = db(self.gain_db(GainTarget::Monitor)?);
        if self.has_mix(RouteMix::Send) { saved.send_gain_db = db(self.gain_db(GainTarget::Send)?); }
        if self.has_mix(RouteMix::Broadcast) { saved.broadcast_gain_db = db(self.gain_db(GainTarget::Broadcast)?); }
        let settings = &mut saved.mix;
        settings.mute_mic = self.is_muted(MuteTarget::Mic);
        settings.mute_all = self.is_muted(MuteTarget::All);
        for mix in self.mixes() {
            let muted = self.is_muted(mix.mute_target());
            let (enabled, gain) = self.route_state(RouteSource::Mic, mix)?;
            match mix {
                RouteMix::Output => { settings.mute_output = muted; settings.mic_output_enabled = enabled; settings.mic_output_gain_db = db(gain); }
                RouteMix::Send => { settings.mute_send = muted; settings.mic_send_enabled = enabled; settings.mic_send_gain_db = db(gain); }
                RouteMix::Broadcast => { settings.mute_broadcast = muted; settings.mic_broadcast_enabled = enabled; settings.mic_broadcast_gain_db = db(gain); }
            }
        }
        if self.has_aux {
            saved.aux_gain_db = db(self.source_trim_db(RouteSource::Aux)?);
            settings.mute_aux = self.is_muted(MuteTarget::Aux);
            let mut enabled = [false; MAX_MIXES];
            for mix in self.mixes() {
                let (on, gain) = self.route_state(RouteSource::Aux, mix)?;
                enabled[mix.index()] = on;
                match mix {
                    RouteMix::Output => settings.aux_output_gain_db = db(gain),
                    RouteMix::Send => settings.aux_send_gain_db = db(gain),
                    RouteMix::Broadcast => settings.aux_broadcast_gain_db = db(gain),
                }
            }
            saved.aux_to = AuxDestination::from_enabled(enabled);
        }
        for (index, app) in saved.apps.iter_mut().enumerate() {
            let source = RouteSource::App(index);
            app.muted = self.is_muted(source.mute_target());
            app.gain_db = db(self.source_trim_db(source)?);
            let mut enabled = [false; MAX_MIXES];
            for mix in self.mixes() {
                let (on, gain) = self.route_state(source, mix)?;
                enabled[mix.index()] = on;
                match mix {
                    RouteMix::Output => app.output_gain_db = db(gain),
                    RouteMix::Send => app.send_gain_db = db(gain),
                    RouteMix::Broadcast => app.broadcast_gain_db = db(gain),
                }
            }
            app.to = AuxDestination::from_enabled(enabled);
        }
        saved.validate()?;
        *route = saved;
        Ok(())
    }

    pub fn has_mix(&self, mix: RouteMix) -> bool { self.enabled_mixes[mix.index()] }
    pub fn mixes(&self) -> impl Iterator<Item = RouteMix> + '_ {
        RouteMix::ALL.into_iter().filter(move |&mix| self.has_mix(mix))
    }
    pub fn mix_count(&self) -> usize { self.mixes().count() }
    pub fn has_aux(&self) -> bool { self.has_aux }
    pub fn app_count(&self) -> usize { self.app_count }
    pub fn sources(&self) -> impl Iterator<Item = RouteSource> {
        [RouteSource::Mic].into_iter().chain(self.has_aux.then_some(RouteSource::Aux))
            .chain((0..self.app_count).map(RouteSource::App))
    }
    pub fn stop_requested(&self) -> bool { self.stop_requested.load(Ordering::Relaxed) }

    pub fn vocal_parameters(&self) -> impl Iterator<Item = VocalParameter> + '_ {
        VocalParameter::ALL.into_iter().filter(move |parameter| parameter.available_for(self.vocal_start.preset))
    }

    /// Read one requested value, not a transient DSP ramp.
    pub fn vocal_value(&self, parameter: VocalParameter) -> f32 {
        f32::from_bits(self.vocal_targets[parameter as usize].load(Ordering::Relaxed))
    }

    /// Control-thread snapshot of the targets and the fixed startup topology.
    pub fn vocal_config(&self) -> VocalConfig {
        let mut config = self.vocal_start;
        for parameter in VocalParameter::ALL {
            config.set_parameter(parameter, self.vocal_value(parameter)).expect("validated atomic vocal target");
        }
        config
    }

    /// Requested mute for this stage; global and local mutes remain independent.
    pub fn is_muted(&self, target: MuteTarget) -> bool {
        target.bit().is_some_and(|bit| self.muted.load(Ordering::Relaxed) & bit != 0)
    }

    /// Requested attenuation, before combining the master and send stages.
    pub fn gain_db(&self, target: GainTarget) -> Result<f32, String> {
        Ok(20.0 * f32::from_bits(self.gain_atomic(target)?.load(Ordering::Relaxed)).log10())
    }

    fn gain_atomic(&self, target: GainTarget) -> Result<&AtomicU32, String> {
        let mix = match target {
            GainTarget::Master => return Ok(&self.master),
            GainTarget::Monitor => RouteMix::Output,
            GainTarget::Send => RouteMix::Send,
            GainTarget::Broadcast => RouteMix::Broadcast,
        };
        if !self.has_mix(mix) { return Err(format!("this route has no {} mix", mix.name())); }
        Ok(&self.mix_gains[mix.index()])
    }

    /// Called once before sharing the prepared controls with processing or UI threads.
    #[cfg(any(windows, test))]
    pub(crate) fn prepare_effects(&mut self, bypasses: impl Iterator<Item = bool>) {
        *self.effect_bypasses.get_mut().expect("control bypass list") = bypasses.map(|bypassed| Arc::new(AtomicBool::new(bypassed))).collect();
    }

    /// Publish only after the processing worker has acknowledged the new order.
    #[cfg(any(windows, test))]
    pub(crate) fn replace_effects(&self, flags: Vec<Arc<AtomicBool>>) {
        *self.effect_bypasses.lock().expect("control bypass list") = flags;
    }

    pub(crate) fn effect_flag(&self, index: usize) -> Result<Arc<AtomicBool>, String> {
        self.effect_bypasses.lock().expect("control bypass list").get(index).cloned()
            .ok_or_else(|| "VST3 slot is outside the active chain".into())
    }

    pub fn effect_bypassed(&self, index: usize) -> Result<bool, String> {
        Ok(self.effect_flag(index)?.load(Ordering::Relaxed))
    }

    /// Supplemental input attenuation, independent of mute and all destination sends.
    pub fn source_trim_db(&self, source: RouteSource) -> Result<f32, String> {
        let index = self.trim_index(source)?;
        Ok((20.0 * f32::from_bits(self.source_trims[index].load(Ordering::Relaxed)).log10()).clamp(-96.0, 0.0))
    }

    fn trim_index(&self, source: RouteSource) -> Result<usize, String> {
        match source {
            RouteSource::Mic => Err("use vocal trim for the microphone".into()),
            RouteSource::Aux if self.has_aux => Ok(source.index()),
            RouteSource::App(index) if index < self.app_count => Ok(source.index()),
            RouteSource::Aux => Err("this route has no AUX source".into()),
            RouteSource::App(_) => Err("application slot is outside the active scene".into()),
        }
    }

    pub(crate) fn trim_targets(&self) -> [f32; MAX_SOURCES] {
        std::array::from_fn(|index| f32::from_bits(self.source_trims[index].load(Ordering::Relaxed)))
    }

    /// Requested connection and attenuation. A disabled route retains its level.
    pub fn route_state(&self, source: RouteSource, mix: RouteMix) -> Result<(bool, f32), String> {
        let index = self.route_index(source, mix)?;
        let enabled = self.routes_enabled.load(Ordering::Relaxed) & (1 << index) != 0;
        let db = 20.0 * f32::from_bits(self.route_levels[index].load(Ordering::Relaxed)).log10();
        Ok((enabled, db))
    }

    /// Control/UI thread only. Invalid commands leave every target unchanged.
    /// Stop requests teardown on the owning control thread, never inside a device callback.
    pub fn apply(&self, command: LiveCommand) -> Result<(), String> {
        match command {
            LiveCommand::Mute { target, muted } => {
                if target == MuteTarget::Aux && !self.has_aux { return Err("this route has no AUX source".into()); }
                for mix in RouteMix::ALL {
                    if target == mix.mute_target() && !self.has_mix(mix) {
                        return Err(format!("this route has no {} mix", mix.name()));
                    }
                }
                if matches!(target, MuteTarget::App(index) if index >= self.app_count) {
                    return Err("application slot is outside the active scene".into());
                }
                let bit = target.bit().ok_or("invalid mute target")?;
                if muted { self.muted.fetch_or(bit, Ordering::Relaxed); }
                else { self.muted.fetch_and(!bit, Ordering::Relaxed); }
            }
            LiveCommand::Gain { target, db } => {
                validate_db(db)?;
                let level = self.gain_atomic(target)?;
                level.store(gain(db).to_bits(), Ordering::Relaxed);
            }
            LiveCommand::Route { source, mix, enabled } => {
                let index = self.route_index(source, mix)?;
                if enabled { self.routes_enabled.fetch_or(1 << index, Ordering::Relaxed); }
                else { self.routes_enabled.fetch_and(!(1 << index), Ordering::Relaxed); }
            }
            LiveCommand::RouteGain { source, mix, db } => {
                let index = self.route_index(source, mix)?;
                validate_db(db)?;
                self.route_levels[index].store(gain(db).to_bits(), Ordering::Relaxed);
            }
            LiveCommand::SourceTrim { source, db } => {
                validate_db(db)?;
                let index = self.trim_index(source)?;
                self.source_trims[index].store(gain(db).to_bits(), Ordering::Relaxed);
            }
            LiveCommand::EffectBypass { index, bypassed } => {
                self.effect_flag(index)?.store(bypassed, Ordering::Relaxed);
            }
            LiveCommand::Vocal { parameter, value } => {
                if !parameter.available_for(self.vocal_start.preset) {
                    return Err("this parameter requires a non-off vocal preset; stop and select a preset first".into());
                }
                parameter.validate(value).map_err(|error| error.to_string())?;
                self.vocal_targets[parameter as usize].store(value.to_bits(), Ordering::Relaxed);
            }
            LiveCommand::Stop => self.stop_requested.store(true, Ordering::Relaxed),
            LiveCommand::Status => {}
        }
        Ok(())
    }

    fn route_index(&self, source: RouteSource, mix: RouteMix) -> Result<usize, String> {
        if source == RouteSource::Aux && !self.has_aux { return Err("this route has no AUX source".into()); }
        if matches!(source, RouteSource::App(index) if index >= self.app_count) { return Err("application slot is outside the active scene".into()); }
        if !self.has_mix(mix) { return Err(format!("this route has no {} mix", mix.name())); }
        Ok(source.index() * MAX_MIXES + mix.index())
    }

    /// Per-send targets only. Source mute and final output mute remain separate stages.
    pub(crate) fn route_targets(&self) -> [f32; MAX_ROUTES] {
        let enabled = self.routes_enabled.load(Ordering::Relaxed);
        std::array::from_fn(|index| if enabled & (1 << index) != 0 {
            f32::from_bits(self.route_levels[index].load(Ordering::Relaxed))
        } else { 0.0 })
    }

    pub(crate) fn source_targets(&self) -> [f32; MAX_SOURCES] {
        let muted = self.muted.load(Ordering::Relaxed);
        std::array::from_fn(|slot| {
            let bit = match slot { 0 => MIC, 1 => AUX, index => 1 << (index + 3) };
            if muted & bit == 0 { 1.0 } else { 0.0 }
        })
    }
    pub(crate) fn output_targets(&self) -> [f32; MAX_MIXES] {
        let muted = self.muted.load(Ordering::Relaxed);
        let master = f32::from_bits(self.master.load(Ordering::Relaxed));
        std::array::from_fn(|index| {
            if self.enabled_mixes[index] && muted & (ALL | [OUTPUT, SEND, BROADCAST][index]) == 0 {
                master * f32::from_bits(self.mix_gains[index].load(Ordering::Relaxed))
            } else { 0.0 }
        })
    }

    /// Requested state, not proof that another app or listener has received silence.
    pub fn describe(&self) -> String {
        let muted = self.muted.load(Ordering::Relaxed);
        let mut text = format!("master_gain={:.1}dB mute[mic={},aux={},all={}]",
            20.0 * f32::from_bits(self.master.load(Ordering::Relaxed)).log10(),
            muted & MIC != 0, muted & AUX != 0, muted & ALL != 0);
        for mix in self.mixes() {
            let gain = self.gain_db(mix.gain_target()).expect("configured mix");
            text.push_str(&format!(" {}_extra_gain={gain:.1}dB mute[{}={}]", mix.name(), mix.name(), self.is_muted(mix.mute_target())));
        }
        for parameter in self.vocal_parameters() {
            text.push_str(&format!(" vocal.{}={:.3}", parameter.name(), self.vocal_value(parameter)));
        }
        for (index, flag) in self.effect_bypasses.lock().expect("control bypass list").iter().enumerate() {
            text.push_str(&format!(" vst3.{}.bypass={}", index + 1, flag.load(Ordering::Relaxed)));
        }
        let enabled = self.routes_enabled.load(Ordering::Relaxed);
        for source in self.sources() {
            let name = source.name();
            if source != RouteSource::Mic {
                let db = self.source_trim_db(source).expect("configured source");
                text.push_str(&format!(" {name}_trim={db:.1}dB"));
            }
            if matches!(source, RouteSource::App(_)) {
                text.push_str(&format!(" mute[{name}={}]", self.is_muted(source.mute_target())));
            }
            for mix in self.mixes() {
                let destination = mix.name();
                let index = source.index() * MAX_MIXES + mix.index();
                let state = if enabled & (1 << index) != 0 { "on" } else { "off" };
                let db = 20.0 * f32::from_bits(self.route_levels[index].load(Ordering::Relaxed)).log10();
                text.push_str(&format!(" {name}->{destination}={state}@{db:.1}dB"));
            }
        }
        text
    }
}

/// Five-millisecond linear gain ramp at the consuming stream's rate. No delay buffer.
/// Repeating the same target must NOT restart the ramp on each callback.
pub(crate) struct GainRamp { value: f32, target: f32, step: f32, left: u32, frames: u32 }
impl GainRamp {
    pub fn new(rate: u32, value: f32) -> Self {
        Self { value, target: value, step: 0.0, left: 0, frames: (rate / 200).max(1) }
    }
    pub fn set(&mut self, target: f32) {
        if target != self.target {
            self.target = target;
            self.left = self.frames;
            self.step = (target - self.value) / self.frames as f32;
        }
    }
    pub fn next_gain(&mut self) -> f32 {
        if self.left != 0 {
            self.left -= 1;
            if self.left == 0 { self.value = self.target; } else { self.value += self.step; }
        }
        self.value
    }
}

/// Route-order gains for one physical output stream, applied AFTER its clock queue/resampler.
/// A shared endpoint ramps its lanes on its one clock; distinct devices have their own ramps.
pub(crate) struct PlaybackGains {
    controls: Arc<LiveControls>,
    mixes: [usize; MAX_MIXES],
    count: usize,
    ramps: [GainRamp; MAX_MIXES],
}
impl PlaybackGains {
    pub fn new(rate: u32, mixes: &[usize], controls: Arc<LiveControls>) -> Result<Self, String> {
        if !(8_000..=192_000).contains(&rate) || !(1..=MAX_MIXES).contains(&mixes.len())
            || mixes.iter().enumerate().any(|(index, &mix)| !controls.enabled_mixes.get(mix).copied().unwrap_or(false)
                || mixes[..index].contains(&mix)) {
            return Err("invalid playback gain bindings".into());
        }
        let mut lanes = [0; MAX_MIXES];
        lanes[..mixes.len()].copy_from_slice(mixes);
        let targets = controls.output_targets();
        let ramps = std::array::from_fn(|lane| GainRamp::new(rate,
            if lane < mixes.len() { targets[lanes[lane]] } else { 0.0 }));
        Ok(Self { controls, mixes: lanes, count: mixes.len(), ramps })
    }
    pub fn update(&mut self) {
        let targets = self.controls.output_targets();
        for lane in 0..self.count { self.ramps[lane].set(targets[self.mixes[lane]]); }
    }
    pub fn next_gains(&mut self) -> [f32; MAX_MIXES] { std::array::from_fn(|lane| self.ramps[lane].next_gain()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typed_reads_preserve_independent_requested_controls_and_reject_absent_routes() {
        let controls = LiveControls::new(-18.0, [Some(0.0), Some(-6.0), None], true, 0, VocalConfig::default()).unwrap();
        assert!((controls.gain_db(GainTarget::Master).unwrap() + 18.0).abs() < 1e-5);
        assert!((controls.gain_db(GainTarget::Send).unwrap() + 6.0).abs() < 1e-5);
        assert_eq!(controls.route_state(RouteSource::Mic, RouteMix::Output).unwrap(), (true, 0.0));
        assert_eq!(controls.route_state(RouteSource::Aux, RouteMix::Send).unwrap(), (false, 0.0));

        for command in ["mute all", "mute mic", "gain master -12.5", "gain send -9",
            "level aux send -15", "route aux send on", "route aux send off"] {
            controls.apply(command.parse().unwrap()).unwrap();
        }
        assert!(controls.is_muted(MuteTarget::All));
        assert!(controls.is_muted(MuteTarget::Mic));
        assert!(!controls.is_muted(MuteTarget::Output));
        assert!(!controls.is_muted(MuteTarget::Send));
        assert!((controls.gain_db(GainTarget::Master).unwrap() + 12.5).abs() < 1e-5);
        assert!((controls.gain_db(GainTarget::Send).unwrap() + 9.0).abs() < 1e-5);
        let (enabled, db) = controls.route_state(RouteSource::Aux, RouteMix::Send).unwrap();
        assert!(!enabled);
        assert!((db + 15.0).abs() < 1e-5);
        controls.apply("unmute all".parse().unwrap()).unwrap();
        assert!(!controls.is_muted(MuteTarget::All));
        assert!(controls.is_muted(MuteTarget::Mic));

        let single = LiveControls::new(0.0, [Some(0.0), None, None], false, 0, VocalConfig::default()).unwrap();
        assert_eq!(single.gain_db(GainTarget::Master).unwrap(), 0.0);
        assert!(single.gain_db(GainTarget::Send).is_err());
        assert!(single.route_state(RouteSource::Aux, RouteMix::Output).is_err());
        assert!(single.route_state(RouteSource::Mic, RouteMix::Send).is_err());
    }

    #[test]
    fn commands_validate_before_mutating_and_master_unmute_preserves_local_mutes() {
        let controls = LiveControls::new(-18.0, [Some(0.0), Some(-6.0), None], true, 0, VocalConfig::default()).unwrap();
        for invalid in ["", "mute", "mute mic now", "stop now", "gain output -6", "gain master NaN", "gain send inf", "gain master 1", "gain master -97"] {
            assert!(invalid.parse::<LiveCommand>().is_err(), "{invalid}");
        }
        let initial = controls.output_targets();
        controls.apply("mute send".parse().unwrap()).unwrap();
        controls.apply("mute mic".parse().unwrap()).unwrap();
        controls.apply("mute all".parse().unwrap()).unwrap();
        assert_eq!(controls.output_targets(), [0.0; 3]);
        controls.apply("unmute all".parse().unwrap()).unwrap();
        assert_eq!(controls.output_targets(), [initial[0], 0.0, 0.0]);
        assert_eq!(controls.source_targets()[..2], [0.0, 1.0]);
        controls.apply("gain send -12".parse().unwrap()).unwrap();
        assert_eq!(controls.output_targets()[1], 0.0); // changing a fader never unmutes it
        controls.apply("unmute send".parse().unwrap()).unwrap();
        assert!((controls.output_targets()[1] - gain(-30.0)).abs() < 1e-7);
        let single = LiveControls::new(-18.0, [Some(0.0), None, None], false, 0, VocalConfig::default()).unwrap();
        for command in ["mute aux", "mute send", "gain send -12"] {
            assert!(single.apply(command.parse().unwrap()).is_err());
        }
        let before = single.output_targets();
        assert!(single.apply(LiveCommand::Gain { target: GainTarget::Master, db: f32::NAN }).is_err());
        assert_eq!(single.output_targets(), before);
    }

    #[test]
    fn app_controls_are_independent_and_roundtrip_requested_settings() {
        use crate::config::{Cli, Command, AppSource};
        use clap::Parser;
        let Command::Check(mut route) = Cli::parse_from(["audio", "check", "--input", "mic", "--output", "phones",
            "--send", "cable", "--app", "browser.exe", "--app", "call.exe"]).command else { panic!("expected check") };
        let controls = LiveControls::from_route(&route).unwrap();
        assert_eq!(controls.route_state(RouteSource::App(0), RouteMix::Send).unwrap(), (false, 0.0));
        for command in ["route app1 send on", "level app1 send -7", "route app2 output on", "mute app2",
            "level app2 output -11", "mute all", "unmute all"] {
            controls.apply(command.parse().unwrap()).unwrap();
        }
        controls.snapshot_into(&mut route).unwrap();
        assert_eq!(route.apps[0].to, AuxDestination::Send);
        assert_eq!(route.apps[1].to, AuxDestination::Output);
        assert!(route.apps[1].muted && !route.apps[0].muted);
        let restored = LiveControls::from_route(&route).unwrap();
        assert_eq!(restored.describe(), controls.describe());
        assert!(restored.apply("route app3 send on".parse().unwrap()).is_err());
        assert!(restored.apply("mute app3".parse().unwrap()).is_err());
        assert!("mute app0".parse::<LiveCommand>().is_err());
        route.apps.push(AppSource::new("browser.exe".into()));
        assert!(route.validate().is_err());
    }

    #[test]
    fn bypass_commands_target_only_explicit_active_vst3_slots() {
        let mut controls = LiveControls::new(0.0, [Some(0.0), None, None], false, 0, VocalConfig::default()).unwrap();
        assert!(controls.apply("bypass 1 on".parse().unwrap()).is_err());
        controls.prepare_effects([false, true].into_iter());
        let first = controls.effect_flag(0).unwrap();
        assert!(!first.load(Ordering::Relaxed));
        assert!(controls.effect_bypassed(1).unwrap());
        controls.apply("bypass 1 on".parse().unwrap()).unwrap();
        controls.apply("bypass 2 off".parse().unwrap()).unwrap();
        assert!(first.load(Ordering::Relaxed));
        assert!(!controls.effect_bypassed(1).unwrap());
        assert!(controls.apply("bypass 3 on".parse().unwrap()).is_err());
        for invalid in ["bypass 0 on", "bypass -1 on", "bypass 1 true", "bypass 1 on now"] {
            assert!(invalid.parse::<LiveCommand>().is_err());
        }
        assert!(controls.describe().contains("vst3.1.bypass=true vst3.2.bypass=false"));
        let second = controls.effect_flag(1).unwrap();
        controls.replace_effects(vec![second.clone(), first.clone()]);
        assert!(Arc::ptr_eq(&controls.effect_flag(0).unwrap(), &second));
        controls.apply("bypass 2 off".parse().unwrap()).unwrap();
        assert!(!first.load(Ordering::Relaxed));
        controls.replace_effects(vec![]);
        assert!(controls.effect_bypassed(0).is_err());
        assert!(!controls.describe().contains("vst3."));
    }

    #[test]
    fn supplemental_trim_is_independent_validated_and_saved() {
        use crate::config::{Cli, Command};
        use clap::Parser;
        let Command::Check(mut route) = Cli::parse_from(["audio", "check", "--input", "mic", "--output", "phones",
            "--send", "cable", "--aux-input", "mr", "--aux-gain-db", "-9", "--app", "browser.exe", "--app", "call.exe"]).command
            else { panic!("expected check") };
        let controls = LiveControls::from_route(&route).unwrap();
        assert!((controls.source_trim_db(RouteSource::Aux).unwrap() + 9.0).abs() < 1e-4);
        for command in ["trim aux -6", "trim app1 -24", "trim app2 -96", "mute app1"] {
            controls.apply(command.parse().unwrap()).unwrap();
        }
        let before = controls.trim_targets();
        for command in ["trim mic -6", "trim app0 -6", "trim app9 -6", "trim aux NaN", "trim app1 1", "trim app2 -97"] {
            assert!(command.parse::<LiveCommand>().is_err(), "{command}");
        }
        assert!(controls.apply("trim app3 -9".parse().unwrap()).is_err());
        assert!(controls.apply(LiveCommand::SourceTrim { source: RouteSource::Mic, db: -3.0 }).is_err());
        assert!(controls.apply(LiveCommand::SourceTrim { source: RouteSource::App(0), db: f32::NAN }).is_err());
        assert_eq!(controls.trim_targets(), before);
        let absent = LiveControls::new(0.0, [Some(0.0), None, None], false, 0, VocalConfig::default()).unwrap();
        assert!(absent.apply("trim aux -6".parse().unwrap()).is_err());
        assert!(absent.source_trim_db(RouteSource::Aux).is_err());
        assert_eq!(controls.route_state(RouteSource::App(0), RouteMix::Send).unwrap(), (false, 0.0));
        controls.snapshot_into(&mut route).unwrap();
        assert!((route.aux_gain_db + 6.0).abs() < 1e-4);
        assert!((route.apps[0].gain_db + 24.0).abs() < 1e-4);
        assert!((route.apps[1].gain_db + 96.0).abs() < 1e-4);
        assert!(route.apps[0].muted);
        let restored = LiveControls::from_route(&route).unwrap();
        assert_eq!(restored.describe(), controls.describe());
        assert_eq!(restored.vocal_config(), controls.vocal_config());
    }

    #[test]
    fn live_vocal_parameters_are_validated_and_preserved_in_scene_snapshot() {
        use crate::config::{Cli, Command};
        use clap::Parser;
        use mixlive_effects::Preset;
        let Command::Check(mut route) = Cli::parse_from(["audio", "check", "--input", "mic", "--output", "phones",
            "--vocal", "warm", "--vocal-strength", "0", "--reverb-predelay-ms", "25"]).command else { panic!("expected check") };
        let controls = LiveControls::from_route(&route).unwrap();
        for command in ["vocal trim -3", "vocal strength 0.75", "vocal compressor -35",
            "vocal deesser -40", "vocal reverb 0.25", "vocal decay 2.5", "vocal echo 0.375", "vocal echo-feedback 0.5", "vocal ratio 5", "vocal attack 2", "vocal release 250", "vocal deess-max 8",
            "vocal eq-hpf 120", "vocal eq-body -3", "vocal eq-presence 2.5", "vocal eq-air 4"] {
            controls.apply(command.parse().unwrap()).unwrap();
        }
        let before = controls.vocal_config();
        for command in ["vocal reverb NaN", "vocal decay 5.1", "vocal trim -37",
            "vocal compressor 1", "vocal strength 1.01", "vocal predelay 10", "vocal preset warm",
            "vocal echo NaN", "vocal echo-feedback 0.86", "vocal echo-time 100", "vocal ratio 9", "vocal attack 0", "vocal release 501", "vocal deess-max NaN",
            "vocal eq-hpf 39", "vocal eq-hpf 201", "vocal eq-body -6.1", "vocal eq-presence 6.1", "vocal eq-air NaN"] {
            assert!(command.parse::<LiveCommand>().is_err(), "{command}");
        }
        assert!(controls.apply(LiveCommand::Vocal { parameter: VocalParameter::Trim, value: f32::NAN }).is_err());
        assert_eq!(controls.vocal_config(), before);
        controls.snapshot_into(&mut route).unwrap();
        assert_eq!(route.vocal_config(), before);
        assert_eq!(route.vocal, Preset::Warm);
        assert_eq!(route.reverb_predelay_ms, 25.0);
        assert_eq!(route.vocal_config().eq, mixlive_effects::EqConfig {
            hpf_hz: 120.0, body_db: -3.0, presence_db: 2.5, air_db: 4.0,
        });
        // A later snapshot can update EQ again; it no longer changes the topology.
        controls.apply("vocal eq-hpf 80".parse().unwrap()).unwrap();
        controls.snapshot_into(&mut route).unwrap();
        let scene = crate::scene::AudioScene::new(route.clone(), Vec::new()).unwrap();
        let json = serde_json::to_string(&scene).unwrap();
        let saved: crate::scene::AudioScene = serde_json::from_str(&json).unwrap();
        assert_eq!(saved.route.vocal_config(), controls.vocal_config());
        let restored = LiveControls::from_route(&route).unwrap();
        assert_eq!(restored.describe(), controls.describe());
        let off = LiveControls::new(0.0, [Some(0.0), None, None], false, 0, VocalConfig::default()).unwrap();
        assert!(off.apply("vocal strength 1".parse().unwrap()).is_err());
        assert!(off.apply("vocal ratio 4".parse().unwrap()).is_err());
        assert!(off.apply("vocal eq-hpf 95".parse().unwrap()).is_err());
        assert!(off.apply("vocal eq-body 3".parse().unwrap()).is_err());
        assert!(off.apply("vocal reverb 0.5".parse().unwrap()).is_ok());
        assert!(off.apply("vocal echo 0.5".parse().unwrap()).is_ok());
        assert_eq!(off.vocal_parameters().count(), 5);
    }

    #[test]
    fn broadcast_routing_mutes_and_gains_roundtrip_without_a_call_mix() {
        use crate::config::{Cli, Command};
        use clap::Parser;
        let Command::Check(mut route) = Cli::parse_from(["audio", "check", "--input", "mic", "--output", "phones",
            "--broadcast", "cable", "--app", "browser.exe"]).command else { panic!("expected check") };
        let controls = LiveControls::from_route(&route).unwrap();
        assert_eq!(controls.mixes().collect::<Vec<_>>(), [RouteMix::Output, RouteMix::Broadcast]);
        for command in ["route app1 broadcast on", "level app1 broadcast -7", "route mic broadcast off",
            "gain monitor -4", "gain broadcast -8", "mute broadcast"] {
            controls.apply(command.parse().unwrap()).unwrap();
        }
        assert!(controls.apply("gain send -3".parse().unwrap()).is_err());
        assert!(controls.apply("route app1 send on".parse().unwrap()).is_err());
        assert!(controls.output_targets()[0] > 0.0);
        assert_eq!(controls.output_targets()[1..], [0.0, 0.0]);
        controls.snapshot_into(&mut route).unwrap();
        assert_eq!(route.apps[0].to, AuxDestination::Broadcast);
        assert!(!route.mix.mic_broadcast_enabled && route.mix.mute_broadcast);
        assert_eq!(LiveControls::from_route(&route).unwrap().describe(), controls.describe());
    }

    #[test]
    fn ramps_are_continuous_block_invariant_and_reach_exact_silence() {
        for rate in [8_000, 44_100, 48_000, 192_000] {
            let frames = (rate / 200) as usize;
            let mut ramp = GainRamp::new(rate, 1.0);
            ramp.set(0.0);
            let mut previous = 1.0;
            for index in 0..frames {
                ramp.set(0.0); // a callback boundary cannot restart this fade
                let value = ramp.next_gain();
                assert!((0.0..=previous).contains(&value));
                assert!((previous - value) < 1.0 / frames as f32 + 1e-5);
                if index + 1 < frames { assert!(value > 0.0); }
                previous = value;
            }
            assert_eq!(ramp.next_gain(), 0.0);
            ramp.set(0.5);
            for _ in 0..frames / 2 { ramp.next_gain(); }
            let middle = ramp.value;
            ramp.set(0.1);
            assert!((ramp.next_gain() - middle).abs() < 1.0 / frames as f32);
            for _ in 1..frames { ramp.next_gain(); }
            assert_eq!(ramp.next_gain(), 0.1);
        }
    }

    #[test]
    fn output_mutes_act_on_already_queued_audio_and_keep_other_mix_running() {
        use crate::bridge::{channel, Health, QUANTUM};
        let controls = Arc::new(LiveControls::new(-6.0, [Some(0.0), Some(-12.0), None], false, 0, VocalConfig::default()).unwrap());
        let health = Arc::new(Health::default());
        let (mut writer, mut reader) = channel(48_000, 48_000, 30, 2, health.clone()).unwrap();
        let mut faders = PlaybackGains::new(48_000, &[0, 1], controls.clone()).unwrap();
        let priming = (reader.queue_ms * 48.0).round() as usize + 1024;
        for _ in 0..priming { writer.push([0.4, -0.2, 0.4, -0.2, 0.0, 0.0]); }
        for _ in 0..4 * QUANTUM { reader.next_frame(faders.next_gains()); }
        let before = reader.next_frame(faders.next_gains());
        assert!((before[0] - 0.4 * gain(-6.0)).abs() < 1e-4);
        assert!((before[2] - 0.4 * gain(-18.0)).abs() < 1e-4);
        controls.apply("mute output".parse().unwrap()).unwrap();
        faders.update();
        let mut after = [0.0; 6];
        for _ in 0..240 { after = reader.next_frame(faders.next_gains()); }
        assert_eq!(&after[..2], &[0.0; 2]);
        assert!((after[2] - before[2]).abs() < 1e-4);
        let _ = reader.stats.take_peaks(); // discard the preceding fade interval
        reader.next_frame(faders.next_gains());
        let peaks = reader.stats.take_peaks();
        assert_eq!(peaks[0], 0.0);
        assert!((peaks[1] - after[2].abs()).abs() < 1e-4);
        controls.apply("gain send -6".parse().unwrap()).unwrap();
        faders.update();
        for _ in 0..240 { after = reader.next_frame(faders.next_gains()); }
        assert!((after[2] - 0.4 * gain(-12.0)).abs() < 1e-4);
        controls.apply("mute all".parse().unwrap()).unwrap();
        faders.update();
        for _ in 0..240 { after = reader.next_frame(faders.next_gains()); }
        assert_eq!(after, [0.0; 6]);
        // Peak is measured AFTER mute/attenuation; stale queued samples are still consumed.
        let _ = reader.stats.take_peaks();
        assert_eq!(reader.next_frame(faders.next_gains()), [0.0; 6]);
        assert_eq!(reader.stats.take_peaks(), [0.0; 3]);
        assert!(!health.failed());
        let mut send_only = PlaybackGains::new(44_100, &[1], controls.clone()).unwrap();
        controls.apply("unmute all".parse().unwrap()).unwrap();
        send_only.update();
        for _ in 0..220 { send_only.next_gains(); }
        assert!((send_only.next_gains()[0] - gain(-12.0)).abs() < 1e-7);
        assert_eq!(send_only.next_gains()[1], 0.0);
    }

    #[test]
    fn route_switches_and_levels_are_independent_of_mutes_and_each_other() {
        let controls = LiveControls::new(-18.0, [Some(0.0), Some(-6.0), None], true, 0, VocalConfig::default()).unwrap();
        assert_eq!(controls.route_targets()[..6], [1.0, 1.0, 0.0, 0.0, 0.0, 0.0]);
        for invalid in ["route mic", "route mic output", "route mic output yes", "route mic output off extra",
            "route none send on", "route aux all on", "level aux send", "level mic output 1",
            "level mic output -97", "level aux send NaN", "level aux send inf"] {
            assert!(invalid.parse::<LiveCommand>().is_err(), "{invalid}");
        }
        controls.apply("route mic output off".parse().unwrap()).unwrap();
        controls.apply("level mic output -6".parse().unwrap()).unwrap();
        assert_eq!(controls.route_targets()[..6], [0.0, 1.0, 0.0, 0.0, 0.0, 0.0]);
        controls.apply("route aux send on".parse().unwrap()).unwrap();
        controls.apply("level aux send -12".parse().unwrap()).unwrap();
        let expected = [0.0, 1.0, 0.0, 0.0, gain(-12.0), 0.0];
        assert_eq!(controls.route_targets()[..6], expected);
        for command in ["mute all", "mute mic", "mute send", "unmute all", "gain master -3"] {
            controls.apply(command.parse().unwrap()).unwrap();
            assert_eq!(controls.route_targets()[..6], expected);
        }
        controls.apply("route mic output on".parse().unwrap()).unwrap();
        assert_eq!(controls.source_targets()[0], 0.0); // routing cannot unmute the source
        assert_eq!(controls.output_targets()[1], 0.0); // nor the whole send mix
        assert_eq!(controls.route_targets()[0], gain(-6.0));
        assert!(controls.describe().contains("aux->send=on@-12.0dB"));
        let before = controls.describe();
        assert!(controls.apply(LiveCommand::RouteGain { source: RouteSource::Mic, mix: RouteMix::Output, db: f32::NAN }).is_err());
        assert_eq!(controls.describe(), before);
        let single = LiveControls::new(0.0, [Some(0.0), None, None], false, 0, VocalConfig::default()).unwrap();
        let before = single.describe();
        for command in ["route aux output on", "level aux output -6", "route mic send on", "level mic send -3"] {
            assert!(single.apply(command.parse().unwrap()).is_err());
            assert_eq!(single.describe(), before);
        }
    }

}
