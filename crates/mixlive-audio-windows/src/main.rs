use clap::{Args, CommandFactory, FromArgMatches};
use mixlive_audio_windows::{
    config::{Cli, Command, RouteOptions, SceneCommand},
    plugin::PluginArgs,
    scene::{DeviceScene, Result},
};

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
    if plugin.enabled() && !matches!(&cli.command,
        Command::Run { .. } | Command::Scene { command: SceneCommand::Run { .. } }) {
        return Err("VST3 launch options are supported by run and scene run only; use mixlive-plugins to inspect/edit".into());
    }
    match cli.command {
        Command::Devices => device::list(),
        Command::Check(route) => device::check(&route),
        Command::Run { route, seconds, interactive } => device::run(&route, seconds, interactive, &plugin),
        Command::Scene { command } => match command {
            SceneCommand::Save { file, route } => {
                DeviceScene::new(route)?.save_new(&file)?;
                println!("Saved {}. No audio device was opened or verified; use scene check before scene run.", file.display());
                Ok(())
            }
            SceneCommand::Show { file } => {
                println!("{}", serde_json::to_string_pretty(&DeviceScene::read(&file)?)?);
                Ok(())
            }
            SceneCommand::Check { file } => device::check(&DeviceScene::read(&file)?.route),
            SceneCommand::Run { file, seconds, interactive } => device::run(&DeviceScene::read(&file)?.route, seconds, interactive, &plugin),
        },
    }
}

#[cfg(any(windows, test))]
fn read_commands(reader: impl std::io::BufRead, mut feedback: impl std::io::Write,
    controls: &mixlive_audio_windows::control::LiveControls) -> std::io::Result<()> {
    use mixlive_audio_windows::control::LiveCommand;
    let result = (|| {
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() { continue; }
            match line.parse::<LiveCommand>().and_then(|command| controls.apply(command)) {
                Ok(()) => writeln!(feedback, "{}", controls.describe())?,
                Err(error) => writeln!(feedback, "Control rejected: {error}")?,
            }
            if controls.stop_requested() { break; }
        }
        Ok(())
    })();
    let _ = controls.apply(LiveCommand::Stop);
    result
}

#[cfg(windows)]
mod device {
    use super::{PluginArgs, Result, RouteOptions};
    use mixlive_audio_windows::windows::{Prepared, list_devices};
    use std::time::{Duration, Instant};

    pub fn list() -> Result<()> {
        let rows = list_devices()?;
        if rows.is_empty() { println!("No WASAPI capture/playback endpoints available."); }
        for row in rows { println!("{row}"); }
        Ok(())
    }

    pub fn check(route: &RouteOptions) -> Result<()> {
        println!("{}", Prepared::new(route)?.describe());
        Ok(())
    }

    pub fn run(route: &RouteOptions, seconds: u64, interactive: bool, plugin: &PluginArgs) -> Result<()> {
        let prepared = Prepared::new(route)?;
        println!("{}", prepared.describe());
        println!("Use headphones at LOW hardware volume. Disable other monitoring paths.\nStarting a {seconds}s live mic route; no recording. Sample clipping guard is not a mastering limiter.");
        if plugin.enabled() {
            println!("Loading a trusted native VST3 chain in this process. A plugin crash/hang is not isolated. No delay compensation; all processing must finish before shutdown can complete.");
        }
        if !plugin.vst3_editor.is_empty() {
            println!("Opening selected LIVE plugin editors. Closing a window keeps audio running. Restart/bus/latency requests stop the route. Use stop or the session time limit to finish.");
        }
        let mut active = prepared.start(plugin)?;
        for line in &active.timing { println!("{line}"); }
        if interactive {
            println!("{}", mixlive_audio_windows::control::HELP);
            let controls = active.controls.clone();
            std::thread::Builder::new().name("mixlive-console".into()).spawn(move || {
                if let Err(error) = super::read_commands(std::io::stdin().lock(), std::io::stderr(), &controls) {
                    eprintln!("Console closed: {error}");
                }
            })?;
        }
        let started = Instant::now();
        let mut next_report = Duration::ZERO;
        let poll_interval = Duration::from_millis(if plugin.vst3_editor.is_empty() { 50 } else { 16 });
        while started.elapsed() < Duration::from_secs(seconds) && !active.controls.stop_requested() {
            active.poll_plugin()?;
            if let Some(error) = active.health.error() { return Err(error.into()); }
            if started.elapsed() >= next_report {
                println!("{} | {}", active.report(), active.controls.describe());
                next_report += Duration::from_secs(1);
            }
            std::thread::sleep(poll_interval);
        }
        let report = active.report();
        let finished = active.finish(plugin.vst3_chain_out.as_deref());
        println!("Stopped. {report}");
        finished?;
        if let Some(path) = &plugin.vst3_chain_out { println!("Saved VST3 chain: {}", path.display()); }
        Ok(())
    }
}

