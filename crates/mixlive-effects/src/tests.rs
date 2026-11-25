use super::*;
use std::f64::consts::TAU;

fn tone(hz: f64, amplitude: f64, rate: u32, frames: usize) -> Vec<f32> {
    (0..frames).map(|i| (amplitude * (TAU * hz * i as f64 / f64::from(rate)).sin()) as f32).collect()
}
fn rms(samples: &[f32]) -> f64 {
    (samples.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>() / samples.len() as f64).sqrt()
}

#[test]
fn config_rejects_invalid_values_before_processing() {
    let base = VocalConfig { preset: Preset::Natural, ..VocalConfig::default() };
    for rate in [0, 8000, 16000, 192001] { assert!(VocalChain::new(rate, base).is_err()); }
    for rate in [32000, 44100, 48000, 96000, 192000] { assert!(VocalChain::new(rate, base).is_ok()); }
    for bad in [f32::NAN, f32::INFINITY, -0.1, 1.1] {
        assert!(VocalChain::new(48000, VocalConfig { strength: bad, ..base }).is_err());
    }
    assert!(VocalChain::new(48000, VocalConfig { input_gain_db: 19.0, ..base }).is_err());
    assert!(VocalChain::new(48000, VocalConfig { deess_threshold_db: f32::NAN, ..base }).is_err());
    assert!(VocalChain::new(48000, VocalConfig { compressor_threshold_db: -61.0, ..base }).is_err());
    assert_eq!("warm".parse::<Preset>().unwrap(), Preset::Warm);
    assert!("magic".parse::<Preset>().is_err());
}

#[test]
fn off_and_zero_strength_preserve_dry_audio_and_float_headroom() {
    let input = [0.0, -0.0, 0.25, -0.5, 1.5, -1.5];
    for config in [VocalConfig::default(), VocalConfig { preset: Preset::Bright, strength: 0.0, ..VocalConfig::default() }] {
        let mut chain = VocalChain::new(48000, config).unwrap();
        let mut output = [0.0; 6];
        let mut right = [0.0; 6];
        let meters = chain.process(&input, &mut output, &mut right).unwrap();
        assert_eq!(input, output);
        assert_eq!(input, right);
        assert_eq!(meters.compression_db, 0.0);
        assert_eq!(meters.deessing_db, 0.0);
        assert_eq!(meters.output_peak, 1.5);
    }
    let mut chain = VocalChain::new(8000, VocalConfig { input_gain_db: -6.0, ..VocalConfig::default() }).unwrap();
    let mut output = [0.0; 6];
    let mut right = [0.0; 6];
    chain.process(&input, &mut output, &mut right).unwrap();
    assert_eq!(output, right);
    for (a, b) in input.iter().zip(output) { assert!((f64::from(*a) * gain(-6.0) - f64::from(b)).abs() < 1e-7); }
}

#[test]
fn highpass_rejects_rumble_without_cutting_the_voice_band() {
    for (hz, maximum, minimum) in [(20.0, 0.08, 0.0), (1000.0, 1.01, 0.99)] {
        let input = tone(hz, 0.1, 48000, 24000);
        let mut filter = Filter::new(Type::HighPass, 80.0, Q_BUTTERWORTH_F64, 48000.0).unwrap();
        let output: Vec<f32> = input.iter().map(|&x| filter.run(f64::from(x)) as f32).collect();
        let ratio = rms(&output[12000..]) / rms(&input[12000..]);
        assert!(ratio > minimum && ratio < maximum, "{hz} Hz ratio {ratio}");
    }
}

