//! A synthetic, in-memory scenario. No microphone, Discord, driver, or speaker is opened.
use mixlive_core::{ChannelLayout, Engine, Input, Mix, Scene, Send, Source};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let scene = Scene {
        sources: vec![
            Source { name: "Microphone".into(), channels: ChannelLayout::Mono, gain_db: 0.0, muted: false },
            Source { name: "Chrome MR".into(), channels: ChannelLayout::Stereo, gain_db: 0.0, muted: false },
            Source { name: "Discord return".into(), channels: ChannelLayout::Stereo, gain_db: 0.0, muted: false },
        ],
        mixes: vec![
            Mix { name: "Discord send".into(), gain_db: 0.0, muted: false, excluded_sources: vec![2] },
            Mix { name: "Headphones".into(), gain_db: 0.0, muted: false, excluded_sources: vec![] },
        ],
        sends: vec![
            Send { source: 0, mix: 0, gain_db: 0.0, enabled: true },
            Send { source: 1, mix: 0, gain_db: 0.0, enabled: true },
            Send { source: 0, mix: 1, gain_db: 0.0, enabled: true },
            Send { source: 1, mix: 1, gain_db: 0.0, enabled: true },
            Send { source: 2, mix: 1, gain_db: 0.0, enabled: true },
        ],
    };
    let mut engine = Engine::new(&scene, 128)?;
    let microphone = [0.1; 128];
    let mr = [0.2; 128];
    let remote_voice = [0.3; 128];
    engine.render(128, &[
        Input::Mono(&microphone),
        Input::Stereo { left: &mr, right: &mr },
        Input::Stereo { left: &remote_voice, right: &remote_voice },
    ])?;
    println!("MixLive: synthetic routing only; no devices, effects or virtual microphone.");
    for (index, mix) in scene.mixes.iter().enumerate() {
        let output = engine.output(index).ok_or("render produced no output")?;
        println!("{}: sample peak {:.3}", mix.name, output.sample_peak());
    }
    println!("Expected: Discord send 0.300; headphones 0.600 (return excluded from send).");
    Ok(())
}
