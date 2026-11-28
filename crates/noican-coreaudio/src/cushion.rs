//! See docs/adr/0003-coreaudio-transport.md#split-cushion-experiment.

use std::ops::RangeInclusive;

use crate::WORKER_BLOCK_SAMPLES;

pub(crate) const VARIABLE: &str = "NOICAN_SPLIT_CUSHION_MS";

pub(crate) const SHIPPED_SAMPLES: usize = 2_400;

const ENGINE_RATE: usize = 48_000;

const MILLISECONDS: RangeInclusive<usize> = 1..=500;

pub(crate) fn requested(value: Option<&str>) -> Result<usize, String> {
    let Some(value) = value else {
        return Ok(SHIPPED_SAMPLES);
    };
    value
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|milliseconds| MILLISECONDS.contains(milliseconds))
        .map(|milliseconds| milliseconds * ENGINE_RATE / 1_000)
        .ok_or_else(|| {
            format!(
                "{VARIABLE} must be a whole number of milliseconds from {} to {}, got {value:?}",
                MILLISECONDS.start(),
                MILLISECONDS.end()
            )
        })
}

pub(crate) fn phase_floor(capture_rate: u32, capture_frames: usize, render_frames: usize) -> usize {
    let capture_rate = usize::try_from(capture_rate).unwrap_or(usize::MAX).max(1);
    let burst = capture_frames
        .saturating_mul(ENGINE_RATE)
        .div_ceil(capture_rate);
    burst
        .saturating_add(render_frames)
        .saturating_add(WORKER_BLOCK_SAMPLES)
}

pub(crate) fn describe(
    cushion: usize,
    capture_rate: u32,
    capture_frames: Option<usize>,
    render_frames: Option<usize>,
) -> String {
    let frames = |frames: Option<usize>| {
        frames.map_or_else(
            || "unreadable".to_owned(),
            |frames| format!("{frames} frames"),
        )
    };
    let floor = capture_frames.zip(render_frames).map_or_else(
        || "unknown".to_owned(),
        |(capture, render)| format!("{} samples", phase_floor(capture_rate, capture, render)),
    );
    format!(
        "cushion {cushion} samples, capture buffer {}, render buffer {}, floor {floor}",
        frames(capture_frames),
        frames(render_frames)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unset_variable_keeps_the_shipped_cushion() {
        assert_eq!(requested(None), Ok(SHIPPED_SAMPLES));
    }

    #[test]
    fn a_millisecond_count_becomes_engine_samples() {
        assert_eq!(requested(Some("35")), Ok(1_680));
        assert_eq!(requested(Some(" 50 ")), Ok(SHIPPED_SAMPLES));
        assert_eq!(requested(Some("1")), Ok(48));
        assert_eq!(requested(Some("500")), Ok(24_000));
    }

    #[test]
    fn anything_else_is_refused_by_name() {
        for value in ["", "0", "501", "-5", "35.5", "35ms", "on"] {
            let error = requested(Some(value)).expect_err(value);
            assert!(error.contains(VARIABLE), "{error}");
            assert!(error.contains(&format!("{value:?}")), "{error}");
        }
    }

    #[test]
    fn the_floor_adds_one_burst_one_render_and_one_block() {
        assert_eq!(phase_floor(48_000, 512, 512), 1_504);
        assert_eq!(phase_floor(48_000, 128, 128), 736);
        assert_eq!(phase_floor(44_100, 512, 512), 1_550);
        assert_eq!(phase_floor(16_000, 512, 512), 2_528);
    }

    #[test]
    fn the_description_names_what_could_not_be_read() {
        assert_eq!(
            describe(2_400, 16_000, Some(512), Some(512)),
            "cushion 2400 samples, capture buffer 512 frames, render buffer 512 frames, \
             floor 2528 samples"
        );
        assert_eq!(
            describe(1_680, 48_000, None, Some(256)),
            "cushion 1680 samples, capture buffer unreadable, render buffer 256 frames, \
             floor unknown"
        );
    }
}