#[test]
fn compressor_has_a_continuous_knee_correct_ratio_and_release() {
    assert_eq!(reduction(-30.0, -24.0, 3.0, 6.0), 0.0);
    assert!((reduction(-12.0, -24.0, 3.0, 6.0) - 8.0).abs() < 1e-12);
    for edge in [-27.0, -21.0] {
        assert!((reduction(edge - 1e-7, -24.0, 3.0, 6.0) - reduction(edge + 1e-7, -24.0, 3.0, 6.0)).abs() < 1e-6);
    }
    let mut compressor = Compressor::new(48000.0, -24.0, DynamicsConfig { ratio: 3.0, ..DynamicsConfig::default() });
    let x = gain(-12.0);
    let mut y = x;
    for _ in 0..48000 { y = compressor.run(x); }
    assert!((20.0 * (y / x).log10() + 8.0).abs() < 0.01);
    for _ in 0..144000 { assert_eq!(compressor.run(0.0), 0.0); }
    assert!(compressor.reduction < 0.001);
}

#[test]
fn deesser_attenuates_high_band_without_ducking_low_vowels() {
    for (hz, minimum, maximum) in [(200.0, 0.999, 1.001), (10000.0, 0.4, 0.75)] {
        let input = tone(hz, 0.5, 48000, 24000);
        let mut deesser = DeEsser::new(48000.0, -30.0, 6.0).unwrap();
        let output: Vec<f32> = input.iter().map(|&x| deesser.run(f64::from(x)) as f32).collect();
        let ratio = rms(&output[12000..]) / rms(&input[12000..]);
        assert!(ratio > minimum && ratio < maximum, "{hz} Hz ratio {ratio}");
        assert!(deesser.reduction <= 6.0 + 1e-10);
    }
}

#[test]
fn processing_is_invariant_to_callback_block_boundaries() {
    let mut input = tone(170.0, 0.3, 48000, 12000);
    for (index, value) in input.iter_mut().enumerate() {
        *value += (0.08 * (TAU * 9000.0 * index as f64 / 48000.0).sin()) as f32;
    }
    for preset in [Preset::Natural, Preset::Warm, Preset::Bright] {
        let config = VocalConfig { preset, ..VocalConfig::default() };
        let mut whole = vec![0.0; input.len()];
        let mut whole_right = vec![0.0; input.len()];
        VocalChain::new(48000, config).unwrap().process(&input, &mut whole, &mut whole_right).unwrap();
        assert_eq!(whole, whole_right);
        for size in [1, 17, 256, 1024] {
            let mut blocks = vec![0.0; input.len()];
            let mut right = vec![0.0; input.len()];
            let mut chain = VocalChain::new(48000, config).unwrap();
            for ((part, left), right) in input.chunks(size).zip(blocks.chunks_mut(size)).zip(right.chunks_mut(size)) {
                chain.process(part, left, right).unwrap();
            }
            assert_eq!(whole, blocks, "preset {preset}, block {size}");
            assert_eq!(whole_right, right);
        }
    }
}

#[test]
fn invalid_audio_silences_the_whole_block_and_resets_state() {
    let config = VocalConfig { preset: Preset::Natural, ..VocalConfig::default() };
    let mut chain = VocalChain::new(48000, config).unwrap();
    chain.process(&[0.2; 128], &mut [0.0; 128], &mut [0.0; 128]).unwrap();
    for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut left = [1.0; 3];
        let mut right = [1.0; 3];
        assert_eq!(chain.process(&[0.1, invalid, 0.2], &mut left, &mut right).unwrap_err(), DspError::NonFiniteAudio);
        assert_eq!(left, [0.0; 3]);
        assert_eq!(right, [0.0; 3]);
        let mut left = [1.0; 256];
        let mut right = [1.0; 256];
        chain.process(&[0.0; 256], &mut left, &mut right).unwrap();
        assert_eq!(left, [0.0; 256]);
        assert_eq!(right, [0.0; 256]);
    }
    assert_eq!(chain.process(&[], &mut [], &mut []).unwrap().output_peak, 0.0);
}

