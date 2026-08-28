//! Self-monitor acoustic echo cancellation (AEC Stage 1).
//!
//! When the Preview monitor plays through speakers, the microphone picks
//! the played signal back up. A general AEC needs a Core Audio process
//! tap to capture other applications' playback as its reference
//! (docs/tech-research.md §7.2 — extra TCC prompt, macOS 26 tap
//! instability). Preview is the one case that needs none of that: the
//! sound leaving the speaker is the signal *this engine generated one
//! branch earlier*, so the far-end reference already exists in process —
//! it is exactly the block the worker tees into the monitor ring.
//! [`SelfMonitorAec`] subtracts that self-echo from the microphone
//! before the noise-suppression model sees it, which is what lets
//! Preview run on the built-in speakers.
//!
//! Placement and alignment: the canceller runs on the inference worker
//! at the top of the per-block step — *upstream* of the model, the
//! dry/wet mixer's dry tap, the level meters, and the monitor tee — so
//! every consumer hears the same echo-cancelled input. Because the
//! mixer taps its dry signal after this stage, the canceller's fixed
//! capture-path delay (measured ≈ 9 ms, see
//! `dry_wet_alignment_survives_the_aec`) shifts the dry and wet paths
//! equally and the strength control's alignment (`noican_core::mix`) is
//! preserved by construction.
//!
//! Gating (all lock-free, docs/tech-research.md §9): the canceller
//! engages only while the shared [`MonitorState`] cell reads
//! [`MonitorState::Playing`] — the same single atomic the
//! [`crate::monitor::MonitorTee`] gates on — so the reference always
//! matches what the monitor actually plays. While the monitor is off or
//! the feedback killswitch has tripped (the tee renders silence), the
//! canceller is a complete bypass: no processing, no reference, no cost
//! beyond one atomic load per block. A second atomic — the monitor
//! *session generation*, bumped by every control-plane enable — resets
//! the canceller whenever the monitor is toggled or moves to another
//! output device, because the echo path it learned no longer exists.
//!
//! Engine selection (task: evaluate the `aec3` crate first):
//! - **`aec3` 0.3.2** (`RubyBit/aec3-rs`, MIT OR BSD-3-Clause — the
//!   license question left open in docs/tech-research.md §11 is
//!   resolved, and it passes the cargo-deny allow list): rejected. Its
//!   supported surface, the graph/`LinearPipeline` API, is built on
//!   `Rc` packets — `!Send`, so it cannot be constructed on the control
//!   plane and moved into the inference worker (this codebase's
//!   transport pattern), and it heap-allocates a packet per frame by
//!   design. Its README declares the API a work in progress ("still
//!   validating"). In a like-for-like probe (48 kHz mono, 10 ms frames,
//!   60 ms echo path) it converged far slower than sonora: ERLE
//!   −1.7 dB vs 42 dB at 3–5 s, 20 dB vs 53 dB at 8–10 s.
//! - **`sonora` 0.2** (`dignifiedquire/sonora`, BSD-3-Clause): adopted.
//!   A faithful pure-Rust port of WebRTC's audio processing module with
//!   AEC3's built-in render-delay estimation, a swap-queue hot path that
//!   avoids per-frame allocation in steady state, and a documented
//!   `Send + Sync` [`AudioProcessing`] handle. Probe results (x86-64,
//!   release): ERLE 57 dB at a 60 ms echo path, 56 dB at 100 ms, 30 dB
//!   at 150 ms; double-talk preserves the near speech within ~1 dB;
//!   the self-monitor scenario (far end = the user's own processed
//!   voice) preserves the live voice within ~2 dB; real-time factor
//!   ≈ 0.007.
//! - Alternatives not taken: a hand-rolled partitioned NLMS in
//!   `noican-core` (no delay estimator — it would have to cover the
//!   whole 40–150 ms echo-path range with filter length alone, and
//!   would be the least battle-tested piece of the audio path) and
//!   `speexdsp` bindings (C build dependency, weaker double-talk
//!   handling, requires a roughly known delay).

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};

