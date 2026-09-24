//! Input leveler for level-sensitive cores: trims the input so the
//! loudest sustained talker sits at a target level, and undoes the trim
//! on the output so the stage's overall level is untouched.
//!
//! Hush's network reads the *absolute* input level as a cue for who is
//! the primary speaker (measured in `docs/hush-48k-eval.md`, "Level
//! dependence"): a lone voice passes at parity around −37 dBFS RMS,
//! loses level and its 4–7 kHz band progressively above that (−4.6 dB at
//! −30, −8.8 dB at −15), and is treated as background below ≈ −45. In a
//! live session the talker's level moves with their voice and mic
//! distance, so the output brightness follows the sentence dynamics —
//! the "quality comes and goes" the owner heard on 2026-09-24.
//!
//! # Design
//!
//! `x ── × g(t) ──▶ core ──▶ × 1/g(t − delay) ──▶ out`
//!
//! - `g` is **attenuation only** (`g ≤ 1`). Boosting would also lift a
//!   quiet talker across the room into the level window where Hush
//!   passes a lone voice as the primary speaker; with attenuation only,
//!   input at or below the target is untouched and behaves exactly as
//!   the plain stage did in the owner's live acceptance.
//! - The anchor follows the frame level in dB with a **slew-rate limit**
//!   in each direction: fast up (`ATTACK_DB_PER_SECOND`) so the talker's
//!   first loud sentence sets it within a couple of seconds while a
//!   100 ms clap moves it by only 2 dB; slow down
//!   (`RELEASE_DB_PER_SECOND`) for frames within `RELEASE_GATE_DB` of
//!   the anchor, i.e. the same talker getting quieter; and a crawl
//!   (`LEAK_DB_PER_SECOND`) for anything further below — breaths and
//!   room tone between sentences, or a quieter second talker — so they
//!   do not hand the next loud sentence to the core hot. Frames below
//!   `SPEECH_FLOOR_DBFS` hold the anchor, so pauses of any length leave
//!   the trim as it is.
//! - The trim is **floored on the sentence-scale level** so the talker's
//!   own quiet sentences never reach the core in the region the sweep
//!   shows a lone voice being gated or zeroed: a peak-hold of the frame
//!   level (held for `FLOOR_HOLD_SECONDS`, then falling at
//!   `FLOOR_DECAY_DB_PER_SECOND`) is never trimmed below
//!   `CORE_FLOOR_DBFS`. Loud sentences get the anchor's full trim with
//!   their syllable dynamics and pauses intact, sentences already under
//!   the floor get none, and a sentence in between is lifted onto the
//!   floor within about two seconds.
//!   (The anchor alone would send a −42 dBFS sentence after a −20 dBFS
//!   one to the core at −57 dBFS, where the sweep records the output
//!   zeroed.)
//! - The gain ramps linearly across each frame (no zipper noise) and the
//!   per-sample gain is remembered for `delay` samples so the restore
//!   divides each output sample by the gain that was applied to the
//!   input it came from: with an identity core the pair is the identity
//!   to float precision (a unit test pins it).
//!
//! What the core sees is therefore a compressed copy of the input whose
//! loud talker sits at the target; what leaves the stage has the input's
//! dynamics back. Only the core's decisions change.

use std::collections::VecDeque;

/// Anchor target. Hush passes a lone voice at parity around −37 dBFS
/// segment RMS (`docs/hush-48k-eval.md`, level sweep: own-voice SI-SDR
/// peaks at −38…−37, output level +0.4 dB there). The anchor sits on the
/// loud frames, a few dB above the segment RMS, so the target is set
/// 2 dB above that point; measured with the harness (stand-in voice at
/// −37 / −30 / −22 dBFS, SIR +12, own-voice SI-SDR in dB): target −37 →
/// 12.4 / 15.2 / 14.4, **−35 → 14.3 / 14.1 / 14.0**, −33 → 14.8 / 12.0 /
/// 12.4. −35 is the flattest, i.e. the most level-independent, and
/// stays within 0.3 dB of the unleveled stage's best case (14.4).
pub(crate) const HUSH_TARGET_LEVEL_DBFS: f32 = -35.0;

