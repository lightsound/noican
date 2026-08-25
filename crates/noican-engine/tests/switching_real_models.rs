//! Switches between two real catalogued models while audio flows.
//!
//! `engine.rs` already tests switching with synthetic stages, and that is what
//! caught an off-by-one-block bug in the priming. It cannot cover what a real
//! model brings: a native sample rate that differs from the host's, so the
//! runner resamples in both directions; a block size that does not divide the
//! device buffer; a genuine algorithmic delay; and inference that takes real
//! time on the very thread that has to prime the incoming stage.
//!
//! Run with `--ignored` after `noican fetch fastenhancer-t gtcrn`. It is not in
//! the default set because it needs weights on disk, and a test that silently
//! skips is worse than one that has to be asked for.

use std::time::{Duration, Instant};

use noican_core::Stage;
use noican_engine::{Engine, EngineConfig};
use noican_models::{ModelStore, build_stage_by_id, catalog};

/// 48 kHz with its own transform, 512-sample blocks.
const AT_HOST_RATE: &str = "fastenhancer-t";

/// 16 kHz driven by our transform, 256-sample blocks — so switching between the
/// two changes the rate, the block size, and the latency at once.
const NEEDS_RESAMPLING: &str = "gtcrn";

const HOST_RATE: u32 = 48_000;
const DEVICE_BLOCK: usize = 256;

/// The weights directory.
///
/// `ModelStore`'s default is `models` relative to the current directory, and
/// cargo runs a test with the crate as its directory rather than the workspace
/// root. So resolve it from the manifest unless the environment says otherwise.
fn store() -> ModelStore {
    if std::env::var_os("NOICAN_MODEL_DIR").is_some() {
        ModelStore::from_environment()
    } else {
        ModelStore::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../models"))
    }
}

fn stage(id: &str) -> Box<dyn Stage> {
    let store = store();
    let model = catalog::find(id).expect("catalogued");
    assert!(
        store.is_present(model),
        "`{id}` is not downloaded. Run `cargo run -p noican-cli -- fetch {id}` first."
    );
    build_stage_by_id(id, &store).expect("the stage should load")
}

fn config() -> EngineConfig {
    EngineConfig {
        sample_rate: HOST_RATE,
        max_device_block: DEVICE_BLOCK,
        ..EngineConfig::default()
    }
}

/// One pumping session's output, split at dropouts.
struct Pumped {
    /// Runs of consecutive complete blocks. A dropout starts a new run, so
    /// sample-to-sample analysis is never fooled by substituted silence.
    runs: Vec<Vec<f32>>,
    dropouts: usize,
}

impl Pumped {
    /// Largest sample-to-sample step anywhere inside a run.
    fn largest_step(&self) -> f32 {
        self.runs
            .iter()
            .flat_map(|run| run.windows(2))
            .fold(0.0f32, |largest, pair| {
                largest.max((pair[1] - pair[0]).abs())
            })
    }

    fn samples(&self) -> usize {
        self.runs.iter().map(Vec::len).sum()
    }

    fn peak(&self) -> f32 {
        self.runs
            .iter()
            .flatten()
            .fold(0.0f32, |largest, sample| largest.max(sample.abs()))
    }
}

/// Drives the callback at the real device rate, feeding a tone.
///
/// A tone rather than silence or noise: silence hides a discontinuity, noise is
/// all discontinuity, and a tone has a known, small step between samples so
/// anything larger stands out.
///
/// Note what this means for the assertions below. A speech-enhancement model
/// correctly treats a pure tone as non-speech and suppresses it — GTCRN takes
/// this one down to a thousandth of its input. So these tests deliberately
/// assert on what the *engine* controls (continuity across a switch, and
/// whether the runner underruns) and not on how much signal survives, which is
/// the model's business and is verified against real speech elsewhere. A
/// suppressing model is if anything the harder case for a click: the level
/// change across the switch is larger.
fn pump(bridge: &mut noican_engine::AudioBridge, blocks: usize, phase: &mut f64) -> Pumped {
    let period = Duration::from_secs_f64(
        f64::from(u32::try_from(DEVICE_BLOCK).unwrap_or(1)) / f64::from(HOST_RATE),
    );
    let step = std::f64::consts::TAU * 440.0 / f64::from(HOST_RATE);

    let mut input = vec![0.0f32; DEVICE_BLOCK];
    let mut output = vec![0.0f32; DEVICE_BLOCK];
    let mut runs: Vec<Vec<f32>> = vec![Vec::new()];
    let mut dropouts = 0;

    let mut next = Instant::now();
    for _ in 0..blocks {
        next += period;
        for sample in &mut input {
            *phase += step;
            #[expect(clippy::cast_possible_truncation, reason = "test fixture")]
            let value = (phase.sin() * 0.3) as f32;
            *sample = value;
        }

        if bridge.process(&input, &mut output) {
            runs.last_mut()
                .expect("a run exists")
                .extend_from_slice(&output);
        } else {
            dropouts += 1;
            if !runs.last().expect("a run exists").is_empty() {
                runs.push(Vec::new());
            }
        }

        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        }
    }
    runs.retain(|run| !run.is_empty());
    Pumped { runs, dropouts }
}