#[test]
fn reverb_settings_are_validated_even_when_room_is_disabled() {
    let valid = ReverbConfig { mix: 0.35, decay_secs: 1.5, predelay_ms: 25.0 };
    for rate in [8000, 16000, 32000, 44100, 48000, 96000, 192000] {
        assert!(VocalChain::new(rate, VocalConfig { reverb: valid, ..VocalConfig::default() }).is_ok());
    }
    for value in [f32::NAN, f32::INFINITY, -0.01, 1.01] {
        let config = VocalConfig { reverb: ReverbConfig { mix: value, ..valid }, ..VocalConfig::default() };
        assert_eq!(config.validate(), Err(DspError::ReverbMix));
    }
    for value in [f32::NAN, f32::INFINITY, 0.19, 5.01] {
        let config = VocalConfig { reverb: ReverbConfig { mix: 0.0, decay_secs: value, ..valid }, ..VocalConfig::default() };
        assert_eq!(config.validate(), Err(DspError::ReverbDecay));
    }
    for value in [f32::NAN, f32::INFINITY, -0.01, 80.01] {
        let config = VocalConfig { reverb: ReverbConfig { predelay_ms: value, ..valid }, ..VocalConfig::default() };
        assert_eq!(config.validate(), Err(DspError::ReverbPredelay));
    }
}

#[test]
fn stereo_room_keeps_dry_onset_and_predelay_shifts_only_the_wet_tail() {
    let mut input = vec![0.0; 48000];
    input[0] = 0.5;
    let render = |mix, predelay_ms| {
        let config = VocalConfig { reverb: ReverbConfig { mix, decay_secs: 1.2, predelay_ms }, ..VocalConfig::default() };
        let mut left = vec![0.0; input.len()];
        let mut right = vec![0.0; input.len()];
        let meters = VocalChain::new(48000, config).unwrap().process(&input, &mut left, &mut right).unwrap();
        let peak = left.iter().chain(&right).fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
        assert_eq!(meters.output_peak, peak);
        (left, right)
    };
    let (dry_left, dry_right) = render(0.0, 80.0);
    assert_eq!(dry_left, input);
    assert_eq!(dry_right, input);
    let (wet_left, wet_right) = render(1.0, 0.0);
    let (later_left, later_right) = render(1.0, 20.0);
    let delay = 960;
    assert!(later_left[..delay].iter().chain(&later_right[..delay]).all(|&sample| sample == 0.0));
    assert_eq!(&later_left[delay..], &wet_left[..input.len() - delay]);
    assert_eq!(&later_right[delay..], &wet_right[..input.len() - delay]);
    assert!(wet_left.iter().zip(&wet_right).any(|(a, b)| (a - b).abs() > 0.001));
    let (mixed_left, mixed_right) = render(0.25, 20.0);
    assert_eq!([mixed_left[0], mixed_right[0]], [0.375; 2]);
    for (mixed, wet) in mixed_left[1..].iter().zip(&later_left[1..]) {
        assert!((mixed - wet * 0.25).abs() < 1e-7);
    }
    for (mixed, wet) in mixed_right[1..].iter().zip(&later_right[1..]) {
        assert!((mixed - wet * 0.25).abs() < 1e-7);
    }
}

