use clap::Parser;
use mixlive_audio_windows::{
    config::{Cli, Command, RouteOptions, SceneCommand},
    scene::{DeviceScene, Result},
};

fn main() {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("MixLive: {error}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Devices => device::list(),
        Command::Check(route) => device::check(&route),
        Command::Run { route, seconds } => device::run(&route, seconds),
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
            SceneCommand::Run { file, seconds } => device::run(&DeviceScene::read(&file)?.route, seconds),
        },
    }
}

#[cfg(windows)]
mod device {
    use super::{Result, RouteOptions};
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

    pub fn run(route: &RouteOptions, seconds: u64) -> Result<()> {
        // Both direct and saved routes re-resolve exact IDs and actual formats at launch.
        let prepared = Prepared::new(route)?;
        println!("{}", prepared.describe());
        println!("Use headphones at LOW hardware volume. Disable other monitoring paths.\nStarting a {seconds}s live mic route; no recording. Sample clipping guard is not a mastering limiter.");
        let active = prepared.start()?;
        for line in &active.timing { println!("{line}"); }
        let started = Instant::now();
        let mut next_report = Duration::ZERO;
        while started.elapsed() < Duration::from_secs(seconds) {
            if let Some(error) = active.health.error() { return Err(error.into()); }
            if started.elapsed() >= next_report {
                println!("{}", active.report());
                next_report += Duration::from_secs(1);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        println!("Stopped. {}", active.report());
        if let Some(error) = active.health.error() { return Err(error.into()); }
        Ok(())
    }
}

#[cfg(not(windows))]
mod device {
    use super::{Result, RouteOptions};

    const UNSUPPORTED: &str = "audio devices are implemented for Windows only; scene save/show do not require devices";

    pub fn list() -> Result<()> { Err(UNSUPPORTED.into()) }
    pub fn check(_route: &RouteOptions) -> Result<()> { Err(UNSUPPORTED.into()) }
    pub fn run(_route: &RouteOptions, _seconds: u64) -> Result<()> { Err(UNSUPPORTED.into()) }
}
