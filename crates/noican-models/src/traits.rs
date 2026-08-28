//! Picker-facing model characteristics: four 0–5 ratings, a one-line
//! tagline, and a detail string for tooltips.
//!
//! Normal users cannot choose between model code names, so every catalog
//! entry carries a comparable profile. The ratings follow one rule:
//! **every axis points the same way — more filled is better** — so a bar
//! never needs interpreting (latency becomes "responsiveness", CPU cost
//! becomes "efficiency").
//!
//! Basis, so the numbers stay honest:
//! - `responsiveness` is derived from the *measured* engine-reported
//!   algorithmic latency (`Stage::latency_samples`, 2026-08-27:
//!   FastEnhancer 21.3 ms, Hush 22.5 ms, UL-UNAS 34.5 ms, DFN3 40 ms,
//!   DPDFNet 60 ms): 5 ≤ 25 ms, 4 ≤ 35 ms, 3 ≤ 45 ms, 2 ≤ 65 ms.
//! - `efficiency` is derived from parameter count (weight-file size):
//!   5 ≤ 0.25 M, 4 ≤ 1.5 M, 3 ≤ 2.5 M, 2 above.
//! - `voice_quality` is the native rate: 48 kHz native scores 4
//!   (processing always costs some naturalness; only the passthrough
//!   reference scores 5), 16 kHz telephony-band models score 2.
//! - `noise_removal` is editorial, anchored by the 2026-08-27 hardware
//!   run: DPDFNet8 / DeepFilterNet3 / Hush suppressed transient clicks
//!   (trackpad, keyboard); the lighter models let them through.
//!
//! The raw numbers behind the ratings live in each entry's `details`
//! string, surfaced as a tooltip.

/// Picker-facing profile of one catalog entry. All ratings are 0–5 with
/// "more is better"; 0 means the axis does not apply (the passthrough
/// removes nothing).
#[derive(Debug, Clone, Copy)]
pub struct ModelTraits {
    /// How much noise disappears, transient clicks included (editorial,
    /// hardware-anchored).
    pub noise_removal: u8,
    /// How natural the voice stays (native rate: 48 kHz high, 16 kHz low).
    pub voice_quality: u8,
    /// Inverse algorithmic latency (measured; higher = less delay).
    pub responsiveness: u8,
    /// Inverse compute cost (parameter count; higher = lighter).
    pub efficiency: u8,
    /// One-line purpose tag appended to the picker row.
    pub tagline: &'static str,
    /// Raw facts behind the ratings, shown as a tooltip.
    pub details: &'static str,
}

impl ModelTraits {
    /// Compact profile constructor for the [`PROFILES`] table; ratings in
    /// removal / quality / responsiveness / efficiency order.
    const fn rated(ratings: [u8; 4], tagline: &'static str, details: &'static str) -> Self {
        Self {
            noise_removal: ratings[0],
            voice_quality: ratings[1],
            responsiveness: ratings[2],
            efficiency: ratings[3],
            tagline,
            details,
        }
    }

    /// The profile for a catalog id. Unknown ids (which the catalog never
    /// produces) fall back to an unrated profile rather than panicking.
    #[must_use]
    pub fn for_id(id: &str) -> Self {
        PROFILES
            .iter()
            .find(|(profile_id, _)| *profile_id == id)
            .map_or(Self::rated([0, 0, 0, 0], "", ""), |(_, traits)| *traits)
    }
}