#[test]
fn room_decay_and_reset_are_finite_and_independent_of_block_size() {
    let mut input = vec![0.0; 96_000];
    input[0] = 0.5;
    let render = |decay_secs| {
        let config = VocalConfig { reverb: ReverbConfig { mix: 1.0, decay_secs, predelay_ms: 0.0 }, ..VocalConfig::default() };
        let mut left = vec![0.0; input.len()];
        let mut right = vec![0.0; input.len()];
        let mut chain = VocalChain::new(48000, config).unwrap();
        chain.process(&input, &mut left, &mut right).unwrap();
        assert!(left.iter().chain(&right).all(|sample| sample.is_finite() && sample.abs() < 1.0));
        (left, right)
    };
    let (short, _) = render(0.35);
    let (long, long_right) = render(2.0);
    assert!(rms(&long[38_400..48_000]) > 10.0 * rms(&short[38_400..48_000]));
    assert!(rms(&long[86_400..]) < 0.05 * rms(&long[4800..14_400]));
    let config = VocalConfig { reverb: ReverbConfig { mix: 1.0, decay_secs: 2.0, predelay_ms: 0.0 }, ..VocalConfig::default() };
    let mut chain = VocalChain::new(48000, config).unwrap();
    let mut left = vec![0.0; input.len()];
    let mut right = vec![0.0; input.len()];
    for ((part, left), right) in input.chunks(137).zip(left.chunks_mut(137)).zip(right.chunks_mut(137)) {
        chain.process(part, left, right).unwrap();
    }
    assert_eq!(left, long);
    assert_eq!(right, long_right);
    // A nonfinite input must not leave a tail in a reused state.
    assert_eq!(chain.process(&[f32::NAN], &mut [1.0], &mut [1.0]).unwrap_err(), DspError::NonFiniteAudio);
    let silence = vec![0.0; 12_000];
    chain.process(&silence, &mut left[..silence.len()], &mut right[..silence.len()]).unwrap();
    assert_eq!(&left[..silence.len()], silence.as_slice());
    assert_eq!(&right[..silence.len()], silence.as_slice());
    let mut left = [1.0; 2];
    let mut right = [1.0; 1];
    assert_eq!(chain.process(&[0.1; 2], &mut left, &mut right).unwrap_err(), DspError::BlockLength);
    assert_eq!(left, [0.0; 2]);
    assert_eq!(right, [0.0; 1]);
}

#[test]
fn live_trim_transition_is_exact_and_independent_of_block_partition() {
    let render = |blocks: &[usize]| {
        let mut chain = VocalChain::new(48000, VocalConfig::default()).unwrap();
        let mut result = Vec::new();
        for &count in blocks {
            chain.set_parameter(VocalParameter::Trim, -6.0).unwrap();
            let mut left = vec![0.0; count];
            let mut right = vec![0.0; count];
            chain.process(&vec![0.5; count], &mut left, &mut right).unwrap();
            assert_eq!(left, right);
            result.extend(left);
        }
        result
    };
    let whole = render(&[512]);
    assert_eq!(whole, render(&[1, 17, 113, 256, 125]));
    let target = (0.5 * gain(-6.0)) as f32;
    assert!(whole[0] < 0.5 && whole[0] > 0.499);
    assert_eq!(whole[479], target);
    assert_eq!(whole[511], target);
    assert!(whole.windows(2).all(|pair| pair[0] >= pair[1]));
}

#[test]
fn live_room_opens_from_zero_and_zero_blend_keeps_processing_history_current() {
    let config = VocalConfig { reverb: ReverbConfig { mix: 0.0, decay_secs: 1.2, predelay_ms: 0.0 }, ..VocalConfig::default() };
    let mut chain = VocalChain::new(48000, config).unwrap();
    let mut impulse = [0.0; 512];
    impulse[0] = 0.5;
    let mut left = [0.0; 512];
    let mut right = [0.0; 512];
    chain.process(&impulse, &mut left, &mut right).unwrap();
    assert_eq!(left, impulse);
    assert_eq!(right, impulse);
    chain.set_parameter(VocalParameter::ReverbMix, 0.5).unwrap();
    chain.set_parameter(VocalParameter::ReverbDecay, 2.0).unwrap();
    let mut saw_stereo = false;
    for _ in 0..8 {
        chain.process(&[0.0; 512], &mut left, &mut right).unwrap();
        saw_stereo |= left.iter().zip(&right).any(|(a, b)| (a - b).abs() > 0.001);
        assert!(left.iter().chain(&right).all(|s| s.is_finite()));
    }
    assert!(saw_stereo);
    chain.set_parameter(VocalParameter::ReverbMix, 0.0).unwrap();
    chain.process(&[0.1; 512], &mut left, &mut right).unwrap();
    assert_eq!(left[479..], [0.1; 33]);
    assert_eq!(right[479..], [0.1; 33]);
    let before = chain.config();
    assert!(chain.set_parameter(VocalParameter::ReverbDecay, f32::NAN).is_err());
    assert_eq!(chain.config(), before);
}