use sonora::config::EchoCanceller;
use sonora::{AudioProcessing, Config, StreamConfig};

use crate::WORKER_BLOCK_SAMPLES;
use crate::monitor::MonitorState;

/// Mono stream at the 48 kHz engine rate: the shape of every frame the
/// canceller exchanges with sonora (10 ms, [`WORKER_BLOCK_SAMPLES`]).
const STREAM: StreamConfig = StreamConfig::new(48_000, 1);

/// Echo canceller for the Preview self-monitor, owned by the inference
/// worker (never the audio I/O callback).
///
/// See the module docs for placement, gating, and the engine-selection
/// rationale. Constructed on the control plane (allocates); once running
/// it performs no locking — its two gates are plain atomic loads — and
/// sonora's steady-state processing works in buffers preallocated at
/// (re)initialization time.
#[derive(Debug)]
pub struct SelfMonitorAec {
    apm: AudioProcessing,
    /// The shared [`MonitorState`] cell (the same one the monitor tee
    /// gates on): the canceller engages only while it reads `Playing`.
    state: Arc<AtomicI32>,
    /// Monitor session generation, bumped by every control-plane enable.
    /// A bump means the echo path may have changed (toggle, new output
    /// device), so the canceller resets when it observes one.
    generation: Arc<AtomicU64>,
    /// Generation the canceller is currently engaged under, or `None`
    /// while bypassed. Re-engaging (or observing a bump) resets sonora's
    /// filter and delay state.
    engaged: Option<u64>,
    /// Preallocated landing buffer for sonora's output frames.
    scratch: Box<[f32; WORKER_BLOCK_SAMPLES]>,
}

impl SelfMonitorAec {
    /// Creates a canceller around the shared monitor-state cell and the
    /// monitor session generation. Allocates (control plane only).
    #[must_use]
    pub fn new(state: Arc<AtomicI32>, generation: Arc<AtomicU64>) -> Self {
        let apm = AudioProcessing::builder()
            .config(Config {
                // Echo cancellation only: noise suppression is this
                // product's own model downstream, and AGC would fight
                // the strength control. Sonora enforces its high-pass
                // filter alongside AEC3 (a WebRTC invariant the
                // canceller's adaptation relies on); it engages only
                // while the canceller does.
                echo_canceller: Some(EchoCanceller::default()),
                ..Default::default()
            })
            .capture_config(STREAM)
            .render_config(STREAM)
            .build();
        Self {
            apm,
            state,
            generation,
            engaged: None,
            scratch: Box::new([0.0; WORKER_BLOCK_SAMPLES]),
        }
    }

    /// Cancels the self-monitor echo out of one microphone block, in
    /// place, when the preview monitor is playing; otherwise a complete
    /// bypass (the block is untouched and no state advances).
    ///
    /// Called by the inference worker at the top of every block, before
    /// the engine (model, dry/wet mix). Engaging — the first playing
    /// block after a bypass, or a generation bump — resets the
    /// canceller's filter and delay state; the reset reinitializes
    /// sonora's pipeline, which allocates (allowed on the worker; only
    /// the audio I/O callback is allocation-free —
    /// docs/tech-research.md §9).
    pub fn process_capture(&mut self, block: &mut [f32]) {
        if MonitorState::from_raw(self.state.load(Ordering::Acquire)) != MonitorState::Playing {
            self.engaged = None;
            return;
        }
        let generation = self.generation.load(Ordering::Acquire);
        if self.engaged != Some(generation) {
            // The echo path is new (fresh preview, re-arm, or another
            // output device): forget the learned filter and delay.
            self.apm.initialize(STREAM, STREAM, STREAM, STREAM);
            self.engaged = Some(generation);
        }
        if block.len() != self.scratch.len() {
            // The worker always feeds 10 ms blocks; anything else would
            // be a caller bug. Fail open (bypass) rather than poison
            // the meeting-facing path.
            return;
        }
        if self
            .apm
            .process_capture_f32(&[&*block], &mut [&mut self.scratch[..]])
            .is_ok()
        {
            block.copy_from_slice(&self.scratch[..]);
        }
    }