/// Frames quieter than this do not move the anchor (room tone and
/// pauses; VCTK stand-in speech frames sit above −55 dBFS at the target).
const SPEECH_FLOOR_DBFS: f32 = -60.0;

/// Anchor slew upward. 20 dB/s: a talker 15 dB above the target is
/// trimmed to it within ≈ 1 s (2.4 s to settle within 1 dB on the
/// stand-in), while a 100 ms transient moves the anchor by 2 dB.
const ATTACK_DB_PER_SECOND: f32 = 20.0;

/// Anchor slew downward for frames within [`RELEASE_GATE_DB`] of the
/// anchor. 3 dB/s: a within-sentence dip of 10 dB lasting 300 ms
/// releases the trim by under 1 dB; a talker who genuinely gets quieter
/// is followed within seconds. Measured against 2 and 10 dB/s on the
/// stand-in (10 dB/s let the trim breathe with the syllables and left
/// the core 9 dB hot; 2 dB/s settled in 13 s instead of 2.4 s).
const RELEASE_DB_PER_SECOND: f32 = 3.0;

/// Frames more than this far under the anchor release it only at
/// [`LEAK_DB_PER_SECOND`]. Without the gate, three seconds of breaths
/// and room tone at −55 dBFS between sentences released 9 dB of trim
/// and the next loud sentence reached the core at −28 dBFS (stand-in at
/// −22 dBFS, per-second trace in the pull request); with the gate the
/// core's per-second level tracks the −37 dBFS reference within 5 dB
/// (12 and 15 dB measured alike). 12 dB is chosen so a second talker at
/// the harness's +12 dB SIR sits outside it and cannot pull the trim
/// down within their turn, while the talker's own softer sentences
/// (a few dB) are still followed at the release rate.
const RELEASE_GATE_DB: f32 = 12.0;

/// Anchor crawl for frames under the release gate: a talker who steps
/// 20 dB back from the microphone regains full level in 40 s rather
/// than never.
const LEAK_DB_PER_SECOND: f32 = 0.5;

/// Deepest trim. Input hotter than target + 30 dB (−7 dBFS RMS) is at
/// the microphone's own clipping point; the trim stops there.
const MAX_ATTENUATION_DB: f32 = -30.0;

/// The talker's sustained level is not trimmed below this. It is
/// measured on a peak-hold of the frame level (see
/// [`FLOOR_DECAY_DB_PER_SECOND`]), which sits about 5 dB above the
/// segment RMS the sweep in `docs/hush-48k-eval.md` is expressed in, so
/// −35 here corresponds to ≈ −40 dBFS RMS there: a lone voice at −40
/// still comes through (own-voice SI-SDR 9.1 dB, level +1.3 dB) while
/// one at −40 and below is still suppressed as background (−17.7 dB);
/// at −38 the second flips (−2.1 dB). The floor therefore keeps the
/// talker's quiet sentences audible without lifting a second talker
/// into the primary window.
const CORE_FLOOR_DBFS: f32 = -35.0;

/// How long the peak-hold level the floor is measured on keeps its
/// value before it starts to fall. The floor must react to a *sentence*
/// getting quieter, not to the pauses and soft syllables inside a loud
/// one: measured per 10 ms frame, on a 0.3–1 s one-pole, or on a
/// peak-hold that starts falling at once, the level dropped during
/// every pause and the next onset reached the core hot (dynamic
/// stand-in, second loud block: SI-SDR 8.0 / 7.3 dB; static stand-in at
/// −22 dBFS: 11.6 dB against 14.0 dB without a floor). Pauses and
/// syllable dips are shorter than a second; a quieter sentence lasts
/// longer.
const FLOOR_HOLD_SECONDS: f32 = 1.0;