#[test]
fn zero_strength_can_enable_the_prepared_dynamics_and_update_thresholds() {
    let mut chain = VocalChain::new(48000, VocalConfig {
        preset: Preset::Natural, strength: 0.0, ..VocalConfig::default()
    }).unwrap();
    let input = tone(1000.0, 0.5, 48000, 4800);
    let mut left = vec![0.0; input.len()];
    let mut right = vec![0.0; input.len()];
    let meters = chain.process(&input, &mut left, &mut right).unwrap();
    assert_eq!(left, input);
    assert_eq!(meters.compression_db, 0.0);
    chain.set_parameter(VocalParameter::Strength, 1.0).unwrap();
    chain.set_parameter(VocalParameter::Compressor, -45.0).unwrap();
    chain.set_parameter(VocalParameter::Deesser, -40.0).unwrap();
    let meters = chain.process(&input, &mut left, &mut right).unwrap();
    assert!(meters.compression_db > 5.0);
    assert_eq!(left, right);
    assert!(rms(&left[2400..]) < rms(&input[2400..]) * 0.75);
    assert_eq!(chain.config().compressor_threshold_db, -45.0);
    assert_eq!(chain.config().deess_threshold_db, -40.0);
}

#[test]
fn echo_validates_even_while_disabled_and_preserves_dry_onset() {
    let valid = EchoConfig { mix: 0.25, time_ms: 125.0, feedback: 0.5 };
    for rate in [8000, 44100, 48000, 96000, 192000] {
        let config = VocalConfig { echo: valid, ..VocalConfig::default() };
        assert!(VocalChain::new(rate, config).is_ok());
    }
    for value in [f32::NAN, f32::INFINITY, 9.99, 1000.01] {
        let config = VocalConfig { echo: EchoConfig { time_ms: value, mix: 0.0, ..valid }, ..VocalConfig::default() };
        assert_eq!(config.validate(), Err(DspError::EchoTime));
    }
    for (parameter, error, invalid) in [(VocalParameter::EchoMix, DspError::EchoMix, 1.01),
        (VocalParameter::EchoFeedback, DspError::EchoFeedback, 0.86)] {
        let mut config = VocalConfig::default();
        for value in [f32::NAN, f32::INFINITY, -0.01, invalid] {
            assert_eq!(config.set_parameter(parameter, value), Err(error));
        }
    }
    let render = |sizes: &[usize]| {
        let mut chain = VocalChain::new(8000, VocalConfig { echo: valid, ..VocalConfig::default() }).unwrap();
        let mut input = vec![0.0; 4001];
        input[0] = 0.5;
        let mut l = vec![0.0; input.len()];
        let mut r = l.clone();
        let mut start = 0;
        for &size in sizes {
            let end = start + size;
            chain.process(&input[start..end], &mut l[start..end], &mut r[start..end]).unwrap();
            start = end;
        }
        assert_eq!(start, input.len());
        assert_eq!(l, r);
        l
    };
    let whole = render(&[4001]);
    assert_eq!(whole, render(&[1, 137, 512, 777, 1024, 1550]));
    for (i, &sample) in whole.iter().enumerate() {
        let expected = if i == 0 { 0.375 } else if i % 1000 == 0 {
            0.125 * 0.5_f32.powi((i / 1000 - 1) as i32)
        } else { 0.0 };
        assert_eq!(sample, expected, "frame {i}");
    }
}