    /// Feeds the far-end reference: the processed block the worker just
    /// teed into the monitor ring. `teed` is the tee's own verdict
    /// ([`crate::monitor::MonitorTee::feed`]) so the reference stays
    /// exactly what the monitor actually plays — when the tee is
    /// disarmed (off, or silenced by a feedback trip) nothing is
    /// referenced, matching the silence the monitor renders.
    ///
    /// Called after the tee, once per block while engaged. No-op while
    /// bypassed.
    pub fn feed_render(&mut self, block: &[f32], teed: bool) {
        if !teed || self.engaged.is_none() || block.len() != self.scratch.len() {
            return;
        }
        let _passthrough = self
            .apm
            .process_render_f32(&[block], &mut [&mut self.scratch[..]]);
    }
}

#[cfg(test)]
mod tests {
    use noican_core::{IntensityControl, Stage, StageError, SwitchingEngine};

    use super::*;

    const RATE: usize = 48_000;
    const BLOCK: usize = WORKER_BLOCK_SAMPLES;

    /// A canceller with directly controllable state and generation cells.
    fn aec(initial: MonitorState) -> (SelfMonitorAec, Arc<AtomicI32>, Arc<AtomicU64>) {
        let state = Arc::new(AtomicI32::new(initial.as_raw()));
        let generation = Arc::new(AtomicU64::new(0));
        (
            SelfMonitorAec::new(Arc::clone(&state), Arc::clone(&generation)),
            state,
            generation,
        )
    }

    /// Deterministic speech-like signal (gliding fundamental, harmonics,
    /// syllabic envelope) — the same construction the capture-path tests
    /// use (`noican_core::capture`).
    fn pseudo_speech(len: usize, seed: f32) -> Vec<f32> {
        #[expect(clippy::cast_precision_loss, reason = "test signal indices are small")]
        (0..len)
            .map(|n| {
                let t = n as f32 / RATE as f32;
                let f0 = ((3.0 + seed) * t).sin().mul_add(20.0, 130.0 + seed);
                let phase = 2.0 * std::f32::consts::PI * f0 * t;
                let voiced: f32 = (1..=8_u32)
                    .map(|h| {
                        #[expect(clippy::cast_precision_loss, reason = "harmonic numbers are tiny")]
                        let harmonic = h as f32;
                        (phase * harmonic).sin() / harmonic
                    })
                    .sum();
                let envelope = 0.5_f32.mul_add((2.0 * std::f32::consts::PI * 2.5 * t).sin(), 0.5);
                voiced * envelope * 0.15
            })
            .collect()
    }

    /// Deterministic band-limited noise (aperiodic, speech-band): the
    /// cross-correlation signal for delay measurements, where a
    /// quasi-periodic signal would alias the lag estimate.
    fn speech_band_noise(len: usize) -> Vec<f32> {
        let mut lcg: u64 = 0x2545_F491_4F6C_DD1D;
        let mut noise: Vec<f32> = (0..len)
            .map(|_| {
                lcg = lcg.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                #[expect(clippy::cast_precision_loss, reason = "31-bit noise values")]
                let value = (lcg >> 33) as f32 / (1_u64 << 31) as f32 - 0.5;
                value * 0.4
            })
            .collect();
        // Four one-pole passes keep the content in the speech band.
        for _ in 0..4 {
            let mut state = 0.0_f32;
            for sample in &mut noise {
                state = 0.23_f32.mul_add(*sample - state, state);
                *sample = state;
            }
        }
        noise
    }

    /// Echo reduction in dB: energy of the injected echo over the energy
    /// of what the canceller left of it.
    fn erle_db(echo: &[f32], out: &[f32]) -> f64 {
        let echo_power: f64 = echo.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
        let out_power: f64 = out.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
        10.0 * (echo_power / out_power.max(1e-12)).log10()
    }

