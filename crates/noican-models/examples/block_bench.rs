//! Per-block processing-time measurement for one model stage.
//!
//! Feeds 10 ms blocks (480 samples at 48 kHz — exactly what the live
//! inference worker hands to the engine) through a stage and reports the
//! block-time distribution against the real-time budget. This is the
//! measurement behind the single-thread decision documented on
//! `onnx::load_streaming_session` and the budget numbers recorded in
//! docs/macos-hardware-test.md.
//!
//! Usage:
//!
//! ```bash
//! NOICAN_MODELS_DIR=/tmp/noican-models \
//!   cargo run --release -p noican-models --example block_bench -- \
//!   fastenhancer-l 60
//! ```

#![expect(
    clippy::print_stdout,
    reason = "a measurement binary reports through stdout"
)]

use std::time::Instant;

use noican_models::{StageOptions, create_stage};

/// One block is 10 ms at the 48 kHz engine rate.
const BLOCK_SAMPLES: usize = 480;
/// The real-time budget per block, mirroring
/// `noican_coreaudio::BLOCK_BUDGET_NS`.
const BUDGET_NS: u128 = 10_000_000;

/// Deterministic 24-bit noise sample in the ±0.25 range.
#[expect(
    clippy::cast_precision_loss,
    reason = "noise amplitude; exactness is irrelevant"
)]
fn noise_sample(lcg: &mut u32) -> f32 {
    *lcg = lcg.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    ((*lcg >> 8) as f32 / 8_388_608.0 - 1.0) * 0.25
}

/// Quantile lookup on sorted nanosecond times.
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "quantile index arithmetic on a small vector"
)]
fn quantile_ns(sorted_ns: &[u128], q: f64) -> u128 {
    sorted_ns[(((sorted_ns.len() - 1) as f64) * q) as usize]
}

/// Nanoseconds as fractional milliseconds for display.
#[expect(
    clippy::cast_precision_loss,
    reason = "display only; nanosecond exactness is irrelevant"
)]
fn as_ms(ns: u128) -> f64 {
    ns as f64 / 1_000_000.0
}

/// Percentage of `part` in `total` for display.
#[expect(clippy::cast_precision_loss, reason = "percentage display")]
fn percent(part: usize, total: usize) -> f64 {
    part as f64 * 100.0 / total as f64
}

fn main() {
    let mut args = std::env::args().skip(1);
    let model = args.next().unwrap_or_else(|| "fastenhancer-l".to_owned());
    // Clamped to one second so a `0` argument cannot reach the quantile
    // lookup with an empty vector.
    let seconds: usize = args
        .next()
        .map_or(60, |s| s.parse().expect("seconds must be an integer"))
        .max(1);
    let models_dir = std::env::var_os("NOICAN_MODELS_DIR")
        .map_or_else(|| "models".into(), std::path::PathBuf::from);

    let mut stage =
        create_stage(&model, &models_dir, &StageOptions::default()).expect("stage should load");

    // Deterministic noise input: block cost is content-independent for
    // these architectures, and noise avoids shipping an audio fixture.
    let mut lcg: u32 = 0x1234_5678;
    let blocks = seconds * 100;
    let mut input = vec![0.0_f32; BLOCK_SAMPLES];
    let mut output = vec![0.0_f32; BLOCK_SAMPLES];
    let mut times_ns = Vec::with_capacity(blocks);
    for _ in 0..blocks {
        for sample in &mut input {
            *sample = noise_sample(&mut lcg);
        }
        let start = Instant::now();
        stage
            .process_block(&input, &mut output)
            .expect("block should process");
        times_ns.push(start.elapsed().as_nanos());
    }

    times_ns.sort_unstable();
    let over = times_ns.iter().filter(|&&t| t > BUDGET_NS).count();
    println!(
        "{model}: {blocks} blocks, over-budget {over} ({:.1}%), \
         p50 {:.2} ms, p95 {:.2} ms, p99 {:.2} ms, max {:.2} ms",
        percent(over, blocks),
        as_ms(quantile_ns(&times_ns, 0.50)),
        as_ms(quantile_ns(&times_ns, 0.95)),
        as_ms(quantile_ns(&times_ns, 0.99)),
        as_ms(quantile_ns(&times_ns, 1.0)),
    );
}
