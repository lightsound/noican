//! Measured algorithmic delay of each catalogued model.
//!
//! A model's delay is not derivable from its configuration: it depends on the
//! lookahead its architecture uses, and none of the published exports declare
//! it. These figures were therefore measured, per model, by pushing a signal
//! through the loaded graph and locating the lag of peak cross-correlation
//! between input and output — the same routine the CLI's `latency` subcommand
//! runs, so any value here can be re-derived on demand:
//!
//! ```text
//! cargo run -p noican-cli -- latency
//! ```
//!
//! The numbers matter for two reasons. They are the honest answer to "how much
//! delay does this model add", which the live latency budget in
//! `docs/tech-research.md` §9 is measured against; and they let the offline
//! comparison line up outputs so that A/B listening is not confounded by a time
//! offset.

/// Delay of a model in samples at its own native rate.
struct Entry {
    id: &'static str,
    samples: usize,
}

/// Delays measured on 2026-08-25 with `noican latency --probe <real speech>`.
///
/// The probe has to be a real recording. Speech-enhancement models correctly
/// treat synthetic tones as non-speech and suppress them, which leaves nothing
/// to correlate; the built-in synthetic probe is a convenience, not a
/// substitute.
///
/// A model absent from this table is reported as having no algorithmic delay,
/// which understates it; add measured entries rather than guessing.
static MEASURED: &[Entry] = &[
    // FastEnhancer fills a whole analysis window before emitting a hop, so its
    // delay is `n_fft - hop`. Every 48 kHz variant uses an n_fft of 1024 with a
    // different hop: 512 for T/S/B, 320 for M, 200 for L.
    Entry {
        id: "fastenhancer-t",
        samples: 512,
    },
    Entry {
        id: "fastenhancer-s",
        samples: 512,
    },
    Entry {
        id: "fastenhancer-b",
        samples: 512,
    },
    Entry {
        id: "fastenhancer-m",
        samples: 704,
    },
    Entry {
        id: "fastenhancer-l",
        samples: 824,
    },
    // Five hops: four inside the model — its reference implementation discards
    // `2 * window_length` of output, which is four hops at these settings — plus
    // one for our own analysis and synthesis round trip.
    Entry {
        id: "dpdfnet2-48k-hr",
        samples: 2_400,
    },
    Entry {
        id: "dpdfnet2-16k",
        samples: 800,
    },
    Entry {
        id: "dpdfnet4-16k",
        samples: 800,
    },
    Entry {
        id: "dpdfnet8-16k",
        samples: 800,
    },
    // Strictly causal per-frame models: the only delay is the one hop our own
    // analysis and synthesis round trip costs.
    Entry {
        id: "gtcrn",
        samples: 256,
    },
    Entry {
        id: "ul-unas",
        samples: 256,
    },
    // The DeepFilterNet family emits the frame `lookahead` behind the newest,
    // plus the hop our own transform costs. DeepFilterNet3 declares two frames
    // of lookahead, Hush none.
    Entry {
        id: "deepfilternet3",
        samples: 1_440,
    },
    Entry {
        id: "hush",
        samples: 160,
    },
];

/// The measured algorithmic delay of `model_id`, in samples at its native rate.
#[must_use]
pub fn of(model_id: &str) -> usize {
    MEASURED
        .iter()
        .find(|entry| entry.id == model_id)
        .map_or(0, |entry| entry.samples)
}

#[cfg(test)]
mod tests {
    use super::{MEASURED, of};
    use crate::catalog;

    #[test]
    fn every_catalogued_model_has_an_entry() {
        for model in catalog::CATALOG {
            assert!(
                MEASURED.iter().any(|entry| entry.id == model.id),
                "{} has no measured latency; run `noican latency` and record it",
                model.id
            );
        }
    }

    #[test]
    fn entries_refer_to_real_models() {
        for entry in MEASURED {
            assert!(
                catalog::find(entry.id).is_some(),
                "latency table mentions unknown model `{}`",
                entry.id
            );
        }
    }

    #[test]
    fn unknown_models_report_no_delay() {
        assert_eq!(of("no-such-model"), 0);
        assert_eq!(of("dpdfnet2-48k-hr"), 2_400);
    }
}