    /// Lag of the peak normalized cross-correlation (the PR #15 delay
    /// measurement): `delayed` is `reference` delayed by the returned
    /// lag.
    fn best_lag(reference: &[f32], delayed: &[f32], max_lag: usize) -> (usize, f64) {
        let mut best = (0, f64::MIN);
        for lag in 0..=max_lag {
            let overlap = reference.len().min(delayed.len().saturating_sub(lag));
            let mut dot = 0.0_f64;
            let mut energy_ref = 0.0_f64;
            let mut energy_del = 0.0_f64;
            for index in 0..overlap {
                let sample_ref = f64::from(reference[index]);
                let sample_del = f64::from(delayed[index + lag]);
                dot = sample_ref.mul_add(sample_del, dot);
                energy_ref = sample_ref.mul_add(sample_ref, energy_ref);
                energy_del = sample_del.mul_add(sample_del, energy_del);
            }
            let denom = (energy_ref * energy_del).sqrt();
            let corr = if denom > 0.0 { dot / denom } else { 0.0 };
            if corr > best.1 {
                best = (lag, corr);
            }
        }
        best
    }

    /// Runs the worker's per-block AEC sequence over a whole signal:
    /// capture through the canceller, then the (delayed, attenuated)
    /// echo of the fed render mixed into the following capture blocks —
    /// far end = what the monitor plays, exactly as in `run_block`.
    fn run_echo_session(
        aec: &mut SelfMonitorAec,
        near: &[f32],
        far: &[f32],
        teed: bool,
    ) -> Vec<f32> {
        let mut out = vec![0.0_f32; near.len()];
        for (index, (near_block, far_block)) in
            near.chunks(BLOCK).zip(far.chunks(BLOCK)).enumerate()
        {
            let output_block = &mut out[index * BLOCK..index * BLOCK + near_block.len()];
            output_block.copy_from_slice(near_block);
            aec.process_capture(output_block);
            aec.feed_render(far_block, teed);
        }
        out
    }

    /// Builds the near/far pair of a speaker session: the far end is the
    /// signal the monitor plays, whose echo returns `delay` samples
    /// later at `gain`, on top of `voice` (empty slice for echo-only).
    fn speaker_session(
        far: &[f32],
        voice: &[f32],
        delay: usize,
        gain: f32,
    ) -> (Vec<f32>, Vec<f32>) {
        let len = far.len();
        let mut near = vec![0.0_f32; len];
        let mut echo = vec![0.0_f32; len];
        for n in 0..len {
            let echoed = if n >= delay {
                far[n - delay] * gain
            } else {
                0.0
            };
            echo[n] = echoed;
            near[n] = echoed + voice.get(n).copied().unwrap_or(0.0);
        }
        (near, echo)
    }

