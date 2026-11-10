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
    Err("VST3 tools currently target Windows x64; they do not open an audio device".into())
}

#[cfg(windows)]
fn run(cli: Cli) -> Result<()> {
    use mixlive_vst3::{Command, editor, host, inspect, render, write_new};
    match cli.command {
        Command::List { directory } => println!("{}", serde_json::to_string_pretty(&host::list(&directory)?)?),
        Command::Inspect(options) => println!("{}", serde_json::to_string_pretty(&inspect(&std::env::current_exe()?, &options)?)?),
        Command::Probe { options, report } => write_new(&report, &serde_json::to_vec(&host::probe(&options)?)?)?,
        Command::Edit(options) => {
            eprintln!("No audio preview or microphone capture. Close the plugin window normally to finish.");
            if let Some(path) = &options.state_out { eprintln!("Save on close to a NEW file: {}", path.display()); }
            else { eprintln!("No --state-out: changes will be discarded when the window closes."); }
            println!("{}", serde_json::to_string_pretty(&editor::edit(&std::env::current_exe()?, &options)?)?);
        }
        Command::EditChild { options, report } => write_new(&report, &serde_json::to_vec(&editor::edit_in_child(&options)?)?)?,
        Command::Render(options) => println!("{}", serde_json::to_string_pretty(&render::render_file(&std::env::current_exe()?, &options)?)?),
        Command::RenderChild { options, report } => write_new(&report, &serde_json::to_vec(&render::render_in_child(&options)?)?)?,
    }
    Ok(())
}
