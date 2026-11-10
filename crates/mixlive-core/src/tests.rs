use crate::{ChannelLayout, ConfigError, Engine, Input, Mix, ProcessError, Scene, Send, Source};

fn scene() -> Scene {
    Scene {
        sources: vec![
            Source {
                name: "Microphone".into(),
                channels: ChannelLayout::Mono,
                gain_db: 0.0,
                muted: false,
            },
            Source {
                name: "MR".into(),
                channels: ChannelLayout::Stereo,
                gain_db: 0.0,
                muted: false,
            },
            Source {
                name: "Discord return".into(),
                channels: ChannelLayout::Stereo,
                gain_db: 0.0,
                muted: false,
            },
        ],
        mixes: vec![
            Mix {
                name: "Discord send".into(),
                gain_db: 0.0,
                muted: false,
                excluded_sources: vec![2],
            },
            Mix {
                name: "Monitor".into(),
                gain_db: 0.0,
                muted: false,
                excluded_sources: vec![],
            },
        ],
        sends: vec![
            Send { source: 0, mix: 0, gain_db: 0.0, enabled: true },
            Send { source: 1, mix: 0, gain_db: 0.0, enabled: true },
            Send { source: 0, mix: 1, gain_db: 0.0, enabled: true },
            Send { source: 1, mix: 1, gain_db: 0.0, enabled: true },
            Send { source: 2, mix: 1, gain_db: 0.0, enabled: true },
        ],
    }
}

fn close(actual: f32, expected: f32) {
    assert!((actual - expected).abs() < 1e-6, "{actual} != {expected}");
}

fn inputs<'a>(mic: &'a [f32], mr: &'a [f32], call: &'a [f32]) -> [Input<'a>; 3] {
    [
        Input::Mono(mic),
        Input::Stereo { left: mr, right: mr },
        Input::Stereo { left: call, right: call },
    ]
}

#[test]
fn sends_exclude_call_return_and_keep_monitor_independent() {
    let mut engine = Engine::new(&scene(), 8).expect("valid scene");
    engine.render(2, &inputs(&[0.1; 2], &[0.2; 2], &[0.3; 2])).expect("render");
    let send = engine.output(0).expect("send output");
    let monitor = engine.output(1).expect("monitor output");
    close(send.left[0], 0.3);
    close(send.right[1], 0.3);
    close(monitor.left[0], 0.6);
    close(monitor.sample_peak(), 0.6);
}

#[test]
fn source_send_and_mix_gains_are_applied_at_their_own_stage() {
    let mut config = scene();
    config.sources[0].gain_db = -6.0;
    config.sends[1].gain_db = -6.0;
    config.mixes[0].gain_db = -6.0;
    let gain = 10.0_f32.powf(-6.0 / 20.0);
    let mut engine = Engine::new(&config, 1).expect("valid scene");
    engine.render(1, &inputs(&[0.1], &[0.2], &[0.3])).expect("render");
    close(engine.output(0).expect("send").left[0], (0.1 * gain + 0.2 * gain) * gain);
    close(engine.output(1).expect("monitor").left[0], 0.1 * gain + 0.2 + 0.3);
}

#[test]
fn mute_disabled_send_and_stereo_layout_are_preserved() {
    let mut config = scene();
    config.sources[0].muted = true;
    config.mixes[1].muted = true;
    let mut engine = Engine::new(&config, 1).expect("valid scene");
    let input = [
        Input::Mono(&[0.9]),
        Input::Stereo { left: &[0.2], right: &[-0.4] },
        Input::Stereo { left: &[0.3], right: &[0.3] },
    ];
    engine.render(1, &input).expect("render");
    close(engine.output(0).expect("send").left[0], 0.2);
    close(engine.output(0).expect("send").right[0], -0.4);
    close(engine.output(1).expect("monitor").sample_peak(), 0.0);
    config.sends[1].enabled = false;
    let mut engine = Engine::new(&config, 1).expect("valid scene");
    engine.render(1, &input).expect("render");
    close(engine.output(0).expect("send").sample_peak(), 0.0);
}