/// Switching between a 48 kHz model and a resampled 16 kHz one, repeatedly,
/// must not click and must not wedge.
#[test]
#[ignore = "needs downloaded weights; run with --ignored"]
fn switching_between_real_models_neither_clicks_nor_wedges() {
    let mut engine = Engine::new(config()).expect("the engine should build");
    let mut bridge = engine
        .start(stage(AT_HOST_RATE))
        .expect("the engine should start");
    let mut phase = 0.0f64;

    // Settle, then measure what a steady state looks like. Calibrating against
    // the engine's own quiet output beats guessing a threshold: it accounts for
    // the tone's slope, the resampler's ripple, and whatever the model does to
    // the waveform.
    pump(&mut bridge, 200, &mut phase);
    let quiet = pump(&mut bridge, 400, &mut phase);
    assert!(
        quiet.samples() > 0,
        "no audio came through before switching"
    );
    assert!(
        quiet.peak() > 0.01,
        "the first model produced near-silence (peak {}), so this test would pass vacuously",
        quiet.peak()
    );
    let baseline = quiet.largest_step();

    let mut worst = 0.0f32;
    let mut produced = 0;
    for round in 0..4 {
        let incoming = if round % 2 == 0 {
            NEEDS_RESAMPLING
        } else {
            AT_HOST_RATE
        };
        engine
            .set_stage(stage(incoming))
            .expect("the switch should be accepted");

        // Long enough to cover the fade out, the incoming model's priming, and
        // the fade back in, with room to spare.
        let switched = pump(&mut bridge, 600, &mut phase);
        produced += switched.samples();
        worst = worst.max(switched.largest_step());
        engine.drain_retired();

        assert!(
            switched.samples() > 0,
            "the engine produced nothing after switching to {incoming}"
        );
    }

    assert!(
        produced > 0,
        "no audio survived any switch, so nothing was measured"
    );
    // An unramped swap between two models steps by the difference between their
    // outputs, which for a 0.3-amplitude tone is a large fraction of the peak.
    // Four times the steady-state step is well below that and well above the
    // ripple a legitimate crossfade adds.
    let allowed = baseline.mul_add(4.0, 0.02);
    assert!(
        worst <= allowed,
        "output stepped by {worst} during a switch; the steady state steps by {baseline}, so \
         anything above {allowed} is a click"
    );

    engine.stop();
}

/// A model that needs resampling has to survive being selected first, not only
/// switched to — the runner primes differently on a fresh start, and an
/// underrun here means its estimate is short by a resampler's worth of samples.
#[test]
#[ignore = "needs downloaded weights; run with --ignored"]
fn a_resampled_model_does_not_underrun_from_a_cold_start() {
    let mut engine = Engine::new(config()).expect("the engine should build");
    let mut bridge = engine
        .start(stage(NEEDS_RESAMPLING))
        .expect("the engine should start");
    let mut phase = 0.0f64;

    pump(&mut bridge, 200, &mut phase);
    let settled = pump(&mut bridge, 600, &mut phase);

    assert!(
        settled.samples() > 0,
        "the engine produced no complete blocks at all"
    );
    // Dropouts during the first blocks are priming, not failure; by now the
    // queues should be full and the thread keeping up. This is the assertion
    // that matters: the runner has to have primed enough for a rate conversion
    // it does not perform at the host rate.
    assert_eq!(
        settled.dropouts, 0,
        "the runner underran {} times after settling, so its priming estimate is short",
        settled.dropouts
    );

    engine.stop();
}
