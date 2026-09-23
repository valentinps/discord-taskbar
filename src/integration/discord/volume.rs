//! Converting between the volume Discord stores and the volume it shows.
//!
//! RPC reports a per-user volume as a raw **amplitude** percentage. Discord's
//! own slider shows a **perceptual** percentage, because hearing is roughly
//! logarithmic and a linear gain control feels wrong — most of the useful
//! range bunches up at the bottom.
//!
//! The two agree only at 0% and 100% and diverge everywhere between, worst in
//! the middle, which is exactly where anyone actually listens. Showing the
//! amplitude raw meant the taskbar and Discord disagreed about the same
//! number: amplitude 50 is 78% on Discord's slider.
//!
//! Discord publishes the idea at <https://github.com/discord/perceptual>, and
//! the boost branch above 100% matches that library exactly — a 6 dB range,
//! confirmed against a live client. The branch below 100% does **not** match
//! the library's linear-in-decibels form; measured against a real client it is
//! a power law, and the readings pin the exponent at 2.8:
//!
//! | Discord shows | amplitude |
//! |---------------|-----------|
//! | 44            | 10        |
//! | 50            | 14        |
//! | 78            | 50        |
//! | 151           | 142       |
//!
//! Both constants are configurable rather than baked in, because they are
//! Discord's to change and this was derived by measurement, not documentation.

/// Exponent of the sub-100% curve. Measured, not documented.
pub const DEFAULT_CURVE: f32 = 2.8;

/// Decibel range Discord spreads 100%..200% over. Matches its own library.
pub const DEFAULT_BOOST_DB: f32 = 6.0;

/// Amplitude and perceptual meet here; both scales call it 100%.
const UNITY: f32 = 100.0;

/// What Discord's slider would read for a stored amplitude.
pub fn amplitude_to_perceptual(amplitude: f32, curve: f32, boost_db: f32) -> f32 {
    if amplitude <= 0.0 {
        return 0.0;
    }
    let ratio = amplitude / UNITY;

    if ratio > 1.0 {
        // Above unity Discord spreads a fixed decibel range over 100%..200%.
        let db = 20.0 * ratio.log10();
        UNITY * (db / boost_db.max(0.001) + 1.0)
    } else {
        UNITY * ratio.powf(1.0 / curve.max(0.001))
    }
}

/// The amplitude to store for a slider reading.
pub fn perceptual_to_amplitude(perceptual: f32, curve: f32, boost_db: f32) -> f32 {
    if perceptual <= 0.0 {
        return 0.0;
    }
    let ratio = perceptual / UNITY;

    if ratio > 1.0 {
        let db = (ratio - 1.0) * boost_db;
        UNITY * 10f32.powf(db / 20.0)
    } else {
        UNITY * ratio.powf(curve.max(0.001))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every pair here was read off a live Discord client, and they are the
    /// whole reason the conversion exists. A change that breaks one of them
    /// has broken the thing this module is for.
    const MEASURED: &[(f32, f32)] = &[
        // (amplitude as RPC reports it, percentage Discord's slider showed)
        (10.0, 44.0),
        (14.0, 50.0),
        (50.0, 78.0),
        (142.0, 151.0),
    ];

    #[test]
    fn matches_what_discord_displayed() {
        for (amplitude, expected) in MEASURED {
            let shown = amplitude_to_perceptual(*amplitude, DEFAULT_CURVE, DEFAULT_BOOST_DB);
            assert!(
                (shown - expected).abs() <= 1.0,
                "amplitude {amplitude} showed {shown:.2}, Discord showed {expected}"
            );
        }
    }

    #[test]
    fn the_two_directions_undo_each_other() {
        for perceptual in [0.0, 1.0, 25.0, 50.0, 99.0, 100.0, 101.0, 150.0, 200.0, 400.0] {
            let amplitude = perceptual_to_amplitude(perceptual, DEFAULT_CURVE, DEFAULT_BOOST_DB);
            let back = amplitude_to_perceptual(amplitude, DEFAULT_CURVE, DEFAULT_BOOST_DB);
            assert!(
                (back - perceptual).abs() < 0.01,
                "{perceptual} -> {amplitude} -> {back}"
            );
        }
    }

    #[test]
    fn unity_and_silence_are_the_same_on_both_scales() {
        // The two anchors that must never drift, or every number shifts.
        assert_eq!(amplitude_to_perceptual(0.0, DEFAULT_CURVE, DEFAULT_BOOST_DB), 0.0);
        assert_eq!(perceptual_to_amplitude(0.0, DEFAULT_CURVE, DEFAULT_BOOST_DB), 0.0);
        assert!(
            (amplitude_to_perceptual(100.0, DEFAULT_CURVE, DEFAULT_BOOST_DB) - 100.0).abs() < 0.001
        );
        assert!(
            (perceptual_to_amplitude(100.0, DEFAULT_CURVE, DEFAULT_BOOST_DB) - 100.0).abs() < 0.001
        );
    }

    #[test]
    fn the_curve_is_continuous_across_unity() {
        // The two branches meet at 100%; a step there would make the slider
        // jump as it crossed.
        let below = amplitude_to_perceptual(99.99, DEFAULT_CURVE, DEFAULT_BOOST_DB);
        let above = amplitude_to_perceptual(100.01, DEFAULT_CURVE, DEFAULT_BOOST_DB);
        assert!((above - below).abs() < 0.05, "{below} then {above}");
    }

    #[test]
    fn perceptual_rises_with_amplitude() {
        // Monotonic, or the slider would run backwards somewhere.
        let mut last = -1.0;
        for step in 0..=400 {
            let shown =
                amplitude_to_perceptual(step as f32, DEFAULT_CURVE, DEFAULT_BOOST_DB);
            assert!(shown >= last, "fell at amplitude {step}");
            last = shown;
        }
    }

    #[test]
    fn silly_constants_do_not_divide_by_zero() {
        assert!(amplitude_to_perceptual(50.0, 0.0, 0.0).is_finite());
        assert!(perceptual_to_amplitude(50.0, 0.0, 0.0).is_finite());
    }
}
