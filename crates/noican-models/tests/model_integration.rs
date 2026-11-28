//! See docs/adr/0009-model-catalog-fetch.md#model-integration-tests-opt-in.

#![expect(
    clippy::print_stderr,
    reason = "test skip notices must be visible in the test log"
)]

use std::path::PathBuf;

use noican_core::ENGINE_SAMPLE_RATE;
use noican_models::{ModelSpec, create_stage};

fn models_dir() -> PathBuf {
    std::env::var_os("NOICAN_MODELS_DIR").map_or_else(
        || {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join("models")
        },
        PathBuf::from,
    )
}

fn test_signal() -> Vec<f32> {
    let mut rng_state = 0x1234_5678_u32;
    let mut noise = move || {
        rng_state ^= rng_state << 13;
        rng_state ^= rng_state >> 17;
        rng_state ^= rng_state << 5;
        #[expect(
            clippy::cast_precision_loss,
            reason = "uniform noise precision is irrelevant here"
        )]
        let uniform = rng_state as f32 / u32::MAX as f32;
        uniform - 0.5
    };
    (0..ENGINE_SAMPLE_RATE as usize)
        .map(|n| {
            #[expect(
                clippy::cast_precision_loss,
                reason = "sample indices fit f32 for a one-second signal"
            )]
            let t = n as f32 / 48_000.0;
            let tone = |freq: f32| (2.0 * std::f32::consts::PI * freq * t).sin();
            let voiced = 0.25f32.mul_add(tone(880.0), 0.5f32.mul_add(tone(440.0), tone(220.0)));
            let envelope = 0.5f32.mul_add(tone(3.0), 0.5);
            0.05f32.mul_add(noise(), 0.2 * voiced * envelope)
        })
        .collect()
}

fn run_model(id: &str) {
    let dir = models_dir();
    if let Some(spec) = ModelSpec::find(id)
        && !noican_models::fetch::is_fetched(&dir, spec)
    {
        eprintln!("[skip] {id}: weights not fetched under {}", dir.display());
        return;
    }
    let mut stage = create_stage(id, &dir).expect("stage should load");
    let input = test_signal();
    let mut output = vec![0.0_f32; input.len()];
    for (i, o) in input.chunks(480).zip(output.chunks_mut(480)) {
        stage
            .process_block(i, o)
            .expect("processing should succeed");
    }
    assert!(
        output.iter().all(|s| s.is_finite()),
        "{id}: non-finite output"
    );
    let latency = stage.latency_samples();
    assert!(
        latency < input.len(),
        "{id}: latency {latency} exceeds one second"
    );
    let peak = output.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
    assert!(peak < 10.0, "{id}: implausible output peak {peak}");
}

macro_rules! model_test {
    ($name:ident, $id:literal) => {
        #[test]
        #[ignore = "requires downloaded model weights (run: noican fetch)"]
        fn $name() {
            run_model($id);
        }
    };
}

model_test!(dpdfnet2_runs, "dpdfnet2");
model_test!(dpdfnet8_runs, "dpdfnet8");
model_test!(dfn3_ll_runs, "dfn3-ll");
model_test!(ulunas_runs, "ul-unas");
model_test!(hush_runs, "hush");
model_test!(hush_48k_runs, "hush-48k");

#[test]
#[ignore = "slow (tract plan build); run with --ignored"]
fn dfn3_runs() {
    run_model("dfn3");
}

#[test]
#[ignore = "requires downloaded model weights (run: noican fetch dfn3-ll)"]
fn dfn3_ll_latency_is_twenty_ms() {
    let dir = models_dir();
    let spec = ModelSpec::find("dfn3-ll").expect("registered");
    if !noican_models::fetch::is_fetched(&dir, spec) {
        eprintln!(
            "[skip] dfn3-ll: weights not fetched under {}",
            dir.display()
        );
        return;
    }
    let stage = create_stage("dfn3-ll", &dir).expect("stage should load");
    assert_eq!(stage.latency_samples(), 960);
}
