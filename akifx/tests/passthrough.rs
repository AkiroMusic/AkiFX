use akifx::apply_gain;

#[test]
fn unity_gain_leaves_samples_unchanged() {
    let mut samples = vec![0.5, -0.5, 1.0, -1.0, 0.0, 0.25, -0.75];
    let expected = samples.clone();

    apply_gain(1.0, &mut samples);

    for (actual, expected) in samples.iter().zip(expected.iter()) {
        assert!((actual - expected).abs() < 1e-6, "Expected {expected}, got {actual}");
    }
}

#[test]
fn negative_six_db_halves_amplitude() {
    // -6 dB ≈ 0.501187... linear gain
    let gain = nih_plug::util::db_to_gain(-6.0);
    let mut samples = vec![1.0, 1.0, 1.0];
    let expected = gain;

    apply_gain(gain, &mut samples);

    for sample in &samples {
        assert!(
            (sample - expected).abs() < 1e-4,
            "Expected ~{expected}, got {sample}"
        );
    }
}

#[test]
fn silence_input_stays_silent() {
    let mut samples = vec![0.0; 128];

    apply_gain(0.0, &mut samples);

    for sample in &samples {
        assert_eq!(*sample, 0.0);
    }
}

#[test]
fn negative_gain_inverts_phase() {
    let mut samples = vec![1.0, 0.5, -0.5];
    let expected = vec![-1.0, -0.5, 0.5];

    apply_gain(-1.0, &mut samples);

    for (actual, exp) in samples.iter().zip(expected.iter()) {
        assert!((actual - exp).abs() < 1e-6, "Expected {exp}, got {actual}");
    }
}
