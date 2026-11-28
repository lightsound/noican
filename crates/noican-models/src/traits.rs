//! See docs/adr/0009-model-catalog-fetch.md#model-picker-ratings.

/// See docs/adr/0009-model-catalog-fetch.md#model-picker-ratings.
#[derive(Debug, Clone, Copy)]
pub struct ModelTraits {
    /// See docs/adr/0009-model-catalog-fetch.md#model-picker-ratings.
    pub noise_removal: u8,
    /// See docs/adr/0009-model-catalog-fetch.md#model-picker-ratings.
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

    /// See docs/adr/0009-model-catalog-fetch.md#model-picker-ratings.
    #[must_use]
    pub fn for_id(id: &str) -> Self {
        PROFILES
            .iter()
            .find(|(profile_id, _)| *profile_id == id)
            .map_or(Self::rated([0, 0, 0, 0], "", ""), |(_, traits)| *traits)
    }
}

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
        "dfn3-ll",
        ModelTraits::rated(
            [4, 4, 5, 2],
            "strong cleanup, low delay",
            "48 kHz native, ~20 ms delay, ~9.7M parameters. \
             DeepFilterNet3 variant without lookahead.",
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
        "hush-48k",
        ModelTraits::rated(
            [4, 4, 5, 3],
            "mutes background voices, full-band voice",
            "48 kHz output: Hush's suppression with your voice's upper \
             band kept (gated by Hush itself), ~23 ms delay, ~2.2M \
             parameters. Also suppresses other people talking nearby \
             and keyboard/trackpad clicks.",
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
        assert_eq!(ModelTraits::for_id("dfn3").voice_quality, 4);
        assert_eq!(ModelTraits::for_id("hush-48k").voice_quality, 4);
    }

    #[test]
    fn unknown_ids_fall_back_to_an_unrated_profile() {
        let traits = ModelTraits::for_id("no-such-model");
        assert_eq!(traits.noise_removal, 0);
        assert_eq!(traits.tagline, "");
    }
}