/// Fall rate of the peak-hold level once the hold has run out: a 22 dB
/// quieter sentence that follows a loud one without a pause is followed
/// 1.1 s after the hold, 2.1 s in all. Across a pause longer than the
/// hold the fall happens during the pause, and the next sentence gets
/// its floor on its first frame.
const FLOOR_DECAY_DB_PER_SECOND: f32 = 20.0;

/// Trims the input toward a target level and restores the output.
#[derive(Debug)]
pub(crate) struct InputLeveler {
    target_dbfs: f32,
    /// Level of the loudest sustained talker, dBFS; starts at the target
    /// so a fresh stage applies no trim.
    anchor_dbfs: f32,
    attack_per_frame: f32,
    release_per_frame: f32,
    leak_per_frame: f32,
    frame_len: usize,
    /// Linear gain at the end of the previous frame.
    previous_gain: f32,
    /// Trim applied to the last frame in dB (anchor trim, floored).
    applied_db: f32,
    /// Sentence-scale level (peak-hold over speech frames), dBFS: what
    /// the floor is measured against.
    sustained_dbfs: f32,
    /// Speech frames left before `sustained_dbfs` starts to fall.
    hold_frames_left: u32,
    hold_frames: u32,
    floor_decay_per_frame: f32,
    /// Per-sample gains applied to the input, delayed by `delay` for the
    /// restore; primed with unity.
    history: VecDeque<f32>,
    delay: usize,
}

/// 10 · log10 of the mean square of `samples`, floored well under the
/// speech floor for silence.
fn frame_level_dbfs(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return -200.0;
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "frame lengths are a few hundred samples"
    )]
    let len = samples.len() as f32;
    let mean_square = samples.iter().map(|s| s * s).sum::<f32>() / len;
    10.0 * mean_square.max(1e-20).log10()
}

