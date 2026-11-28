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
    pub(crate) const PASSTHROUGH: Self = Self::rated(
        [0, 5, 5, 5],
        "no cleanup, for comparison",
        "The unprocessed microphone: zero delay, zero cost, zero cleanup.",
    );

    pub(crate) const fn rated(
        ratings: [u8; 4],
        tagline: &'static str,
        details: &'static str,
    ) -> Self {
        Self {
            noise_removal: ratings[0],
            voice_quality: ratings[1],
            responsiveness: ratings[2],
            efficiency: ratings[3],
            tagline,
            details,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog;

    fn traits_of(id: &str) -> ModelTraits {
        catalog()
            .find(|entry| entry.id == id)
            .expect("in the catalog")
            .traits
    }

    #[test]
    fn every_catalog_entry_has_a_complete_profile() {
        for entry in catalog() {
            let traits = entry.traits;
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
            assert_eq!(traits_of(id).voice_quality, 2, "{id}");
        }
        assert_eq!(traits_of("dfn3").voice_quality, 4);
        assert_eq!(traits_of("hush-48k").voice_quality, 4);
    }
}