/// One profile per catalog entry (see the module docs for the basis).
static PROFILES: &[(&str, ModelTraits)] = &[
    (
        "passthrough",
        ModelTraits::rated(
            [0, 5, 5, 5],
            "no cleanup, for comparison",
            "The unprocessed microphone: zero delay, zero cost, zero cleanup.",
        ),
    ),
    (
        "fastenhancer-t",
        ModelTraits::rated(
            [2, 4, 5, 5],
            "lightest",
            "48 kHz native, ~21 ms delay, ~0.04M parameters.",
        ),
    ),
    (
        "fastenhancer-b",
        ModelTraits::rated(
            [3, 4, 5, 5],
            "balanced default",
            "48 kHz native, ~21 ms delay, ~0.1M parameters.",
        ),
    ),
    (
        "fastenhancer-s",
        ModelTraits::rated(
            [3, 4, 5, 5],
            "light, a bit stronger",
            "48 kHz native, ~21 ms delay, ~0.2M parameters.",
        ),
    ),
    (
        "fastenhancer-m",
        ModelTraits::rated(
            [4, 4, 5, 4],
            "stronger, still quick",
            "48 kHz native, ~21 ms delay, ~0.5M parameters.",
        ),
    ),
    (
        "fastenhancer-l",
        ModelTraits::rated(
            [4, 4, 5, 4],
            "strongest FastEnhancer",
            "48 kHz native, ~21 ms delay, ~1.1M parameters.",
        ),
    ),
    (
        "dpdfnet2",
        ModelTraits::rated(
            [4, 4, 2, 2],
            "strong cleanup",
            "48 kHz native, ~60 ms delay, ~2.6M parameters.",
        ),
    ),
    (
        "dpdfnet8",
        ModelTraits::rated(
            [5, 4, 2, 2],
            "strongest cleanup",
            "48 kHz native, ~60 ms delay, ~3.7M parameters. \
             Suppresses keyboard/trackpad clicks.",
        ),
    ),
    (
        "dfn3",
        ModelTraits::rated(
            [5, 4, 3, 3],
            "strong cleanup, moderate delay",
            "48 kHz native, ~40 ms delay, ~2.1M parameters. \
             Suppresses keyboard/trackpad clicks.",
        ),
    ),
    (
        "ul-unas",
        ModelTraits::rated(
            [3, 2, 4, 5],
            "light, phone-quality voice",
            "16 kHz model (telephony-band voice), ~35 ms delay, \
             ~0.2M parameters.",
        ),
    ),
    (
        "hush",
        ModelTraits::rated(
            [4, 2, 5, 3],
            "mutes background voices",
            "16 kHz model (telephony-band voice), ~23 ms delay, \
             ~2.2M parameters. Also suppresses other people talking \
             nearby and keyboard/trackpad clicks.",
        ),
    ),
    (
        "tse-48k",
        ModelTraits::rated(
            [4, 4, 3, 2],
            "keeps only your voice",
            "48 kHz native. Extracts the enrolled speaker and removes \
             everything else; requires enrollment.",
        ),
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog;

    #[test]
    fn every_catalog_entry_has_a_complete_profile() {
        for entry in catalog() {
            let traits = ModelTraits::for_id(entry.id);
            assert!(!traits.tagline.is_empty(), "{}: missing tagline", entry.id);
            assert!(!traits.details.is_empty(), "{}: missing details", entry.id);
            for (axis, value) in [
                ("noise_removal", traits.noise_removal),
                ("voice_quality", traits.voice_quality),
                ("responsiveness", traits.responsiveness),
                ("efficiency", traits.efficiency),
            ] {
                assert!(value <= 5, "{}: {axis} rating out of range", entry.id);
            }
            // Every rated model has all axes; only the passthrough's
            // noise_removal is legitimately zero.
            if entry.id != "passthrough" {
                assert!(traits.noise_removal > 0, "{}: unrated", entry.id);
            }
            assert!(traits.voice_quality > 0, "{}: unrated", entry.id);
        }
    }

    #[test]
    fn sixteen_kilohertz_models_score_low_voice_quality() {
        for id in ["ul-unas", "hush"] {
            assert_eq!(ModelTraits::for_id(id).voice_quality, 2, "{id}");
        }
        assert_eq!(ModelTraits::for_id("fastenhancer-b").voice_quality, 4);
    }

    #[test]
    fn unknown_ids_fall_back_to_an_unrated_profile() {
        let traits = ModelTraits::for_id("no-such-model");
        assert_eq!(traits.noise_removal, 0);
        assert!(traits.tagline.is_empty());
    }
}