impl InputLeveler {
    /// A leveler for `frame_len`-sample frames at `sample_rate`, whose
    /// restore runs `delay` samples after the apply (the wrapped core's
    /// total output delay at that rate).
    pub(crate) fn new(target_dbfs: f32, sample_rate: u32, frame_len: usize, delay: usize) -> Self {
        #[expect(
            clippy::cast_precision_loss,
            reason = "frame length and sample rate are small exact integers"
        )]
        let frames_per_second = sample_rate as f32 / frame_len as f32;
        let mut history = VecDeque::with_capacity(delay + frame_len);
        history.extend(std::iter::repeat_n(1.0, delay));
        Self {
            target_dbfs,
            anchor_dbfs: target_dbfs,
            attack_per_frame: ATTACK_DB_PER_SECOND / frames_per_second,
            release_per_frame: RELEASE_DB_PER_SECOND / frames_per_second,
            leak_per_frame: LEAK_DB_PER_SECOND / frames_per_second,
            frame_len,
            previous_gain: 1.0,
            applied_db: 0.0,
            sustained_dbfs: target_dbfs,
            hold_frames_left: 0,
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a positive frame count of the order of 100"
            )]
            hold_frames: (FLOOR_HOLD_SECONDS * frames_per_second).round() as u32,
            floor_decay_per_frame: FLOOR_DECAY_DB_PER_SECOND / frames_per_second,
            history,
            delay,
        }
    }

    /// The anchor's trim in dB (≤ 0), as it stands after the last frame:
    /// what a frame at or above the anchor is trimmed by.
    pub(crate) fn gain_db(&self) -> f32 {
        (self.target_dbfs - self.anchor_dbfs).clamp(MAX_ATTENUATION_DB, 0.0)
    }

    /// Peak-hold of the frame level: rises at once, holds for
    /// `FLOOR_HOLD_SECONDS`, then falls at `FLOOR_DECAY_DB_PER_SECOND`
    /// toward the frame level (no further than the speech floor). It runs
    /// through pauses too — the hold and the fall are wall time, not
    /// speech time — so after a pause longer than the hold the next
    /// sentence sets it on its first frame, whatever its level, and the
    /// floor is right for that sentence from the start.
    fn track_sustained(&mut self, level_dbfs: f32) {
        if level_dbfs >= self.sustained_dbfs {
            self.sustained_dbfs = level_dbfs;
            self.hold_frames_left = self.hold_frames;
        } else if self.hold_frames_left > 0 {
            self.hold_frames_left -= 1;
        } else {
            self.sustained_dbfs = (self.sustained_dbfs - self.floor_decay_per_frame)
                .max(level_dbfs.max(SPEECH_FLOOR_DBFS));
        }
    }

    /// Trim actually applied to the last frame in dB (≤ 0): the anchor's
    /// trim, floored so the frame did not go under [`CORE_FLOOR_DBFS`].
    #[cfg(test)]
    pub(crate) const fn applied_db(&self) -> f32 {
        self.applied_db
    }

    /// Trim for the current frame: the anchor's trim, but never enough to
    /// push the talker's sustained level under the core floor.
    fn frame_trim_db(&self) -> f32 {
        let floor_trim = (CORE_FLOOR_DBFS - self.sustained_dbfs).min(0.0);
        self.gain_db().max(floor_trim)
    }

    /// Updates the peak-hold sustained level on every frame and moves the
    /// anchor toward this frame's level under the slew limits; frames
    /// under the speech floor leave the anchor alone.
    fn track(&mut self, level_dbfs: f32) {
        self.track_sustained(level_dbfs);
        if level_dbfs <= SPEECH_FLOOR_DBFS {
            return;
        }
        if level_dbfs > self.anchor_dbfs {
            self.anchor_dbfs = (self.anchor_dbfs + self.attack_per_frame).min(level_dbfs);
        } else if level_dbfs > self.anchor_dbfs - RELEASE_GATE_DB {
            self.anchor_dbfs = (self.anchor_dbfs - self.release_per_frame).max(level_dbfs);
        } else {
            self.anchor_dbfs = (self.anchor_dbfs - self.leak_per_frame).max(level_dbfs);
        }
    }

    /// Trims one frame in place, ramping from the previous frame's gain
    /// to the new one, and records the per-sample gain for [`Self::restore`].
    pub(crate) fn apply(&mut self, frame: &mut [f32]) {
        debug_assert_eq!(frame.len(), self.frame_len);
        let level = frame_level_dbfs(frame);
        self.track(level);
        self.applied_db = self.frame_trim_db();
        let target = 10.0_f32.powf(self.applied_db / 20.0);
        #[expect(
            clippy::cast_precision_loss,
            reason = "frame length is a few hundred samples; exact in f32"
        )]
        let step = (target - self.previous_gain) / self.frame_len as f32;
        let mut g = self.previous_gain;
        for sample in frame.iter_mut() {
            g += step;
            *sample *= g;
            self.history.push_back(g);
        }
        self.previous_gain = target;
    }

    /// Divides one output frame by the gains applied `delay` samples
    /// earlier.
    pub(crate) fn restore(&mut self, frame: &mut [f32]) {
        debug_assert_eq!(frame.len(), self.frame_len);
        for sample in frame.iter_mut() {
            let g = self.history.pop_front().unwrap_or(1.0);
            *sample /= g;
        }
    }

    /// Back to the fresh state: no trim, unity history.
    pub(crate) fn reset(&mut self) {
        self.anchor_dbfs = self.target_dbfs;
        self.sustained_dbfs = self.target_dbfs;
        self.hold_frames_left = 0;
        self.previous_gain = 1.0;
        self.applied_db = 0.0;
        self.history.clear();
        self.history.extend(std::iter::repeat_n(1.0, self.delay));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;
    const FRAME: usize = 480;

    /// A 400 Hz tone (exactly four cycles per frame, so every frame has the
    /// same RMS) at `level_dbfs` RMS, `frames` frames long.
    fn tone(level_dbfs: f32, frames: usize) -> Vec<f32> {
        let amplitude = 10.0_f32.powf(level_dbfs / 20.0) * std::f32::consts::SQRT_2;
        (0..frames * FRAME)
            .map(|n| {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "sample indices fit f32 for a short test signal"
                )]
                let t = n as f32 / 48_000.0;
                amplitude * (2.0 * std::f32::consts::PI * 400.0 * t).sin()
            })
            .collect()
    }

    fn rms_dbfs(s: &[f32]) -> f32 {
        frame_level_dbfs(s)
    }

    fn run_apply(leveler: &mut InputLeveler, input: &[f32]) -> Vec<f32> {
        let mut out = input.to_vec();
        for frame in out.chunks_mut(FRAME) {
            leveler.apply(frame);
        }
        out
    }

    #[test]
    fn frame_level_matches_a_known_sine() {
        // Amplitude 0.1 sine: RMS 0.0707 → −23.0 dBFS.
        let s = tone(-23.0, 10);
        assert!((rms_dbfs(&s) + 23.0).abs() < 0.05, "{}", rms_dbfs(&s));
        assert!(frame_level_dbfs(&[0.0; FRAME]) < -150.0);
        assert!(frame_level_dbfs(&[]) < -150.0);
    }

    #[test]
    fn input_at_or_below_the_target_is_untouched() {
        for level in [-37.5, -45.0, -55.0] {
            let mut leveler = InputLeveler::new(HUSH_TARGET_LEVEL_DBFS, SR, FRAME, 0);
            let input = tone(level, 300);
            let out = run_apply(&mut leveler, &input);
            assert_eq!(out, input, "level {level}");
            assert!(leveler.gain_db().abs() < f32::EPSILON);
        }
    }

    #[test]
    fn loud_input_is_trimmed_to_the_target() {
        for level in [-30.0, -22.0, -15.0] {
            let mut leveler = InputLeveler::new(HUSH_TARGET_LEVEL_DBFS, SR, FRAME, 0);
            let input = tone(level, 500);
            let out = run_apply(&mut leveler, &input);
            let expected = HUSH_TARGET_LEVEL_DBFS - level;
            assert!(
                (leveler.gain_db() - expected).abs() < 0.05,
                "level {level}: gain {} dB, want {expected}",
                leveler.gain_db()
            );
            // Settled: the last two seconds sit at the target.
            let tail = &out[out.len() - 200 * FRAME..];
            assert!(
                (rms_dbfs(tail) - HUSH_TARGET_LEVEL_DBFS).abs() < 0.1,
                "level {level}: tail at {} dBFS",
                rms_dbfs(tail)
            );
        }
    }

    #[test]
    fn attack_settles_within_two_seconds_and_is_slew_limited() {
        let mut leveler = InputLeveler::new(HUSH_TARGET_LEVEL_DBFS, SR, FRAME, 0);
        let mut input = tone(-22.0, 300);
        let mut settled_at = None;
        for (i, frame) in input.chunks_mut(FRAME).enumerate() {
            leveler.apply(frame);
            let gain = leveler.gain_db();
            // Never faster than the attack slew: 0.2 dB per 10 ms frame.
            #[expect(
                clippy::cast_precision_loss,
                reason = "frame indices fit f32 for a short test signal"
            )]
            let min_gain = -(ATTACK_DB_PER_SECOND / 100.0) * (i + 1) as f32;
            assert!(
                gain >= min_gain - 1e-3,
                "frame {i}: gain {gain} outran the slew"
            );
            if settled_at.is_none() && (gain - (HUSH_TARGET_LEVEL_DBFS + 22.0)).abs() < 0.1 {
                settled_at = Some(i);
            }
        }
        let settled_at = settled_at.expect("the trim must settle");
        assert!(settled_at < 200, "settled after {settled_at} frames");
    }

    #[test]
    fn a_short_transient_barely_moves_the_anchor() {
        let mut leveler = InputLeveler::new(HUSH_TARGET_LEVEL_DBFS, SR, FRAME, 0);
        let mut input = tone(-37.0, 400);
        // 100 ms clap at −7 dBFS at t = 1 s.
        let clap = tone(-7.0, 10);
        input[100 * FRAME..110 * FRAME].copy_from_slice(&clap);
        let mut deepest = 0.0_f32;
        for frame in input.chunks_mut(FRAME) {
            leveler.apply(frame);
            deepest = deepest.min(leveler.gain_db());
        }
        assert!(deepest > -2.1, "clap trimmed by {deepest} dB");
        // And it has released by the end: 3 dB/s over 2.9 s covers it.
        assert!(
            leveler.gain_db() > -0.01,
            "still trimmed by {}",
            leveler.gain_db()
        );
    }

    #[test]
    fn silence_holds_the_anchor() {
        let mut leveler = InputLeveler::new(HUSH_TARGET_LEVEL_DBFS, SR, FRAME, 0);
        run_apply(&mut leveler, &tone(-22.0, 500));
        let before = leveler.gain_db();
        run_apply(&mut leveler, &vec![0.0; 1000 * FRAME]);
        assert!((leveler.gain_db() - before).abs() < f32::EPSILON);
        // Room tone under the floor holds it too.
        run_apply(&mut leveler, &tone(-70.0, 500));
        assert!((leveler.gain_db() - before).abs() < f32::EPSILON);
    }

    #[test]
    fn a_quieter_talker_releases_the_trim_slowly() {
        let mut leveler = InputLeveler::new(HUSH_TARGET_LEVEL_DBFS, SR, FRAME, 0);
        run_apply(&mut leveler, &tone(-22.0, 500));
        let full = HUSH_TARGET_LEVEL_DBFS + 22.0;
        assert!((leveler.gain_db() - full).abs() < 0.05);
        // The same talker 8 dB softer for one second: 3 dB released.
        run_apply(&mut leveler, &tone(-30.0, 100));
        assert!(
            (leveler.gain_db() - (full + 3.0)).abs() < 0.05,
            "gain {} after 1 s of softer speech",
            leveler.gain_db()
        );
        // Ten seconds of it: the trim follows their level.
        run_apply(&mut leveler, &tone(-30.0, 1000));
        assert!(
            (leveler.gain_db() - (HUSH_TARGET_LEVEL_DBFS + 30.0)).abs() < 0.05,
            "gain {}",
            leveler.gain_db()
        );
        // Under the target (and under the gate, so at the leak rate):
        // attenuation only, the trim stops at 0.
        run_apply(&mut leveler, &tone(-45.0, 2000));
        assert!(leveler.gain_db().abs() < f32::EPSILON);
    }

    #[test]
    fn breaths_and_a_distant_talker_only_leak_the_trim() {
        let mut leveler = InputLeveler::new(HUSH_TARGET_LEVEL_DBFS, SR, FRAME, 0);
        run_apply(&mut leveler, &tone(-22.0, 500));
        let full = HUSH_TARGET_LEVEL_DBFS + 22.0;
        assert!((leveler.gain_db() - full).abs() < 0.05);
        // Three seconds of breaths at −50 dBFS (28 dB under the anchor,
        // above the speech floor): 1.5 dB at the leak rate, not 9 dB.
        run_apply(&mut leveler, &tone(-50.0, 300));
        assert!(
            (leveler.gain_db() - (full + 1.5)).abs() < 0.05,
            "gain {} after 3 s of breaths",
            leveler.gain_db()
        );
        // The talker steps 20 dB back for good: the trim is gone in 40 s.
        run_apply(&mut leveler, &tone(-42.0, 4000));
        assert!(
            leveler.gain_db().abs() < f32::EPSILON,
            "gain {}",
            leveler.gain_db()
        );
    }

    /// The owner's spread: sentences at −20 dBFS and at −42 dBFS. The
    /// anchor pins to the loud ones; the quiet ones must still reach the
    /// core at or above the floor, and the loud ones at the target.
    #[test]
    fn quiet_sentences_of_the_same_talker_stay_above_the_core_floor() {
        let mut leveler = InputLeveler::new(HUSH_TARGET_LEVEL_DBFS, SR, FRAME, 0);
        let mut input = Vec::new();
        for _ in 0..4 {
            input.extend(tone(-20.0, 400));
            input.extend(tone(-42.0, 400));
            input.extend(tone(-30.0, 400));
        }
        let mut worst_loud = 0.0_f32;
        for (i, frame) in input.chunks_mut(FRAME).enumerate() {
            let level = frame_level_dbfs(frame);
            leveler.apply(frame);
            let core_sees = level + leveler.applied_db();
            // Skip the first 2.5 s of every sentence: the floor's
            // peak-hold (1 s hold, then 20 dB/s: 2.1 s for the 22 dB step)
            // and the anchor's attack are still moving there.
            if i < 400 || i % 400 < 250 {
                continue;
            }
            // Never trimmed under the floor (sentences already under it
            // pass untouched).
            assert!(
                core_sees > level.min(CORE_FLOOR_DBFS) - 0.05,
                "frame {i}: level {level}, core sees {core_sees}"
            );
            // Quiet sentences pass untouched, loud ones sit at the
            // target, the ones in between never exceed the target.
            if (level + 42.0).abs() < 0.1 {
                assert!(
                    leveler.applied_db().abs() < 0.05,
                    "frame {i} trimmed by {}",
                    leveler.applied_db()
                );
            } else if (level + 20.0).abs() < 0.1 {
                worst_loud = worst_loud.max((core_sees - HUSH_TARGET_LEVEL_DBFS).abs());
            } else if (level + 30.0).abs() < 0.1 {
                // Lifted onto the floor while the anchor is pinned to
                // the loud sentence, then released toward the target
                // (−30 is inside the release gate); never above it.
                assert!(
                    core_sees <= HUSH_TARGET_LEVEL_DBFS + 0.01,
                    "frame {i}: core sees {core_sees}"
                );
            }
        }
        assert!(
            worst_loud < 1.0,
            "loud sentences off target by {worst_loud} dB"
        );
    }

    /// The owner's pauses sit far under the speech floor. The hold and
    /// the fall of the floor's peak-hold run through them, so a quiet
    /// sentence after a loud one and a pause is untrimmed from its first
    /// frame — the first second is not spent in the gated region.
    #[test]
    fn a_pause_lets_the_floor_follow_the_next_sentence_at_once() {
        let mut leveler = InputLeveler::new(HUSH_TARGET_LEVEL_DBFS, SR, FRAME, 0);
        run_apply(&mut leveler, &tone(-20.0, 400));
        assert!((leveler.gain_db() - (HUSH_TARGET_LEVEL_DBFS + 20.0)).abs() < 0.05);
        // Three seconds of digital silence: anchor held, peak-hold fallen.
        run_apply(&mut leveler, &vec![0.0; 300 * FRAME]);
        assert!((leveler.gain_db() - (HUSH_TARGET_LEVEL_DBFS + 20.0)).abs() < 0.05);
        // A −42 dBFS sentence: untouched from the very first frame, even
        // though the anchor still asks for the full trim.
        let mut frame = tone(-42.0, 1);
        let original = frame.clone();
        leveler.apply(&mut frame);
        assert_eq!(frame, original);
        // A −30 dBFS sentence after the same pause: on the floor at once.
        run_apply(&mut leveler, &tone(-20.0, 400));
        run_apply(&mut leveler, &vec![0.0; 300 * FRAME]);
        let mut frame = tone(-30.0, 1);
        leveler.apply(&mut frame);
        assert!(
            (-30.0 + leveler.applied_db() - CORE_FLOOR_DBFS).abs() < 0.05,
            "core sees {}",
            -30.0 + leveler.applied_db()
        );
        // And a loud sentence after a pause gets the anchor's trim at once.
        run_apply(&mut leveler, &vec![0.0; 300 * FRAME]);
        let mut frame = tone(-20.0, 1);
        leveler.apply(&mut frame);
        assert!((leveler.applied_db() - (HUSH_TARGET_LEVEL_DBFS + 20.0)).abs() < 0.05);
    }

    #[test]
    fn trim_is_capped() {
        let mut leveler = InputLeveler::new(HUSH_TARGET_LEVEL_DBFS, SR, FRAME, 0);
        run_apply(&mut leveler, &tone(-1.0, 600));
        assert!((leveler.gain_db() - MAX_ATTENUATION_DB).abs() < f32::EPSILON);
    }

    #[test]
    fn gain_ramps_within_the_frame_without_steps() {
        let mut leveler = InputLeveler::new(HUSH_TARGET_LEVEL_DBFS, SR, FRAME, 0);
        let input = tone(-22.0, 50);
        run_apply(&mut leveler, &input);
        // The history holds every per-sample gain (delay 0 → nothing
        // popped): consecutive samples differ by at most the attack slew
        // spread over a frame.
        let max_step = leveler
            .history
            .iter()
            .zip(leveler.history.iter().skip(1))
            .fold(0.0_f32, |m, (a, b)| m.max((a - b).abs()));
        // 0.2 dB per frame at gain ≤ 1 is < 0.023 linear per frame.
        #[expect(
            clippy::cast_precision_loss,
            reason = "frame length is a few hundred samples; exact in f32"
        )]
        let bound = 0.03 / FRAME as f32;
        assert!(max_step < bound, "per-sample gain step {max_step}");
    }

    #[test]
    fn restore_inverts_apply_through_the_delay() {
        const DELAY: usize = 600;
        let mut leveler = InputLeveler::new(HUSH_TARGET_LEVEL_DBFS, SR, FRAME, DELAY);
        let input = tone(-20.0, 300);
        // Identity core with a DELAY-sample FIFO between apply and restore.
        let mut fifo: VecDeque<f32> = std::iter::repeat_n(0.0, DELAY).collect();
        let mut out = vec![0.0_f32; input.len()];
        let mut frame = vec![0.0_f32; FRAME];
        for (i, o) in input.chunks(FRAME).zip(out.chunks_mut(FRAME)) {
            frame.copy_from_slice(i);
            leveler.apply(&mut frame);
            fifo.extend(frame.iter().copied());
            for s in o.iter_mut() {
                *s = fifo.pop_front().unwrap_or(0.0);
            }
            leveler.restore(o);
        }
        let err = out[DELAY..]
            .iter()
            .zip(&input[..input.len() - DELAY])
            .fold(0.0_f32, |m, (a, b)| m.max((a - b).abs()));
        assert!(err < 1e-6, "restore error {err}");
        assert_eq!(leveler.history.len(), DELAY);
    }

    #[test]
    fn history_never_reallocates_after_construction() {
        let mut leveler = InputLeveler::new(HUSH_TARGET_LEVEL_DBFS, SR, FRAME, 600);
        let cap = leveler.history.capacity();
        let mut frame = vec![0.1_f32; FRAME];
        for _ in 0..20 {
            leveler.apply(&mut frame);
            assert_eq!(leveler.history.capacity(), cap, "grew in apply");
            leveler.restore(&mut frame);
        }
        assert_eq!(leveler.history.capacity(), cap);
    }

    #[test]
    fn reset_returns_to_unity() {
        let mut leveler = InputLeveler::new(HUSH_TARGET_LEVEL_DBFS, SR, FRAME, 600);
        run_apply(&mut leveler, &tone(-15.0, 300));
        assert!(leveler.gain_db() < -19.0, "gain {}", leveler.gain_db());
        leveler.reset();
        assert!(leveler.gain_db().abs() < f32::EPSILON);
        assert_eq!(leveler.history.len(), 600);
        assert!(
            leveler
                .history
                .iter()
                .all(|g| (*g - 1.0).abs() < f32::EPSILON)
        );
        let mut frame = tone(-37.0, 1);
        let original = frame.clone();
        leveler.apply(&mut frame);
        assert_eq!(frame, original);
    }
}
