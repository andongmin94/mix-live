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
        let mut output = input;
        let meters = chain.process(&mut output).unwrap();
        assert_eq!(input, output);
        assert_eq!(meters.compression_db, 0.0);
        assert_eq!(meters.deessing_db, 0.0);
        assert_eq!(meters.output_peak, 1.5);
    }
    let mut chain = VocalChain::new(8000, VocalConfig { input_gain_db: -6.0, ..VocalConfig::default() }).unwrap();
    let mut output = input;
    chain.process(&mut output).unwrap();
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
    let mut compressor = Compressor::new(48000.0, -24.0, 3.0);
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
        let mut whole = input.clone();
        VocalChain::new(48000, config).unwrap().process(&mut whole).unwrap();
        for size in [1, 17, 256, 1024] {
            let mut blocks = input.clone();
            let mut chain = VocalChain::new(48000, config).unwrap();
            for part in blocks.chunks_mut(size) { chain.process(part).unwrap(); }
            assert_eq!(whole, blocks, "preset {preset}, block {size}");
        }
    }
}

#[test]
fn invalid_audio_silences_the_whole_block_and_resets_state() {
    let config = VocalConfig { preset: Preset::Natural, ..VocalConfig::default() };
    let mut chain = VocalChain::new(48000, config).unwrap();
    chain.process(&mut [0.2; 128]).unwrap();
    for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut block = [0.1, invalid, 0.2];
        assert_eq!(chain.process(&mut block).unwrap_err(), DspError::NonFiniteAudio);
        assert_eq!(block, [0.0; 3]);
        let mut silence = [0.0; 256];
        chain.process(&mut silence).unwrap();
        assert_eq!(silence, [0.0; 256]);
    }
    assert_eq!(chain.process(&mut []).unwrap().output_peak, 0.0);
}
