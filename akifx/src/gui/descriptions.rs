//! Human-readable module descriptions and parameter tooltips for the AkiFX GUI.
//!
//! [`module_intro`] returns a one-liner per module id_prefix (shown below the
//! LED in the rack). [`param_tip`] returns per-parameter tooltips for the
//! parameter panel hover state.

/// One-liner description for a module's id_prefix.
///
/// Returns `None` for unknown prefixes. Producer-facing English with key CN
/// terms in parentheses where clarifying.
pub fn module_intro(id_prefix: &str) -> Option<&'static str> {
    match id_prefix {
        "sine_gen" => Some(
            "Test-tone generator (\u{6d4b}\u{8bd5}\u{4fe1}\u{53f7}\u{53d1}\u{751f}\u{5668}): \
             plays a sine wave via MIDI note or fallback frequency.",
        ),
        "soft_vacuum" => Some(
            "Tube saturation (\u{771f}\u{7a7a}\u{7ba1}\u{9971}\u{548c}): \
             Airwindows Hard Vacuum port — warm tube-style drive with oversampling.",
        ),
        "crisp" => Some(
            "Transient enhancer (\u{77ac}\u{6001}\u{6fc0}\u{52b3}\u{5668}): \
             adds high-frequency crispness via ring modulation and filtered noise.",
        ),
        "spectral_gate" => Some(
            "Spectral noise gate (\u{9891}\u{8c31}\u{566a}\u{58f0}\u{95e8}): \
             frequency-dependent gate that cleans noise per-bin.",
        ),
        "frequency_shift" => Some(
            "Bin-wise frequency shifter (\u{9891}\u{7387}\u{642c}\u{79fb}): \
             shifts every FFT bin by a fixed offset for metallic ring textures.",
        ),
        "crossover" => Some(
            "Multiband splitter (\u{5206}\u{9891}\u{5668}): \
             Linkwitz-Riley crossover for 2\u{2013}5 band processing.",
        ),
        "diopser" => Some(
            "Allpass phaser (\u{5168}\u{901a}\u{76f8}\u{4f4d}\u{6ee4}\u{6ce2}): \
             cascaded allpass filters with spread for swirling phase effects.",
        ),
        "spectral_compressor" => Some(
            "Per-bin compressor (\u{9891}\u{8c31}\u{538b}\u{7f29}\u{5668}): \
             up/down spectral compressor — the 16384-band OTT.",
        ),
        "buffr_glitch" => Some(
            "Buffer glitch (\u{7f13}\u{51b2}\u{6545}\u{969c}): \
             MIDI-triggered buffer stutter with pitch-shifted repeats.",
        ),
        "gain" => Some(
            "Master gain (\u{4e3b}\u{589e}\u{76ca}): \
             final gain stage before the safety limiter.",
        ),
        "safety_limiter" => Some(
            "Safety brickwall limiter (\u{5b89}\u{5168}\u{9650}\u{5236}\u{5668}): \
             hard ceiling that plays SOS morse code when overloaded.",
        ),
        _ => None,
    }
}

