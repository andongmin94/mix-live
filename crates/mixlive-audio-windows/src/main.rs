use clap::{Args, CommandFactory, FromArgMatches};
use mixlive_audio_windows::{
    config::{Cli, Command, RouteOptions, SceneCommand},
    plugin::{PluginArgs, ResolvedPlugins},
    scene::{AudioScene, Result},
};
use std::path::Path;
#[cfg(any(windows, test))]
use std::sync::Mutex;

fn parser() -> clap::Command { PluginArgs::augment_args(Cli::command()) }

fn main() {
    let matches = parser().get_matches();
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());
    let plugin = PluginArgs::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());
    if let Err(error) = run(cli, plugin) {
        eprintln!("MixLive: {error}");
        std::process::exit(1);
    }
}

fn run(cli: Cli, plugin: PluginArgs) -> Result<()> {
    plugin.validate()?;
    if plugin.requested() && !matches!(&cli.command,
        Command::Run { .. } | Command::Scene { command: SceneCommand::Run { .. } | SceneCommand::Save { .. } }) {
        return Err("VST3 options belong to run, scene save or scene run; use mixlive-plugins to inspect/edit".into());
    }
    match cli.command {
        Command::Devices => device::list(),
        Command::Apps => device::apps(),
        Command::Check(route) => device::check(&route),
        Command::Run { route, seconds, interactive, scene_out } => {
            validate_scene_output(scene_out.as_deref(), &plugin)?;
            let setup = plugin.resolve()?;
            device::run(&route, seconds, interactive, &setup, scene_out.as_deref())
        }
        Command::Scene { command } => match command {
            SceneCommand::Save { file, route } => {
                if !plugin.vst3_editor.is_empty() || plugin.vst3_chain_out.is_some() {
                    return Err("scene save does not run plugins; editor/chain-out options belong to run".into());
                }
                let scene = AudioScene::new(*route, plugin.resolve()?.effects)?;
                scene.save_new(&file)?;
                println!("Saved {} with {} embedded VST3 slot(s). No audio device or native plugin was opened; use scene check before scene run.", file.display(), scene.effects.len());
                Ok(())
            }
            SceneCommand::Show { file } => {
                println!("{}", serde_json::to_string_pretty(&AudioScene::read(&file)?)?);
                Ok(())
            }
            SceneCommand::Check { file } => {
                let scene = AudioScene::read(&file)?;
                device::check(&scene.route)?;
                println!("Scene contains {} VST3 slot(s). Their configuration was parsed; plugin availability, native compatibility and sound were NOT tested.", scene.effects.len());
                Ok(())
            }
            SceneCommand::Run { file, seconds, interactive, scene_out } => {
                validate_scene_output(scene_out.as_deref(), &plugin)?;
                let scene = AudioScene::read(&file)?;
                let setup = plugin.resolve_scene(scene.effects)?;
                device::run(&scene.route, seconds, interactive, &setup, scene_out.as_deref())
            }
        },
    }
}