#[test]
fn stereo_echo_keeps_channels_separate_and_live_ramps_keep_history_current() {
    let config = EchoConfig { mix: 0.0, time_ms: 10.0, feedback: 0.5 };
    let mut echo = Echo::new(8000, config);
    // Warm the ring with distinct left/right input; zero mix is bit-exact dry.
    for _ in 0..80 { assert_eq!(echo.run([0.5, -0.25]), [0.5, -0.25]); }
    echo.set_mix(1.0);
    echo.set_feedback(0.0);
    for i in 1..=80 {
        let wet = echo.run([0.0; 2]);
        let expected = f64::from(i.min(80)) / 80.0;
        assert!((wet[0] - 0.5 * expected).abs() < 1e-12);
        assert!((wet[1] + 0.25 * expected).abs() < 1e-12);
    }
    // Feedback reached zero, so after this ring's contents have played, silence returns.
    for _ in 0..160 { echo.run([0.0; 2]); }
    assert_eq!(echo.run([0.0; 2]), [0.0; 2]);
    echo.run([0.5, -0.25]);
    echo.reset();
    for _ in 0..160 { assert_eq!(echo.run([0.0; 2]), [0.0; 2]); }
    let mut chain = VocalChain::new(8000, VocalConfig { echo: config, ..VocalConfig::default() }).unwrap();
    let mut left = [0.0; 80];
    let mut right = [0.0; 80];
    chain.process(&[0.5; 80], &mut left, &mut right).unwrap();
    chain.set_parameter(VocalParameter::EchoMix, 1.0).unwrap();
    chain.set_parameter(VocalParameter::EchoFeedback, 0.0).unwrap();
    chain.process(&[0.0; 80], &mut left, &mut right).unwrap();
    assert_eq!(left, right);
    assert_eq!(left[79], 0.5);
    let before = chain.config();
    assert!(chain.set_parameter(VocalParameter::EchoFeedback, 1.0).is_err());
    assert_eq!(chain.config(), before);
    assert!(chain.process(&[f32::NAN], &mut [0.0], &mut [0.0]).is_err());
    chain.process(&[0.0; 80], &mut left, &mut right).unwrap();
    assert_eq!(left, [0.0; 80]);
}

#[test]
fn explicit_eq_changes_the_expected_bands_and_validates_while_off() {
    let base = VocalConfig { preset: Preset::Natural,
        dynamics: DynamicsConfig { ratio: 1.0, deess_max_db: 0.0, ..DynamicsConfig::default() },
        ..VocalConfig::default() };
    let level = |eq: EqConfig, hz: f64| {
        let config = VocalConfig { eq, ..base };
        let input = tone(hz, 0.02, 48000, 24000);
        let mut left = vec![0.0; input.len()];
        let mut right = left.clone();
        VocalChain::new(48000, config).unwrap().process(&input, &mut left, &mut right).unwrap();
        assert_eq!(left, right);
        rms(&left[12000..])
    };
    let eq = EqConfig::default();
    assert!(level(EqConfig { hpf_hz: 160.0, ..eq }, 60.0) < 0.25 * level(EqConfig { hpf_hz: 40.0, ..eq }, 60.0));
    for (hz, changed) in [(100.0, EqConfig { body_db: 6.0, ..eq }),
        (2800.0, EqConfig { presence_db: 6.0, ..eq }),
        (12000.0, EqConfig { air_db: 6.0, ..eq })] {
        assert!(level(changed, hz) > 1.5 * level(eq, hz), "{hz} Hz");
    }
    for eq in [EqConfig { hpf_hz: 39.0, ..eq }, EqConfig { body_db: f32::NAN, ..eq },
        EqConfig { presence_db: -6.1, ..eq }, EqConfig { air_db: 6.1, ..eq }] {
        assert!(VocalConfig { eq, ..VocalConfig::default() }.validate().is_err());
    }
}

#[test]
fn compressor_timing_and_deesser_limit_respond_to_explicit_settings() {
    let settings = DynamicsConfig { ratio: 4.0, attack_ms: 1.0, release_ms: 20.0, deess_max_db: 0.0 };
    let mut fast = Compressor::new(48000.0, -36.0, settings);
    let mut slow = Compressor::new(48000.0, -36.0, DynamicsConfig { attack_ms: 50.0, release_ms: 500.0, ..settings });
    let mut unity = Compressor::new(48000.0, -36.0, DynamicsConfig { ratio: 1.0, ..settings });
    for _ in 0..960 { fast.run(0.5); slow.run(0.5); assert_eq!(unity.run(0.5), 0.5); }
    assert!(fast.reduction > slow.reduction + 5.0);
    for _ in 0..48000 { fast.run(0.5); slow.run(0.5); }
    assert!((fast.reduction - slow.reduction).abs() < 0.01);
    for _ in 0..4800 { fast.run(0.0); slow.run(0.0); }
    assert!(slow.reduction > fast.reduction + 5.0);
    let mut deesser = DeEsser::new(48000.0, -45.0, 12.0).unwrap();
    for sample in tone(10000.0, 0.5, 48000, 24000) { deesser.run(f64::from(sample)); }
    assert!(deesser.reduction > 4.0);
    deesser.maximum = 0.0;
    assert_eq!(deesser.run(0.25), 0.25);
    assert_eq!(deesser.reduction, 0.0);
}