#[cfg(not(windows))]
mod device {
    use super::{PluginArgs, Result, RouteOptions};
    const UNSUPPORTED: &str = "audio devices are implemented for Windows only; scene save/show do not require devices";
    pub fn list() -> Result<()> { Err(UNSUPPORTED.into()) }
    pub fn check(_route: &RouteOptions) -> Result<()> { Err(UNSUPPORTED.into()) }
    pub fn run(_route: &RouteOptions, _seconds: u64, _interactive: bool, _plugin: &PluginArgs) -> Result<()> { Err(UNSUPPORTED.into()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use mixlive_audio_windows::control::LiveControls;
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
        let controls = LiveControls::new(-18.0, None, false).unwrap();
        let mut feedback = Vec::new();
        read_commands(Cursor::new(b"\nmute output\ngain master NaN\nmute aux\ngain master -12\nstatus\nstop\ngain master 0\n"),
            &mut feedback, &controls).unwrap();
        assert!(controls.stop_requested());
        let text = String::from_utf8(feedback).unwrap();
        assert!(text.contains("Control rejected:"));
        assert!(controls.describe().contains("master_gain=-12.0dB"));
        assert!(controls.describe().contains("output=true"));
        let controls = LiveControls::new(-18.0, None, false).unwrap();
        read_commands(Cursor::new(b"status\n"), Vec::new(), &controls).unwrap();
        assert!(controls.stop_requested());
    }

    #[test]
    fn ordered_vst3_chains_work_for_direct_and_scene_runs_without_silent_persistence() {
        for mut words in [vec!["mixlive-audio", "run", "--input", "mic", "--output", "phones"],
            vec!["mixlive-audio", "scene", "run", "singing.json"]] {
            words.extend(["--vst3", "eq.vst3", "--vst3", "room.vst3", "--vst3-editor", "2", "--vst3-editor", "1", "--vst3-chain-out", "voice.json"]);
            let matches = parser().try_get_matches_from(words).unwrap();
            let plugin = PluginArgs::from_arg_matches(&matches).unwrap();
            assert_eq!(plugin.vst3, vec![std::path::PathBuf::from("eq.vst3"), "room.vst3".into()]);
            assert_eq!(plugin.vst3_editor, [2, 1]);
            assert_eq!(plugin.vst3_chain_out, Some("voice.json".into()));
            assert!(plugin.validate().is_ok());
            assert!(Cli::from_arg_matches(&matches).is_ok());
        }
        let matches = parser().try_get_matches_from(["mixlive-audio", "scene", "run", "singing.json", "--vst3-chain", "voice.json"]).unwrap();
        assert_eq!(PluginArgs::from_arg_matches(&matches).unwrap().vst3_chain, Some("voice.json".into()));
        assert!(parser().try_get_matches_from(["mixlive-audio", "devices", "--vst3", "effect.vst3", "--vst3-chain", "voice.json"]).is_err());
        assert!(parser().try_get_matches_from(["mixlive-audio", "devices", "--vst3-editor", "0"]).is_err());
        for legacy in ["--vst3-state", "--vst3-state-out", "--vst3-class"] {
            assert!(parser().try_get_matches_from(["mixlive-audio", "devices", legacy, "old"]).is_err());
        }
        for words in [vec!["mixlive-audio", "devices", "--vst3-editor", "1"],
            vec!["mixlive-audio", "devices", "--vst3-chain-out", "voice.json"],
            vec!["mixlive-audio", "devices", "--vst3", "effect.vst3"]] {
            let matches = parser().try_get_matches_from(words).unwrap();
            assert!(run(Cli::from_arg_matches(&matches).unwrap(), PluginArgs::from_arg_matches(&matches).unwrap()).is_err());
        }
    }
}