    #[test]
    fn the_canceller_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<SelfMonitorAec>();
    }

    #[test]
    fn bypass_is_bit_exact_unless_playing() {
        for gate in [MonitorState::Off, MonitorState::Tripped] {
            let (mut canceller, _state, _generation) = aec(gate);
            let block = pseudo_speech(BLOCK, 0.0);
            let mut processed = block.clone();
            canceller.process_capture(&mut processed);
            canceller.feed_render(&block, false);
            assert_eq!(processed, block, "{gate:?} must be a bit-exact bypass");
        }
    }

    #[test]
    fn converged_erle_exceeds_threshold() {
        let total = RATE * 10;
        let far = pseudo_speech(total, 40.0);
        // 60 ms echo path: monitor ring priming (~40 ms) plus device and
        // acoustic latency — the deployment's expected shape.
        let (near, echo) = speaker_session(&far, &[], 2_880, 0.5);
        let (mut canceller, _state, _generation) = aec(MonitorState::Playing);
        let out = run_echo_session(&mut canceller, &near, &far, true);
        let tail = total - RATE * 2..total;
        let erle = erle_db(&echo[tail.clone()], &out[tail]);
        assert!(erle > 20.0, "converged ERLE too low: {erle:.1} dB");
    }

    #[test]
    fn double_talk_preserves_the_near_speech() {
        let total = RATE * 10;
        let far = pseudo_speech(total, 40.0);
        let voice = pseudo_speech(total, 0.0);
        let (near, _echo) = speaker_session(&far, &voice, 2_880, 0.5);
        let (mut canceller, _state, _generation) = aec(MonitorState::Playing);
        let out = run_echo_session(&mut canceller, &near, &far, true);
        let tail = total - RATE * 2..total;
        let voice_power: f64 = voice[tail.clone()]
            .iter()
            .map(|s| f64::from(*s) * f64::from(*s))
            .sum();
        let out_power: f64 = out[tail]
            .iter()
            .map(|s| f64::from(*s) * f64::from(*s))
            .sum();
        let ratio_db = 10.0 * (out_power / voice_power).log10();
        assert!(
            ratio_db.abs() < 6.0,
            "double-talk output {ratio_db:.1} dB off the near speech level"
        );
    }

    /// The self-monitor special case: the far end is the user's *own*
    /// voice, delayed by the engine latency. A causal filter cannot
    /// subtract the live voice from a reference that lags it, but the
    /// suppressor could still be fooled — the live voice must survive.
    #[test]
    fn self_monitor_reference_preserves_the_live_voice() {
        let total = RATE * 10;
        let voice = pseudo_speech(total, 0.0);
        // The monitor plays the engine's output ≈ the voice shifted by
        // the engine latency (~20 ms).
        let mut far = vec![0.0_f32; total];
        far[960..total].copy_from_slice(&voice[..total - 960]);
        let (near, _echo) = speaker_session(&far, &voice, 2_880, 0.5);
        let (mut canceller, _state, _generation) = aec(MonitorState::Playing);
        let out = run_echo_session(&mut canceller, &near, &far, true);
        let tail = total - RATE * 2..total;
        let voice_power: f64 = voice[tail.clone()]
            .iter()
            .map(|s| f64::from(*s) * f64::from(*s))
            .sum();
        let out_power: f64 = out[tail]
            .iter()
            .map(|s| f64::from(*s) * f64::from(*s))
            .sum();
        let ratio_db = 10.0 * (out_power / voice_power).log10();
        assert!(
            ratio_db.abs() < 6.0,
            "live voice {ratio_db:.1} dB off through the self-monitor reference"
        );
    }

    /// A generation bump (monitor toggle / output device change) must
    /// reset the learned echo path: after the bump the canceller
    /// reconverges on a *different* delay it would otherwise still be
    /// modeling with stale state.
    #[test]
    fn generation_bump_resets_and_reconverges_on_a_new_echo_path() {
        let total = RATE * 10;
        let far = pseudo_speech(total, 40.0);
        let (mut canceller, _state, generation) = aec(MonitorState::Playing);

        // Converge fully on a 60 ms path.
        let (near_old, _echo) = speaker_session(&far, &[], 2_880, 0.5);
        let _converged = run_echo_session(&mut canceller, &near_old, &far, true);

        // The output device changes: new echo path (100 ms), new session.
        generation.fetch_add(1, Ordering::Release);
        let (near_new, echo_new) = speaker_session(&far, &[], 4_800, 0.5);
        let out = run_echo_session(&mut canceller, &near_new, &far, true);
        let tail = total - RATE * 2..total;
        let erle = erle_db(&echo_new[tail.clone()], &out[tail]);
        assert!(erle > 20.0, "post-reset ERLE too low: {erle:.1} dB");
    }

    /// The strength control's dry/wet alignment must survive the AEC
    /// (the PR #15 cross-correlation measurement): the mixer taps its
    /// dry signal *after* the canceller, so at 50% intensity the engine
    /// output is exactly the half-gain, latency-compensated canceller
    /// output — zero residual offset. The same measurement against the
    /// raw microphone bounds the canceller's own capture-path delay
    /// (the Preview-only addition to the virtual microphone's total
    /// latency): ≈ 430 samples (~9 ms) measured on x86-64 and asserted
    /// here to stay under 15 ms.
    #[test]
    fn dry_wet_alignment_survives_the_aec() {
        /// Silent stage with a reported latency, standing in for a
        /// model: at 50% intensity the engine output is exactly the
        /// half-gain, delay-compensated dry path.
        #[derive(Debug)]
        struct Silent {
            latency: usize,
        }
        impl Stage for Silent {
            fn id(&self) -> &'static str {
                "silent"
            }
            fn process_block(
                &mut self,
                input: &[f32],
                output: &mut [f32],
            ) -> Result<(), StageError> {
                if input.len() != output.len() {
                    return Err(StageError::BufferLen {
                        expected: input.len(),
                        got: output.len(),
                    });
                }
                output.fill(0.0);
                Ok(())
            }
            fn latency_samples(&self) -> usize {
                self.latency
            }
            fn reset(&mut self) {}
        }

        let total = RATE * 10;
        let microphone = speech_band_noise(total);
        let stage_latency = 1_234;
        let (_publisher, mut engine) = SwitchingEngine::new(
            Box::new(Silent {
                latency: stage_latency,
            }),
            240,
            BLOCK,
            IntensityControl::new(0.5),
        )
        .expect("engine builds");
        let (mut canceller, _state, _generation) = aec(MonitorState::Playing);

        // The worker's per-block sequence with the AEC engaged: cancel,
        // run the engine, tee/reference the output.
        let mut engine_input = vec![0.0_f32; total];
        let mut engine_output = vec![0.0_f32; total];
        for index in 0..total / BLOCK {
            let range = index * BLOCK..(index + 1) * BLOCK;
            let input_block = &mut engine_input[range.clone()];
            input_block.copy_from_slice(&microphone[range.clone()]);
            canceller.process_capture(input_block);
            let input_block = &engine_input[range.clone()];
            let output_block = &mut engine_output[range];
            engine
                .process_block(input_block, output_block)
                .expect("engine block");
            canceller.feed_render(output_block, true);
        }

        // Measure over the converged tail (also bounds the O(len × lag)
        // debug-build cost of the correlation search).
        let tail = total - RATE * 3;

        // Dry/wet alignment: the output correlates with the *canceller
        // output* at exactly the stage's reported latency.
        let (lag, corr) = best_lag(&engine_input[tail..], &engine_output[tail..], 2_400);
        assert_eq!(lag, stage_latency, "residual dry/wet misalignment");
        assert!(corr > 0.9, "alignment correlation too low: {corr:.3}");

        // Capture-path delay: the canceller shifts the microphone by a
        // bounded fixed delay (both mix paths shift together). The
        // suppressor's time-varying gain lowers the correlation peak —
        // the loose bound only guards the lag from being spurious.
        let (aec_lag, aec_corr) = best_lag(&microphone[tail..], &engine_input[tail..], 2_400);
        assert!(
            aec_lag < 720,
            "AEC capture delay {aec_lag} samples exceeds 15 ms"
        );
        assert!(
            aec_corr > 0.25,
            "AEC capture correlation too low: {aec_corr:.3}"
        );
    }

    /// Malformed block lengths fail open (bypass) instead of poisoning
    /// the meeting-facing path.
    #[test]
    fn non_worker_block_lengths_fail_open() {
        let (mut canceller, _state, _generation) = aec(MonitorState::Playing);
        // Engage once with a proper block so the length check is what
        // fails, not the gate.
        let mut block = vec![0.1_f32; BLOCK];
        canceller.process_capture(&mut block);
        let mut short = vec![0.25_f32; 100];
        let reference = short.clone();
        canceller.process_capture(&mut short);
        assert_eq!(short, reference, "short blocks pass through untouched");
    }
}