#[test]
fn live_dynamics_ramps_are_block_invariant_and_finish_on_requested_values() {
    let config = VocalConfig { preset: Preset::Warm, ..VocalConfig::default() };
    let edits = [(VocalParameter::CompressorRatio, 5.0), (VocalParameter::CompressorAttack, 2.0),
        (VocalParameter::CompressorRelease, 250.0), (VocalParameter::DeesserMaximum, 8.0)];
    let input = tone(10000.0, 0.4, 48000, 2400);
    let render = |block: usize| {
        let mut chain = VocalChain::new(48000, config).unwrap();
        for (parameter, value) in edits { chain.set_parameter(parameter, value).unwrap(); }
        let mut left = vec![0.0; input.len()];
        let mut right = left.clone();
        for ((part, l), r) in input.chunks(block).zip(left.chunks_mut(block)).zip(right.chunks_mut(block)) {
            chain.process(part, l, r).unwrap();
        }
        assert_eq!(left, right);
        let p = chain.processing.as_ref().unwrap();
        assert_eq!(p.compressor.ratio, 5.0);
        assert_eq!(p.compressor.attack, coefficient(2.0, 48000.0));
        assert_eq!(p.compressor.release, coefficient(250.0, 48000.0));
        assert_eq!(p.deesser.maximum, 8.0);
        assert!(p.deesser.reduction <= 8.0);
        left
    };
    assert_eq!(render(2400), render(137));
    for (parameter, invalid) in [(VocalParameter::CompressorRatio, 0.9), (VocalParameter::CompressorAttack, 0.0),
        (VocalParameter::CompressorRelease, 501.0), (VocalParameter::DeesserMaximum, 12.1)] {
        let mut config = config;
        assert!(config.set_parameter(parameter, invalid).is_err());
        assert!(config.set_parameter(parameter, f32::NAN).is_err());
    }
}

#[test]
fn live_eq_reaches_the_requested_frequency_response() {
    let config = VocalConfig { preset: Preset::Natural,
        eq: EqConfig { hpf_hz: 40.0, ..EqConfig::default() },
        dynamics: DynamicsConfig { ratio: 1.0, deess_max_db: 0.0, ..DynamicsConfig::default() },
        ..VocalConfig::default() };
    for (parameter, value, hz, minimum, maximum) in [
        (VocalParameter::EqHpf, 200.0, 60.0, 0.05, 0.15),
        (VocalParameter::EqBody, 6.0, 100.0, 1.5, 2.1),
        (VocalParameter::EqPresence, 6.0, 2800.0, 1.9, 2.1),
        (VocalParameter::EqAir, 6.0, 12000.0, 1.5, 2.1),
    ] {
        let input = tone(hz, 0.02, 48000, 48000);
        let mut unchanged = vec![0.0; input.len()];
        let mut right = unchanged.clone();
        VocalChain::new(48000, config).unwrap().process(&input, &mut unchanged, &mut right).unwrap();
        let mut chain = VocalChain::new(48000, config).unwrap();
        let mut changed = vec![0.0; input.len()];
        let at = 12037;
        chain.process(&input[..at], &mut changed[..at], &mut right[..at]).unwrap();
        chain.set_parameter(parameter, value).unwrap();
        chain.process(&input[at..], &mut changed[at..], &mut right[at..]).unwrap();
        assert_eq!(changed, right);
        assert!((changed[at] - unchanged[at]).abs() < 0.0001, "abrupt {:?} edit", parameter);
        let ratio = rms(&changed[36000..]) / rms(&unchanged[36000..]);
        assert!((minimum..maximum).contains(&ratio), "{parameter:?}: {ratio}");
        let mut reference = vec![0.0; input.len()];
        VocalChain::new(48000, chain.config()).unwrap().process(&input, &mut reference, &mut right).unwrap();
        for (actual, expected) in changed[36000..].iter().zip(&reference[36000..]) {
            assert!((actual - expected).abs() < 1e-7, "{parameter:?} did not settle");
        }
    }
}