fn validate_scene_output(path: Option<&Path>, plugin: &PluginArgs) -> Result<()> {
    let Some(path) = path else { return Ok(()); };
    if plugin.vst3_chain_out.is_some() {
        return Err("choose --scene-out OR --vst3-chain-out; a complete Scene already embeds the VST3 chain".into());
    }
    if path.file_name().is_none() { return Err("Scene output must name a new file".into()); }
    match path.symlink_metadata() {
        Ok(_) => return Err("Scene output already exists; choose a new file".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() { return Err("Scene output parent must be an existing directory".into()); }
    Ok(())
}

// Only the console and owner thread use this gate, never the audio callback/worker.
// The reader may remain blocked on stdin after a time limit, but cannot change a final snapshot.
#[cfg(any(windows, test))]
fn stop_console(controls: &mixlive_audio_windows::control::LiveControls, gate: &Mutex<()>) {
    let _guard = gate.lock().expect("console command gate");
    let _ = controls.apply(mixlive_audio_windows::control::LiveCommand::Stop);
}

#[cfg(any(windows, test))]
fn read_commands(reader: impl std::io::BufRead, mut feedback: impl std::io::Write,
    controls: &mixlive_audio_windows::control::LiveControls, gate: &Mutex<()>) -> std::io::Result<()> {
    use mixlive_audio_windows::control::LiveCommand;
    let result = (|| {
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() { continue; }
            let applied = {
                let _guard = gate.lock().expect("console command gate");
                if controls.stop_requested() { break; }
                line.parse::<LiveCommand>().and_then(|command| controls.apply(command))
            };
            match applied {
                Ok(()) => writeln!(feedback, "{}", controls.describe())?,
                Err(error) => writeln!(feedback, "Control rejected: {error}")?,
            }
            if controls.stop_requested() { break; }
        }
        Ok(())
    })();
    stop_console(controls, gate);
    result
}

#[cfg(windows)]
mod device {
    use super::{Mutex, Path, ResolvedPlugins, Result, RouteOptions};
    use mixlive_audio_windows::windows::{Prepared, list_devices};
    use std::{sync::Arc, time::{Duration, Instant}};

    pub fn list() -> Result<()> {
        let rows = list_devices()?;
        if rows.is_empty() { println!("No WASAPI capture/playback endpoints available."); }
        for row in rows { println!("{row}"); }
        Ok(())
    }

    pub fn apps() -> Result<()> {
        for app in mixlive_audio_windows::windows::list_apps() {
            match app.target {
                Ok(target) => println!("{}\n  PID {} + descendants", app.path, target.pid),
                Err(error) => println!("{}\n  unavailable: {error}", app.path),
            }
        }
        Ok(())
    }

    pub fn check(route: &RouteOptions) -> Result<()> {
        println!("{}", Prepared::new(route)?.describe());
        Ok(())
    }

    pub fn run(route: &RouteOptions, seconds: u64, interactive: bool, plugin: &ResolvedPlugins,
        scene_out: Option<&Path>) -> Result<()> {
        let prepared = Prepared::new(route)?;
        println!("{}", prepared.describe());
        println!("Use headphones at LOW hardware volume. Disable other monitoring paths.\nStarting a {seconds}s live mic route; no recording. Sample clipping guard is not a mastering limiter.");
        if !plugin.effects.is_empty() {
            println!("Loading a trusted native VST3 chain in this process. A plugin crash/hang is not isolated. Reported VST3 latency is compensated on outgoing AUX/app sends, not monitoring; all processing must finish before shutdown can complete.");
        }
        if !plugin.editors.is_empty() {
            println!("Opening selected LIVE plugin editors. Closing a window keeps audio running. Restart/bus/latency requests stop the route. Use stop or the session time limit to finish.");
        }
        let mut active = prepared.start(plugin)?;
        for line in &active.timing { println!("{line}"); }
        let command_gate = Arc::new(Mutex::new(()));
        if interactive {
            println!("{}", mixlive_audio_windows::control::HELP);
            let controls = active.controls.clone();
            let gate = command_gate.clone();
            std::thread::Builder::new().name("mixlive-console".into()).spawn(move || {
                if let Err(error) = super::read_commands(std::io::stdin().lock(), std::io::stderr(), &controls, &gate) {
                    eprintln!("Console closed: {error}");
                }
            })?;
        }
        let started = Instant::now();
        let mut next_report = Duration::ZERO;
        let poll_interval = Duration::from_millis(if plugin.editors.is_empty() { 50 } else { 16 });
        while started.elapsed() < Duration::from_secs(seconds) && !active.controls.stop_requested() {
            // The console owns its Win32 message loop. The GPUI caller never pumps here.
            if !plugin.editors.is_empty() && !mixlive_vst3::editor::pump_editor_messages() {
                return Err("native editor message loop was interrupted".into());
            }
            active.poll_plugin()?;
            if let Some(error) = active.health.error() { return Err(error.into()); }
            if started.elapsed() >= next_report {
                println!("{} | {}", active.report(&active.take_levels()), active.controls.describe());
                next_report += Duration::from_secs(1);
            }
            std::thread::sleep(poll_interval);
        }
        super::stop_console(&active.controls, &command_gate);
        let report = active.report(&active.take_levels());
        let finished = match scene_out {
            Some(path) => active.finish_scene(route.clone()).and_then(|scene| scene.save_new(path)),
            None => active.finish(plugin.chain_out.as_deref()),
        };
        println!("Stopped. {report}");
        finished?;
        if let Some(path) = scene_out { println!("Saved complete Scene: {}", path.display()); }
        if let Some(path) = &plugin.chain_out { println!("Saved VST3 chain: {}", path.display()); }
        Ok(())
    }
}

#[cfg(not(windows))]
mod device {
    use super::{Path, ResolvedPlugins, Result, RouteOptions};
    const UNSUPPORTED: &str = "audio devices are implemented for Windows only; scene save/show do not require devices";
    pub fn list() -> Result<()> { Err(UNSUPPORTED.into()) }
    pub fn apps() -> Result<()> { Err(UNSUPPORTED.into()) }
    pub fn check(_route: &RouteOptions) -> Result<()> { Err(UNSUPPORTED.into()) }
    pub fn run(_route: &RouteOptions, _seconds: u64, _interactive: bool, _plugin: &ResolvedPlugins,
        _scene_out: Option<&Path>) -> Result<()> { Err(UNSUPPORTED.into()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use mixlive_audio_windows::control::LiveControls;
    use mixlive_effects::VocalConfig;
    use std::io::Cursor;

    #[test]
    fn interactive_is_opt_in_for_direct_and_saved_routes_not_a_scene_setting() {
        for flag in [false, true] {
            let mut direct = vec!["mixlive-audio", "run", "--input", "mic", "--output", "phones"];
            let mut saved = vec!["mixlive-audio", "scene", "run", "voice.mixlive-scene.json"];
            if flag { direct.push("--interactive"); saved.push("--interactive"); }
            assert!(matches!(Cli::try_parse_from(direct).unwrap().command,
                Command::Run { interactive, .. } if interactive == flag));
            assert!(matches!(Cli::try_parse_from(saved).unwrap().command,
                Command::Scene { command: SceneCommand::Run { interactive, .. } } if interactive == flag));
        }
        assert!(Cli::try_parse_from(["mixlive-audio", "scene", "save", "voice.json", "--input", "mic", "--output", "phones", "--interactive"]).is_err());
    }

    #[test]
    fn terminal_commands_apply_without_hardware_reject_bad_input_and_stop_on_eof() {
        let controls = LiveControls::new(-18.0, [Some(0.0), None, None], false, 0, VocalConfig::default()).unwrap();
        let mut feedback = Vec::new();
        read_commands(Cursor::new(b"\nmute output\ngain master NaN\nmute aux\ngain master -12\nstatus\nstop\ngain master 0\n"),
            &mut feedback, &controls, &Mutex::new(())).unwrap();
        assert!(controls.stop_requested());
        let text = String::from_utf8(feedback).unwrap();
        assert!(text.contains("Control rejected:"));
        assert!(controls.describe().contains("master_gain=-12.0dB"));
        assert!(controls.describe().contains("output=true"));
        let controls = LiveControls::new(-18.0, [Some(0.0), None, None], false, 0, VocalConfig::default()).unwrap();
        read_commands(Cursor::new(b"status\n"), Vec::new(), &controls, &Mutex::new(())).unwrap();
        assert!(controls.stop_requested());
    }

    #[test]
    fn stopped_console_cannot_change_the_complete_scene_snapshot() {
        let cli = Cli::try_parse_from(["mixlive-audio", "check", "--input", "mic", "--output", "phones"]).unwrap();
        let Command::Check(mut route) = cli.command else { panic!("expected route"); };
        let controls = LiveControls::from_route(&route).unwrap();
        let gate = Mutex::new(());
        controls.apply("gain master -12".parse().unwrap()).unwrap();
        controls.apply("vocal trim -3".parse().unwrap()).unwrap();
        controls.apply("mute mic".parse().unwrap()).unwrap();
        // A late line (for example after the time limit) must never mutate the saved targets.
        stop_console(&controls, &gate);
        read_commands(Cursor::new(b"gain master 0\nvocal trim 0\nunmute mic\n"), Vec::new(), &controls, &gate).unwrap();
        controls.snapshot_into(&mut route).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("snapshot.mixlive-scene.json");
        AudioScene::new(route, vec![]).unwrap().save_new(&path).unwrap();
        let saved = AudioScene::read(&path).unwrap();
        assert!((saved.route.output_gain_db + 12.0).abs() < 1e-4);
        assert_eq!(saved.route.input_gain_db, -3.0);
        assert!(saved.route.mix.mute_mic);
    }

    #[test]
    fn chain_selection_and_per_run_editor_actions_are_not_silently_ignored() {
        let words = ["mixlive-audio", "run", "--input", "mic", "--output", "phones",
            "--vst3", "eq.vst3", "--vst3", "room.vst3", "--vst3-editor", "2", "--vst3-chain-out", "voice.json"];
        let matches = parser().try_get_matches_from(words).unwrap();
        let setup = PluginArgs::from_arg_matches(&matches).unwrap().resolve().unwrap();
        assert_eq!(setup.effects.len(), 2);
        assert_eq!(setup.editors, [2]);
        assert_eq!(setup.chain_out, Some("voice.json".into()));
        assert!(Cli::from_arg_matches(&matches).is_ok());
        let matches = parser().try_get_matches_from(["mixlive-audio", "scene", "run", "singing.json", "--vst3-editor", "2"]).unwrap();
        assert_eq!(PluginArgs::from_arg_matches(&matches).unwrap().vst3_editor, [2]);
        assert!(parser().try_get_matches_from(["mixlive-audio", "devices", "--vst3", "effect.vst3", "--vst3-chain", "voice.json"]).is_err());
        assert!(parser().try_get_matches_from(["mixlive-audio", "devices", "--vst3-editor", "0"]).is_err());
        for legacy in ["--vst3-state", "--vst3-state-out", "--vst3-class"] {
            assert!(parser().try_get_matches_from(["mixlive-audio", "devices", legacy, "old"]).is_err());
        }
        for words in [vec!["mixlive-audio", "devices", "--vst3-editor", "1"],
            vec!["mixlive-audio", "devices", "--vst3-chain-out", "voice.json"],
            vec!["mixlive-audio", "devices", "--vst3", "effect.vst3"],
            vec!["mixlive-audio", "scene", "save", "unused.json", "--input", "mic", "--output", "phones", "--vst3", "effect.vst3", "--vst3-editor", "1"]] {
            let matches = parser().try_get_matches_from(words).unwrap();
            assert!(run(Cli::from_arg_matches(&matches).unwrap(), PluginArgs::from_arg_matches(&matches).unwrap()).is_err());
        }
    }
}