#[test]
fn variable_blocks_clear_old_content_and_keep_output_storage() {
    let mut engine = Engine::new(&scene(), 128).expect("valid scene");
    engine.render(128, &inputs(&[0.1; 128], &[0.2; 128], &[0.3; 128])).expect("render");
    let address = engine.output(0).expect("send").left.as_ptr();
    engine.render(1, &inputs(&[0.0], &[0.0], &[0.0])).expect("render");
    let short = engine.output(0).expect("send");
    assert_eq!(short.left.len(), 1);
    assert_eq!(short.left.as_ptr(), address);
    close(short.sample_peak(), 0.0);
    engine.render(17, &inputs(&[0.0; 17], &[0.0; 17], &[0.0; 17])).expect("render");
    assert_eq!(engine.output(0).expect("send").left.len(), 17);
    close(engine.output(0).expect("send").sample_peak(), 0.0);
}

#[test]
fn rejects_invalid_routes_gains_and_mix_minus_feedback() {
    let mut config = scene();
    config.sends.push(Send { source: 2, mix: 0, gain_db: 0.0, enabled: true });
    assert_eq!(config.validate(), Err(ConfigError::ExcludedSource { source: 2, mix: 0 }));
    config.sends.pop();
    config.sends.push(config.sends[0]);
    assert_eq!(config.validate(), Err(ConfigError::DuplicateSend { source: 0, mix: 0 }));
    config.sends.pop();
    config.sends[0].source = 99;
    assert_eq!(config.validate(), Err(ConfigError::InvalidSource { index: 99 }));
    config.sends[0].source = 0;
    config.sends[0].mix = 99;
    assert_eq!(config.validate(), Err(ConfigError::InvalidMix { index: 99 }));
    config.sends[0].mix = 0;
    for gain in [f32::NAN, f32::INFINITY, 25.0, -97.0] {
        config.sources[0].gain_db = gain;
        assert!(matches!(config.validate(), Err(ConfigError::InvalidGain { .. })));
    }
}

#[test]
fn invalid_input_never_exposes_an_old_or_partial_output() {
    let mut engine = Engine::new(&scene(), 2).expect("valid scene");
    engine.render(1, &inputs(&[0.1], &[0.2], &[0.3])).expect("render");
    assert!(engine.output(0).is_some());
    assert_eq!(engine.render(3, &[]), Err(ProcessError::InvalidFrameCount));
    assert!(engine.output(0).is_none());
    assert_eq!(engine.render(1, &[]), Err(ProcessError::SourceCountMismatch));
    assert_eq!(engine.render(2, &inputs(&[0.0], &[0.0; 2], &[0.0; 2])), Err(ProcessError::ShortInput { source: 0 }));
    assert_eq!(engine.render(1, &inputs(&[f32::NAN], &[0.0], &[0.0])), Err(ProcessError::NonFiniteInput { source: 0 }));
    let bad_layout = [Input::Mono(&[0.0]), Input::Mono(&[0.0]), Input::Mono(&[0.0])];
    assert_eq!(engine.render(1, &bad_layout), Err(ProcessError::ChannelMismatch { source: 1 }));
    assert!(engine.output(0).is_none());
    engine.render(1, &inputs(&[0.0], &[0.0], &[0.0])).expect("valid next block");
    close(engine.output(0).expect("send").sample_peak(), 0.0);
}

#[test]
fn preserves_float_headroom_without_pretending_to_be_a_limiter() {
    let mut engine = Engine::new(&scene(), 1).expect("valid scene");
    engine.render(1, &inputs(&[0.75], &[0.75], &[0.0])).expect("render");
    close(engine.output(0).expect("send").sample_peak(), 1.5);
    assert_eq!(
        engine.render(1, &inputs(&[f32::MAX], &[f32::MAX], &[0.0])),
        Err(ProcessError::NonFiniteOutput { mix: 0 })
    );
    assert!(engine.output(0).is_none());
}

#[test]
fn rejects_empty_scenes_and_zero_capacity() {
    let mut config = scene();
    assert!(matches!(Engine::new(&config, 0), Err(ConfigError::ZeroBlockCapacity)));
    config.sources.clear();
    assert_eq!(config.validate(), Err(ConfigError::EmptySources));
    let mut config = scene();
    config.mixes.clear();
    assert_eq!(config.validate(), Err(ConfigError::EmptyMixes));
}