#[test]
fn live_eq_preserves_history_and_retargets_independently_of_block_size() {
    let edits = [
        [(VocalParameter::EqHpf, 200.0), (VocalParameter::EqBody, 6.0),
            (VocalParameter::EqPresence, -6.0), (VocalParameter::EqAir, 6.0)],
        [(VocalParameter::EqHpf, 40.0), (VocalParameter::EqBody, -6.0),
            (VocalParameter::EqPresence, 6.0), (VocalParameter::EqAir, -6.0)],
        [(VocalParameter::EqHpf, 110.0), (VocalParameter::EqBody, 3.0),
            (VocalParameter::EqPresence, 1.5), (VocalParameter::EqAir, -2.0)],
    ];
    for rate in [32000, 48000, 192000] {
        let config = VocalConfig { preset: Preset::Warm,
            dynamics: DynamicsConfig { ratio: 1.0, deess_max_db: 0.0, ..DynamicsConfig::default() },
            ..VocalConfig::default() };
        // Editing a settled high-pass on DC must not recreate a startup transient.
        let mut equalizer = Equalizer::new(rate, config).unwrap();
        for _ in 0..rate / 2 { equalizer.run(0.25); }
        for targets in edits {
            for (parameter, value) in targets { equalizer.set_parameter(parameter, value).unwrap(); }
            for _ in 0..rate / 40 { assert!(equalizer.run(0.25).abs() < 1e-8); }
        }
        let counts = [rate as usize / 400, rate as usize / 200, rate as usize / 20];
        let frames = counts.iter().sum();
        let mut input = tone(90.0, 0.15, rate, frames);
        for (i, value) in input.iter_mut().enumerate() {
            *value += (0.1 * (TAU * 2800.0 * i as f64 / f64::from(rate)).sin()
                + 0.05 * (TAU * 11000.0 * i as f64 / f64::from(rate)).sin()) as f32;
        }
        let render = |block: usize| {
            let mut chain = VocalChain::new(rate, config).unwrap();
            let mut left = vec![0.0; frames];
            let mut right = left.clone();
            let mut start = 0;
            for (targets, count) in edits.into_iter().zip(counts) {
                let end = start + count;
                for ((part, l), r) in input[start..end].chunks(block)
                    .zip(left[start..end].chunks_mut(block)).zip(right[start..end].chunks_mut(block)) {
                    for (parameter, value) in targets { chain.set_parameter(parameter, value).unwrap(); }
                    chain.process(part, l, r).unwrap();
                }
                start = end;
            }
            assert_eq!(left, right);
            assert!(left.iter().all(|sample| sample.is_finite() && sample.abs() < 1.0));
            left
        };
        let whole = render(frames);
        for block in [1, 17, 256] { assert_eq!(whole, render(block), "rate {rate}, block {block}"); }
    }
    for parameter in [VocalParameter::EqHpf, VocalParameter::EqBody, VocalParameter::EqPresence, VocalParameter::EqAir] {
        assert!(!parameter.available_for(Preset::Off));
        let mut chain = VocalChain::new(48000, VocalConfig { preset: Preset::Natural, ..VocalConfig::default() }).unwrap();
        let before = chain.config();
        for value in [f32::NAN, f32::INFINITY, *parameter.range().start() - 0.1, *parameter.range().end() + 0.1] {
            assert!(chain.set_parameter(parameter, value).is_err());
            assert_eq!(chain.config(), before);
        }
    }
}
