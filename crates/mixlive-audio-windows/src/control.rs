//! Live session controls. Parsing/printing stays on the control thread; callbacks only load atomics.
use std::{str::FromStr, sync::{Arc, atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering}}};

const MIC: u8 = 1;
const AUX: u8 = 2;
const OUTPUT: u8 = 4;
const SEND: u8 = 8;
const ALL: u8 = 16;

pub const HELP: &str = "Live commands: mute/unmute mic|aux|output|send|all; gain master|send <dB -96..0>; route mic|aux output|send on|off; level mic|aux output|send <dB -96..0>; status; stop. Changes last for this run only.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MuteTarget { Mic, Aux, Output, Send, All }
impl MuteTarget {
    fn bit(self) -> u8 {
        match self { Self::Mic => MIC, Self::Aux => AUX, Self::Output => OUTPUT, Self::Send => SEND, Self::All => ALL }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GainTarget { Master, Send }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteSource { Mic, Aux }
impl RouteSource {
    fn index(self) -> usize { match self { Self::Mic => 0, Self::Aux => 1 } }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteMix { Output, Send }
impl RouteMix {
    fn index(self) -> usize { match self { Self::Output => 0, Self::Send => 1 } }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LiveCommand {
    Mute { target: MuteTarget, muted: bool },
    Gain { target: GainTarget, db: f32 },
    Route { source: RouteSource, mix: RouteMix, enabled: bool },
    RouteGain { source: RouteSource, mix: RouteMix, db: f32 },
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
                    Some("output") => MuteTarget::Output, Some("send") => MuteTarget::Send,
                    Some("all") => MuteTarget::All, _ => return Err(HELP.into()),
                };
                Self::Mute { target, muted: verb == "mute" }
            }
            Some("gain") => {
                let target = match words.next() {
                    Some("master") => GainTarget::Master, Some("send") => GainTarget::Send,
                    _ => return Err(HELP.into()),
                };
                let db: f32 = words.next().ok_or(HELP)?.parse().map_err(|_| HELP)?;
                validate_db(db)?;
                Self::Gain { target, db }
            }
            Some(verb @ ("route" | "level")) => {
                let source = match words.next() {
                    Some("mic") => RouteSource::Mic, Some("aux") => RouteSource::Aux,
                    _ => return Err(HELP.into()),
                };
                let mix = match words.next() {
                    Some("output") => RouteMix::Output, Some("send") => RouteMix::Send,
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

/// One session, initialized from its route. Neither commands nor runtime mutes rewrite a Scene.
/// Individual controls are independent atomic values, not an atomic multi-parameter Scene swap.
pub struct LiveControls {
    muted: AtomicU8,
    stop_requested: AtomicBool,
    master: AtomicU32,
    send: AtomicU32,
    has_send: bool,
    has_aux: bool,
    // Fixed order: mic->output, mic->send, AUX->output, AUX->send.
    // Enabled state is independent of fader level and global source/output mutes.
    routes_enabled: AtomicU8,
    route_levels: [AtomicU32; 4],
}
impl LiveControls {
    pub fn new(master_db: f32, send_db: Option<f32>, has_aux: bool) -> Result<Self, String> {
        validate_db(master_db)?;
        validate_db(send_db.unwrap_or(0.0))?;
        Ok(Self { muted: AtomicU8::new(0), stop_requested: AtomicBool::new(false), master: AtomicU32::new(gain(master_db).to_bits()),
            send: AtomicU32::new(gain(send_db.unwrap_or(0.0)).to_bits()), has_send: send_db.is_some(), has_aux,
            // Capture preparation sets the saved AUX destination before any stream starts.
            routes_enabled: AtomicU8::new(if send_db.is_some() { 3 } else { 1 }),
            route_levels: std::array::from_fn(|_| AtomicU32::new(1.0_f32.to_bits())) })
    }
    pub fn mix_count(&self) -> usize { if self.has_send { 2 } else { 1 } }
    pub fn has_aux(&self) -> bool { self.has_aux }
    pub fn stop_requested(&self) -> bool { self.stop_requested.load(Ordering::Relaxed) }

    /// Control/UI thread only. Invalid commands leave every target unchanged.
    /// Stop requests teardown on the owning control thread, never inside a device callback.
    pub fn apply(&self, command: LiveCommand) -> Result<(), String> {
        match command {
            LiveCommand::Mute { target, muted } => {
                if target == MuteTarget::Aux && !self.has_aux { return Err("this route has no AUX source".into()); }
                if target == MuteTarget::Send && !self.has_send { return Err("this route has no send mix".into()); }
                if muted { self.muted.fetch_or(target.bit(), Ordering::Relaxed); }
                else { self.muted.fetch_and(!target.bit(), Ordering::Relaxed); }
            }
            LiveCommand::Gain { target, db } => {
                validate_db(db)?;
                let level = match target {
                    GainTarget::Master => &self.master,
                    GainTarget::Send if self.has_send => &self.send,
                    GainTarget::Send => return Err("this route has no send mix".into()),
                };
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
            LiveCommand::Stop => self.stop_requested.store(true, Ordering::Relaxed),
            LiveCommand::Status => {}
        }
        Ok(())
    }

    fn route_index(&self, source: RouteSource, mix: RouteMix) -> Result<usize, String> {
        if source == RouteSource::Aux && !self.has_aux { return Err("this route has no AUX source".into()); }
        if mix == RouteMix::Send && !self.has_send { return Err("this route has no send mix".into()); }
        Ok(source.index() * 2 + mix.index())
    }

    /// Per-send targets only. Source mute and final output mute remain separate stages.
    pub(crate) fn route_targets(&self) -> [f32; 4] {
        let enabled = self.routes_enabled.load(Ordering::Relaxed);
        std::array::from_fn(|index| if enabled & (1 << index) != 0 {
            f32::from_bits(self.route_levels[index].load(Ordering::Relaxed))
        } else { 0.0 })
    }

    pub(crate) fn source_targets(&self) -> [f32; 2] {
        let muted = self.muted.load(Ordering::Relaxed);
        [if muted & MIC == 0 { 1.0 } else { 0.0 }, if muted & AUX == 0 { 1.0 } else { 0.0 }]
    }
    pub(crate) fn output_targets(&self) -> [f32; 2] {
        let muted = self.muted.load(Ordering::Relaxed);
        let master = f32::from_bits(self.master.load(Ordering::Relaxed));
        let send = f32::from_bits(self.send.load(Ordering::Relaxed));
        [if muted & (ALL | OUTPUT) == 0 { master } else { 0.0 },
            if self.has_send && muted & (ALL | SEND) == 0 { master * send } else { 0.0 }]
    }

    /// Requested state, not proof that another app or listener has received silence.
    pub fn describe(&self) -> String {
        let muted = self.muted.load(Ordering::Relaxed);
        let mut text = format!("master_gain={:.1}dB send_extra_gain={:.1}dB mute[mic={},aux={},output={},send={},all={}]",
            20.0 * f32::from_bits(self.master.load(Ordering::Relaxed)).log10(),
            20.0 * f32::from_bits(self.send.load(Ordering::Relaxed)).log10(),
            muted & MIC != 0, muted & AUX != 0, muted & OUTPUT != 0, muted & SEND != 0, muted & ALL != 0);
        let enabled = self.routes_enabled.load(Ordering::Relaxed);
        for (source, name) in [(0, "mic"), (1, "aux")] {
            if source == 1 && !self.has_aux { continue; }
            for (mix, destination) in [(0, "output"), (1, "send")].into_iter().take(self.mix_count()) {
                let index = source * 2 + mix;
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
/// A shared endpoint ramps both lanes on its one clock; distinct devices have their own ramps.
pub(crate) struct PlaybackGains {
    controls: Arc<LiveControls>,
    mixes: [usize; 2],
    count: usize,
    ramps: [GainRamp; 2],
}
impl PlaybackGains {
    pub fn new(rate: u32, mixes: &[usize], controls: Arc<LiveControls>) -> Result<Self, String> {
        if !(8_000..=192_000).contains(&rate) || !(1..=2).contains(&mixes.len())
            || mixes.iter().any(|&mix| mix >= controls.mix_count())
            || (mixes.len() == 2 && mixes[0] == mixes[1]) {
            return Err("invalid playback gain bindings".into());
        }
        let mut lanes = [0; 2];
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
    pub fn next_gains(&mut self) -> [f32; 2] { [self.ramps[0].next_gain(), self.ramps[1].next_gain()] }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn commands_validate_before_mutating_and_master_unmute_preserves_local_mutes() {
        let controls = LiveControls::new(-18.0, Some(-6.0), true).unwrap();
        for invalid in ["", "mute", "mute mic now", "stop now", "gain output -6", "gain master NaN", "gain send inf", "gain master 1", "gain master -97"] {
            assert!(invalid.parse::<LiveCommand>().is_err(), "{invalid}");
        }
        let initial = controls.output_targets();
        controls.apply("mute send".parse().unwrap()).unwrap();
        controls.apply("mute mic".parse().unwrap()).unwrap();
        controls.apply("mute all".parse().unwrap()).unwrap();
        assert_eq!(controls.output_targets(), [0.0; 2]);
        controls.apply("unmute all".parse().unwrap()).unwrap();
        assert_eq!(controls.output_targets(), [initial[0], 0.0]);
        assert_eq!(controls.source_targets(), [0.0, 1.0]);
        controls.apply("gain send -12".parse().unwrap()).unwrap();
        assert_eq!(controls.output_targets()[1], 0.0); // changing a fader never unmutes it
        controls.apply("unmute send".parse().unwrap()).unwrap();
        assert!((controls.output_targets()[1] - gain(-30.0)).abs() < 1e-7);
        let single = LiveControls::new(-18.0, None, false).unwrap();
        for command in ["mute aux", "mute send", "gain send -12"] {
            assert!(single.apply(command.parse().unwrap()).is_err());
        }
        let before = single.output_targets();
        assert!(single.apply(LiveCommand::Gain { target: GainTarget::Master, db: f32::NAN }).is_err());
        assert_eq!(single.output_targets(), before);
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
        let controls = Arc::new(LiveControls::new(-6.0, Some(-12.0), false).unwrap());
        let health = Arc::new(Health::default());
        let (mut writer, mut reader) = channel(48_000, 48_000, 30, 2, health.clone()).unwrap();
        let mut faders = PlaybackGains::new(48_000, &[0, 1], controls.clone()).unwrap();
        let priming = (reader.queue_ms * 48.0).round() as usize + 1024;
        for _ in 0..priming { writer.push([0.4, -0.2, 0.4, -0.2]); }
        for _ in 0..4 * QUANTUM { reader.next_frame(faders.next_gains()); }
        let before = reader.next_frame(faders.next_gains());
        assert!((before[0] - 0.4 * gain(-6.0)).abs() < 1e-4);
        assert!((before[2] - 0.4 * gain(-18.0)).abs() < 1e-4);
        controls.apply("mute output".parse().unwrap()).unwrap();
        faders.update();
        let mut after = [0.0; 4];
        for _ in 0..240 { after = reader.next_frame(faders.next_gains()); }
        assert_eq!(&after[..2], &[0.0; 2]);
        assert!((after[2] - before[2]).abs() < 1e-4);
        controls.apply("gain send -6".parse().unwrap()).unwrap();
        faders.update();
        for _ in 0..240 { after = reader.next_frame(faders.next_gains()); }
        assert!((after[2] - 0.4 * gain(-12.0)).abs() < 1e-4);
        controls.apply("mute all".parse().unwrap()).unwrap();
        faders.update();
        for _ in 0..240 { after = reader.next_frame(faders.next_gains()); }
        assert_eq!(after, [0.0; 4]);
        // Peak is measured AFTER mute/attenuation; stale queued samples are still consumed.
        reader.stats.peak.store(0, Ordering::Relaxed);
        assert_eq!(reader.next_frame(faders.next_gains()), [0.0; 4]);
        assert_eq!(reader.stats.peak.load(Ordering::Relaxed), 0);
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
        let controls = LiveControls::new(-18.0, Some(-6.0), true).unwrap();
        assert_eq!(controls.route_targets(), [1.0, 1.0, 0.0, 0.0]);
        for invalid in ["route mic", "route mic output", "route mic output yes", "route mic output off extra",
            "route none send on", "route aux all on", "level aux send", "level mic output 1",
            "level mic output -97", "level aux send NaN", "level aux send inf"] {
            assert!(invalid.parse::<LiveCommand>().is_err(), "{invalid}");
        }
        controls.apply("route mic output off".parse().unwrap()).unwrap();
        controls.apply("level mic output -6".parse().unwrap()).unwrap();
        assert_eq!(controls.route_targets(), [0.0, 1.0, 0.0, 0.0]);
        controls.apply("route aux send on".parse().unwrap()).unwrap();
        controls.apply("level aux send -12".parse().unwrap()).unwrap();
        let expected = [0.0, 1.0, 0.0, gain(-12.0)];
        assert_eq!(controls.route_targets(), expected);
        for command in ["mute all", "mute mic", "mute send", "unmute all", "gain master -3"] {
            controls.apply(command.parse().unwrap()).unwrap();
            assert_eq!(controls.route_targets(), expected);
        }
        controls.apply("route mic output on".parse().unwrap()).unwrap();
        assert_eq!(controls.source_targets()[0], 0.0); // routing cannot unmute the source
        assert_eq!(controls.output_targets()[1], 0.0); // nor the whole send mix
        assert_eq!(controls.route_targets()[0], gain(-6.0));
        assert!(controls.describe().contains("aux->send=on@-12.0dB"));
        let before = controls.describe();
        assert!(controls.apply(LiveCommand::RouteGain { source: RouteSource::Mic, mix: RouteMix::Output, db: f32::NAN }).is_err());
        assert_eq!(controls.describe(), before);
        let single = LiveControls::new(0.0, None, false).unwrap();
        let before = single.describe();
        for command in ["route aux output on", "level aux output -6", "route mic send on", "level mic send -3"] {
            assert!(single.apply(command.parse().unwrap()).is_err());
            assert_eq!(single.describe(), before);
        }
    }

}