/// Per-parameter tooltip. Returns `(id_prefix, param_lower) -> Option<&str>`.
///
/// `param_lower` is the lowercase parameter name as yielded by `param_map()`.
/// Content quality: explains WHAT IT DOES + MUSICAL USE CASE in \u{2264}2 short sentences.
pub fn param_tip(id_prefix: &str, param_lower: &str) -> Option<&'static str> {
    match (id_prefix, param_lower) {
        // ── spectral_compressor ────────────────────────────────────────
        ("spectral_compressor", "output_gain") => Some(
            "Output level after compression. Use to compensate for gain changes from compression.",
        ),
        ("spectral_compressor", "dry_wet_ratio") => Some(
            "Blend between compressed (1.0) and unprocessed (0.0) signal. Parallel compression at <1.0.",
        ),
        ("spectral_compressor", "window_size_order") => Some(
            "FFT window size as a power of 2. Larger = finer frequency resolution, more latency.",
        ),
        ("spectral_compressor", "overlap_times_order") => Some(
            "Overlap factor for the STFT. Higher = smoother output, more CPU.",
        ),
        ("spectral_compressor", "compressor_attack_ms") => Some(
            "Attack time in ms. Faster = snappier transient response.",
        ),
        ("spectral_compressor", "compressor_release_ms") => Some(
            "Release time in ms. Longer = smoother sustain, less pumping.",
        ),
        // compressor_bank nested params
        ("spectral_compressor", "threshold_db") => Some(
            "Compression threshold in dB. Signals above this level get compressed per-bin.",
        ),
        ("spectral_compressor", "center_frequency") => Some(
            "Center frequency for the threshold curve. Shapes which bands are affected most.",
        ),
        ("spectral_compressor", "curve_slope") => Some(
            "Slope of the threshold curve. Positive tilts toward highs, negative toward lows.",
        ),
        ("spectral_compressor", "curve_curve") => Some(
            "Curvature of the threshold envelope. 0 = linear, higher = more bowed.",
        ),
        ("spectral_compressor", "mode") => Some(
            "Internal = auto-threshold from input. External = sidechain-driven threshold.",
        ),
        ("spectral_compressor", "sc_channel_link") => Some(
            "Sidechain channel link. 1.0 = fully linked L/R, 0.0 = independent per-channel.",
        ),
        ("spectral_compressor", "threshold_offset_db") => Some(
            "Per-compressor threshold offset in dB. Fine-tune per-band sensitivity.",
        ),
        ("spectral_compressor", "ratio") => Some(
            "Compression ratio. Higher = more aggressive gain reduction per band.",
        ),
        ("spectral_compressor", "high_freq_ratio_rolloff") => Some(
            "Reduces ratio at high frequencies. Preserves air while compressing mids.",
        ),
        ("spectral_compressor", "knee_width_db") => Some(
            "Soft-knee width in dB. Larger = gentler onset of compression around threshold.",
        ),
        // ── crossover ──────────────────────────────────────────────────
        ("crossover", "num_bands") => Some(
            "Number of frequency bands (2\u{2013}5). More bands = finer multiband control.",
        ),
        ("crossover", "crossover_1_freq") => Some("Crossover point 1 in Hz. Splits low from mid bands."),
        ("crossover", "crossover_2_freq") => Some("Crossover point 2 in Hz. Splits mid bands."),
        ("crossover", "crossover_3_freq") => Some("Crossover point 3 in Hz. Splits upper-mid bands."),
        ("crossover", "crossover_4_freq") => Some("Crossover point 4 in Hz. Splits high band."),
        ("crossover", "crossover_type") => Some(
            "Filter topology. Linkwitz-Riley 24/48 dB/oct. Affects phase coherence at split points.",
        ),
        ("crossover", "band_1_gain") => Some("Gain for band 1 (lowest). Boost or cut the bass region."),
        ("crossover", "band_2_gain") => Some("Gain for band 2. Adjust the low-mid level."),
        ("crossover", "band_3_gain") => Some("Gain for band 3. Adjust the mid level."),
        ("crossover", "band_4_gain") => Some("Gain for band 4. Adjust the upper-mid level."),
        ("crossover", "band_5_gain") => Some("Gain for band 5 (highest). Adjust the treble region."),
        // ── diopser ────────────────────────────────────────────────────
        ("diopser", "filter_stages") => Some(
            "Number of allpass filter stages. More = deeper phasing swirl, more CPU.",
        ),
        ("diopser", "filter_frequency") => Some(
            "Center frequency of the allpass cascade. Sweep for classic phaser sweeps.",
        ),
        ("diopser", "filter_resonance") => Some(
            "Feedback resonance. Higher = more pronounced notches, self-oscillation territory.",
        ),
        ("diopser", "filter_spread_octaves") => Some(
            "Frequency spread between stages in octaves. Wider = more dramatic phasing.",
        ),
        ("diopser", "filter_spread_style") => Some(
            "How stage frequencies are distributed. Octaves = exponential, linear = even spacing.",
        ),
        // ── soft_vacuum ────────────────────────────────────────────────
        ("soft_vacuum", "drive") => Some(
            "Input gain into the tube saturation stage. Higher = more harmonic distortion.",
        ),
        ("soft_vacuum", "warmth") => Some(
            "Tone warmth bias. Higher = softer high-frequency rolloff.",
        ),
        ("soft_vacuum", "aura") => Some(
            "Stereo width of the saturation effect. Higher = wider stereo image.",
        ),
        ("soft_vacuum", "output_gain") => Some(
            "Output level after saturation. Compensate for level changes from drive.",
        ),
        ("soft_vacuum", "dry_wet_ratio") => Some(
            "Blend between saturated (1.0) and clean (0.0). Parallel saturation at <1.0.",
        ),
        ("soft_vacuum", "oversampling_factor") => Some(
            "Oversampling ratio. Higher = less aliasing, more CPU. 16x recommended for heavy drive.",
        ),
        // ── buffr_glitch ───────────────────────────────────────────────
        ("buffr_glitch", "dry_level") => Some(
            "Level of the unprocessed (dry) signal. Blends with the glitch output.",
        ),
        ("buffr_glitch", "octave_shift") => Some(
            "Pitch shift of the captured buffer in semitones (12 = one octave).",
        ),
        ("buffr_glitch", "attack_ms") => Some(
            "Fade-in time for each glitch slice in ms. Shorter = more abrupt cuts.",
        ),
        ("buffr_glitch", "release_ms") => Some(
            "Fade-out time for each glitch slice in ms. Longer = smoother tails.",
        ),
        ("buffr_glitch", "crossfade_ms") => Some(
            "Crossfade between consecutive glitch slices in ms. Prevents clicks.",
        ),
        // ── crisp ──────────────────────────────────────────────────────
        ("crisp", "amount") => Some(
            "Amount of transient enhancement. Higher = more pronounced attack crispness.",
        ),
        ("crisp", "mode") => Some(
            "Crispy = ring-mod based, Crunchy = heavier harmonic saturation.",
        ),
        ("crisp", "stereo_mode") => Some(
            "Stereo = independent L/R processing, Mid/Side = process mid only.",
        ),
        ("crisp", "rm_input_lpf_freq") => Some(
            "Low-pass filter on ring mod input. Lower = darker modulation character.",
        ),
        ("crisp", "rm_input_lpf_q") => Some(
            "Resonance of the ring mod input filter. Higher = more nasal quality.",
        ),
        ("crisp", "noise_hpf_freq") => Some(
            "High-pass filter on the noise generator. Higher = thinner noise character.",
        ),
        ("crisp", "noise_hpf_q") => Some("Resonance of the noise HPF. Adds bite at the cutoff."),
        ("crisp", "noise_lpf_freq") => Some(
            "Low-pass filter on the noise. Lower = darker, warmer noise texture.",
        ),
        ("crisp", "noise_lpf_q") => Some("Resonance of the noise LPF. Adds color at cutoff."),
        ("crisp", "output_gain") => Some("Output level after crisp processing. Compensate for level changes."),
        // ── spectral_gate ──────────────────────────────────────────────
        ("spectral_gate", "cutoff") => Some(
            "Gate threshold in dB. Bins below this level are attenuated.",
        ),
        ("spectral_gate", "balance") => Some(
            "L/R balance of the gate. 0 = stereo linked, +/- = bias toward one channel.",
        ),
        ("spectral_gate", "tilt") => Some(
            "Spectral tilt of the gate threshold. Positive = open highs more, negative = open lows.",
        ),
        // ── frequency_shift ────────────────────────────────────────────
        ("frequency_shift", "shift") => Some(
            "Bin shift amount. Non-integer values create inharmonic metallic textures.",
        ),
        ("frequency_shift", "scale") => Some(
            "Scaling factor for the shift. 1.0 = direct bin offset, higher = exponential spread.",
        ),
        // ── sine_gen ───────────────────────────────────────────────────
        ("sine_gen", "level_db") => Some(
            "Output level in dB. Controls the sine generator volume.",
        ),
        ("sine_gen", "fallback_frequency") => Some(
            "Frequency in Hz when no MIDI note is active. Use for drone tones.",
        ),
        // ── gain ───────────────────────────────────────────────────────
        ("gain", "gain") => Some(
            "Master output gain in dB. Final level before the safety limiter.",
        ),
        // ── safety_limiter ─────────────────────────────────────────────
        ("safety_limiter", "threshold") => Some(
            "Ceiling threshold in dBFS. Hard brickwall — nothing passes above this level.",
        ),
        // ── wildcard ───────────────────────────────────────────────────
        _ => None,
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// All 11 known module prefixes must return Some.
    #[test]
    fn module_intro_covers_all_11_prefixes() {
        let prefixes = [
            "sine_gen", "soft_vacuum", "crisp", "spectral_gate",
            "frequency_shift", "spectral_compressor", "crossover", "diopser",
            "buffr_glitch", "gain", "safety_limiter",
        ];
        for p in prefixes {
            let intro = module_intro(p)
                .unwrap_or_else(|| panic!("module_intro must return Some for {p}"));
            assert!(
                !intro.is_empty(),
                "module_intro for {p} must be non-empty"
            );
        }
    }

    /// Unknown prefix returns None.
    #[test]
    fn module_intro_unknown_returns_none() {
        assert!(module_intro("nonexistent_module").is_none());
        assert!(module_intro("").is_none());
    }

    /// Sampled complex param_tip pairs return Some.
    #[test]
    fn param_tip_returns_some_for_complex_modules() {
        let cases = [
            ("spectral_compressor", "threshold_db"),
            ("spectral_compressor", "ratio"),
            ("spectral_compressor", "mode"),
            ("crossover", "num_bands"),
            ("crossover", "band_1_gain"),
            ("diopser", "filter_stages"),
            ("diopser", "filter_resonance"),
            ("soft_vacuum", "drive"),
            ("soft_vacuum", "oversampling_factor"),
            ("buffr_glitch", "attack_ms"),
            ("buffr_glitch", "octave_shift"),
            ("crisp", "amount"),
            ("crisp", "mode"),
            ("spectral_gate", "cutoff"),
            ("frequency_shift", "shift"),
            ("gain", "gain"),
            ("safety_limiter", "threshold"),
            ("sine_gen", "level_db"),
        ];
        for (prefix, param) in cases {
            let tip = param_tip(prefix, param)
                .unwrap_or_else(|| panic!("param_tip must return Some for ({prefix}, {param})"));
            assert!(
                !tip.is_empty(),
                "param_tip for ({prefix}, {param}) must be non-empty"
            );
        }
    }

    /// Unknown (prefix, param) pair returns None.
    #[test]
    fn param_tip_unknown_returns_none() {
        assert!(param_tip("gain", "nonexistent_param").is_none());
        assert!(param_tip("unknown_module", "gain").is_none());
        assert!(param_tip("", "").is_none());
    }
}
