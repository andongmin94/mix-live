#![cfg_attr(windows, windows_subsystem = "windows")]

use clap::Parser;
use std::path::PathBuf;

#[cfg(windows)]
mod session;
#[cfg(windows)]
mod ui;

#[derive(Parser)]
#[command(
    name = "mixlive",
    version,
    about = "MixLive native Scene runner. Opening a Scene never starts audio; press Start in the window."
)]
struct Args {
    /// Optional schema 3 Scene to open. Devices and plugins are opened only after Start.
    scene: Option<PathBuf>,
}

fn main() {
    let args = Args::parse();
    #[cfg(windows)]
    ui::run(args.scene);
    #[cfg(not(windows))]
    {
        let _ = args;
        eprintln!("MixLive's native audio screen requires Windows 11 x64.");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_accepts_only_an_optional_scene_and_has_no_automatic_start() {
        assert!(Args::try_parse_from(["mixlive"]).unwrap().scene.is_none());
        assert_eq!(
            Args::try_parse_from(["mixlive", "노래.mixlive-scene.json"])
                .unwrap()
                .scene,
            Some(PathBuf::from("노래.mixlive-scene.json"))
        );
        assert!(Args::try_parse_from(["mixlive", "--start"]).is_err());
        assert!(Args::try_parse_from(["mixlive", "--vst3", "other.vst3"]).is_err());
    }
}
