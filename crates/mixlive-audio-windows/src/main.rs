use clap::Parser;
use mixlive_audio_windows::config::Cli;

fn main() {
    let cli = Cli::parse();
    if let Err(error) = run(cli) {
        eprintln!("MixLive: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn run(_cli: Cli) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    Err("audio devices are implemented for Windows only; portable bridge tests still run on this platform".into())
}

#[cfg(windows)]
fn run(cli: Cli) -> mixlive_audio_windows::windows::Result<()> {
    use std::time::{Duration, Instant};
    use mixlive_audio_windows::{config::Command, windows::{Prepared, list_devices}};
    match cli.command {
        Command::Devices => {
            let rows = list_devices()?;
            if rows.is_empty() { println!("No WASAPI capture/playback endpoints available."); }
            for row in rows { println!("{row}"); }
        }
        Command::Check(options) => println!("{}", Prepared::new(&options)?.describe()),
        Command::Run { route, seconds } => {
            let prepared = Prepared::new(&route)?;
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
        }
    }
    Ok(())
}
