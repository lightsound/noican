use std::collections::VecDeque;

pub(crate) const HUSH_TARGET_LEVEL_DBFS: f32 = -35.0;

const SPEECH_FLOOR_DBFS: f32 = -60.0;

const ATTACK_DB_PER_SECOND: f32 = 20.0;

const RELEASE_DB_PER_SECOND: f32 = 3.0;

const RELEASE_GATE_DB: f32 = 12.0;

const LEAK_DB_PER_SECOND: f32 = 0.5;

const MAX_ATTENUATION_DB: f32 = -30.0;

const CORE_FLOOR_DBFS: f32 = -35.0;

const FLOOR_HOLD_SECONDS: f32 = 0.6;

const FLOOR_DECAY_DB_PER_SECOND: f32 = 30.0;

#[derive(Debug)]
pub(crate) struct InputLeveler {
    target_dbfs: f32,
    anchor_dbfs: f32,
    attack_per_frame: f32,
    release_per_frame: f32,
    leak_per_frame: f32,
    frame_len: usize,
    previous_gain: f32,
    applied_db: f32,
    sustained_dbfs: f32,
    hold_frames_left: u32,
    hold_frames: u32,
    floor_decay_per_frame: f32,
    history: VecDeque<f32>,
    delay: usize,
}

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

    pub(crate) fn gain_db(&self) -> f32 {
        (self.target_dbfs - self.anchor_dbfs).clamp(MAX_ATTENUATION_DB, 0.0)
    }

    fn track_sustained(&mut self, level_dbfs: f32) {
        let level_dbfs = level_dbfs.max(SPEECH_FLOOR_DBFS);
        if level_dbfs >= self.sustained_dbfs {
            self.sustained_dbfs = level_dbfs;
            self.hold_frames_left = self.hold_frames;
        } else if self.hold_frames_left > 0 {
            self.hold_frames_left -= 1;
        } else {
            let fallen = self.sustained_dbfs - self.floor_decay_per_frame;
            if fallen <= level_dbfs {
                self.sustained_dbfs = level_dbfs;
                self.hold_frames_left = self.hold_frames;
            } else {
                self.sustained_dbfs = fallen;
            }
        }
    }

    #[cfg(test)]
    pub(crate) const fn applied_db(&self) -> f32 {
        self.applied_db
    }

    fn frame_trim_db(&self) -> f32 {
        let floor_trim = (CORE_FLOOR_DBFS - self.sustained_dbfs).min(0.0);
        self.gain_db().max(floor_trim)
    }

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

    pub(crate) fn restore(&mut self, frame: &mut [f32]) {
        debug_assert_eq!(frame.len(), self.frame_len);
        for sample in frame.iter_mut() {
            let g = self.history.pop_front().unwrap_or(1.0);
            *sample /= g;
        }
    }

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
        let clap = tone(-7.0, 10);
        input[100 * FRAME..110 * FRAME].copy_from_slice(&clap);
        let mut deepest = 0.0_f32;
        for frame in input.chunks_mut(FRAME) {
            leveler.apply(frame);
            deepest = deepest.min(leveler.gain_db());
        }
        assert!(deepest > -2.1, "clap trimmed by {deepest} dB");
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
        run_apply(&mut leveler, &tone(-70.0, 500));
        assert!((leveler.gain_db() - before).abs() < f32::EPSILON);
    }

    #[test]
    fn a_quieter_talker_releases_the_trim_slowly() {
        let mut leveler = InputLeveler::new(HUSH_TARGET_LEVEL_DBFS, SR, FRAME, 0);
        run_apply(&mut leveler, &tone(-22.0, 500));
        let full = HUSH_TARGET_LEVEL_DBFS + 22.0;
        assert!((leveler.gain_db() - full).abs() < 0.05);
        run_apply(&mut leveler, &tone(-30.0, 100));
        assert!(
            (leveler.gain_db() - (full + 3.0)).abs() < 0.05,
            "gain {} after 1 s of softer speech",
            leveler.gain_db()
        );
        run_apply(&mut leveler, &tone(-30.0, 1000));
        assert!(
            (leveler.gain_db() - (HUSH_TARGET_LEVEL_DBFS + 30.0)).abs() < 0.05,
            "gain {}",
            leveler.gain_db()
        );
        run_apply(&mut leveler, &tone(-45.0, 2000));
        assert!(leveler.gain_db().abs() < f32::EPSILON);
    }

    #[test]
    fn breaths_and_a_distant_talker_only_leak_the_trim() {
        let mut leveler = InputLeveler::new(HUSH_TARGET_LEVEL_DBFS, SR, FRAME, 0);
        run_apply(&mut leveler, &tone(-22.0, 500));
        let full = HUSH_TARGET_LEVEL_DBFS + 22.0;
        assert!((leveler.gain_db() - full).abs() < 0.05);
        run_apply(&mut leveler, &tone(-50.0, 300));
        assert!(
            (leveler.gain_db() - (full + 1.5)).abs() < 0.05,
            "gain {} after 3 s of breaths",
            leveler.gain_db()
        );
        run_apply(&mut leveler, &tone(-42.0, 4000));
        assert!(
            leveler.gain_db().abs() < f32::EPSILON,
            "gain {}",
            leveler.gain_db()
        );
    }

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
            if i < 400 || i % 400 < 150 {
                continue;
            }
            assert!(
                core_sees > level.min(CORE_FLOOR_DBFS) - 0.05,
                "frame {i}: level {level}, core sees {core_sees}"
            );
            if (level + 42.0).abs() < 0.1 {
                assert!(
                    leveler.applied_db().abs() < 0.05,
                    "frame {i} trimmed by {}",
                    leveler.applied_db()
                );
            } else if (level + 20.0).abs() < 0.1 {
                worst_loud = worst_loud.max((core_sees - HUSH_TARGET_LEVEL_DBFS).abs());
            } else if (level + 30.0).abs() < 0.1 {
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

    #[test]
    fn a_pause_lets_the_floor_follow_the_next_sentence_at_once() {
        let mut leveler = InputLeveler::new(HUSH_TARGET_LEVEL_DBFS, SR, FRAME, 0);
        run_apply(&mut leveler, &tone(-20.0, 400));
        assert!((leveler.gain_db() - (HUSH_TARGET_LEVEL_DBFS + 20.0)).abs() < 0.05);
        run_apply(&mut leveler, &vec![0.0; 120 * FRAME]);
        assert!((leveler.gain_db() - (HUSH_TARGET_LEVEL_DBFS + 20.0)).abs() < 0.05);
        let mut frame = tone(-42.0, 1);
        let original = frame.clone();
        leveler.apply(&mut frame);
        assert_eq!(frame, original);
        run_apply(&mut leveler, &tone(-20.0, 400));
        run_apply(&mut leveler, &vec![0.0; 300 * FRAME]);
        let mut frame = tone(-30.0, 1);
        leveler.apply(&mut frame);
        assert!(
            (-30.0 + leveler.applied_db() - CORE_FLOOR_DBFS).abs() < 0.05,
            "core sees {}",
            -30.0 + leveler.applied_db()
        );
        run_apply(&mut leveler, &vec![0.0; 300 * FRAME]);
        let mut frame = tone(-20.0, 1);
        leveler.apply(&mut frame);
        assert!((leveler.applied_db() - (HUSH_TARGET_LEVEL_DBFS + 20.0)).abs() < 0.05);
    }

    #[test]
    fn a_short_pause_keeps_the_floor_held() {
        let mut leveler = InputLeveler::new(HUSH_TARGET_LEVEL_DBFS, SR, FRAME, 0);
        run_apply(&mut leveler, &tone(-20.0, 400));
        run_apply(&mut leveler, &tone(-19.5, 1));
        let full = leveler.gain_db();
        assert!(full < -15.0, "anchor trim {full}");
        run_apply(&mut leveler, &vec![0.0; 50 * FRAME]);
        let mut frame = tone(-30.0, 1);
        leveler.apply(&mut frame);
        assert!(
            (leveler.applied_db() - full).abs() < 0.05,
            "floor released across a short pause: trim {} (anchor {full})",
            leveler.applied_db()
        );
        let mut leveler = InputLeveler::new(HUSH_TARGET_LEVEL_DBFS, SR, FRAME, 0);
        run_apply(&mut leveler, &tone(-20.0, 400));
        for i in 0..200 {
            #[expect(
                clippy::cast_precision_loss,
                reason = "frame indices fit f32 for a short test signal"
            )]
            let mut frame = tone(0.05f32.mul_add(-(i as f32), -20.0), 1);
            leveler.apply(&mut frame);
        }
        run_apply(&mut leveler, &tone(-26.0, 1));
        let before_pause = leveler.applied_db();
        assert!(
            before_pause > HUSH_TARGET_LEVEL_DBFS + 21.0,
            "floor should be engaged: {before_pause}"
        );
        run_apply(&mut leveler, &vec![0.0; 50 * FRAME]);
        let mut frame = tone(-30.0, 1);
        leveler.apply(&mut frame);
        assert!(
            (leveler.applied_db() - before_pause).abs() < 0.05,
            "short pause released the floor after a trailing-off level: {} → {}",
            before_pause,
            leveler.applied_db()
        );
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
        let max_step = leveler
            .history
            .iter()
            .zip(leveler.history.iter().skip(1))
            .fold(0.0_f32, |m, (a, b)| m.max((a - b).abs()));
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
