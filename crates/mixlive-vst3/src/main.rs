use clap::Parser;
use mixlive_vst3::{Cli, Result};

fn main() {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("MixLive plugins: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn run(_cli: Cli) -> Result<()> {
    Err("VST3 preflight currently targets Windows x64; it does not open an audio device".into())
}

#[cfg(windows)]
fn run(cli: Cli) -> Result<()> {
    use mixlive_vst3::{Command, host, inspect, write_new};
    match cli.command {
        Command::List { directory } => println!("{}", serde_json::to_string_pretty(&host::list(&directory)?)?),
        Command::Inspect(options) => println!("{}", serde_json::to_string_pretty(&inspect(&std::env::current_exe()?, &options)?)?),
        Command::Probe { options, report } => write_new(&report, &serde_json::to_vec(&host::probe(&options)?)?)?,
    }
    Ok(())
}
