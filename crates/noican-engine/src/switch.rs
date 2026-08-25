//! The gain ramp that hides a model switch.
//!
//! Switching models cannot be a plain swap: the outgoing model's output stops
//! mid-waveform and the incoming one starts from a cold pipeline, so a bare
//! swap produces a click at the seam and then a gap while the new model primes.
//!
//! A true crossfade — running both models and mixing — only works when their
//! delays match. They usually do not: the catalogued models range from 10.7 ms
//! to 50 ms of algorithmic delay, so mixing two of them means mixing the same
//! speech against a time-shifted copy of itself, which combs audibly for the
//! length of the fade. So instead the ramp fades the outgoing model down,
//! holds silence for exactly as long as the incoming one needs to fill its
//! pipeline, and fades the incoming one up. The result is a brief dip rather
//! than a click, and the dip is as short as the incoming model allows.

/// State of a switch ramp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// No switch in progress; the active stage plays at unity.
    Idle,
    /// Fading the outgoing stage out.
    FadingOut,
    /// Silent while the incoming stage fills its pipeline.
    Priming,
    /// Fading the incoming stage in.
    FadingIn,
}

/// Produces the gain envelope for a switch, one sample at a time.
///
/// The caller multiplies its output by [`Self::next_gain`] and tells the ramp
/// when to start a switch. Which stage those samples come from is the caller's
/// business: it swaps stages when [`Self::wants_swap`] reports the fade-out has
/// finished.
#[derive(Debug)]
pub struct SwitchRamp {
    phase: Phase,
    /// Samples in each fade.
    fade_samples: usize,
    /// Samples remaining in the current phase.
    remaining: usize,
    /// Silence to hold once the fade-out completes.
    priming_samples: usize,
    /// Set when the fade-out finishes, cleared once the caller has swapped.
    swap_pending: bool,
}

impl SwitchRamp {
    /// Creates an idle ramp whose fades last `fade_samples`.
    #[must_use]
    pub const fn new(fade_samples: usize) -> Self {
        Self {
            phase: Phase::Idle,
            fade_samples,
            remaining: 0,
            priming_samples: 0,
            swap_pending: false,
        }
    }

    /// Length of each fade, in samples.
    #[must_use]
    pub const fn fade_samples(&self) -> usize {
        self.fade_samples
    }

    /// Whether a switch is in progress.
    #[must_use]
    pub const fn is_switching(&self) -> bool {
        !matches!(self.phase, Phase::Idle)
    }

    /// Begins a switch to a stage that needs `priming_samples` of silence
    /// before it produces anything.
    ///
    /// Starting a switch while one is in progress restarts the ramp from
    /// wherever the gain currently is, which keeps rapid clicking on the model
    /// picker from producing a click.
    pub const fn begin(&mut self, priming_samples: usize) {
        self.priming_samples = priming_samples;
        self.swap_pending = false;
        // Restart the fade-out from the current gain rather than from unity.
        self.remaining = match self.phase {
            Phase::FadingOut => self.remaining,
            Phase::FadingIn => self.fade_samples - self.remaining,
            _ => self.fade_samples,
        };
        self.phase = Phase::FadingOut;
        if self.remaining == 0 {
            self.finish_fade_out();
        }
    }

    /// Whether the caller should swap in the pending stage now.
    ///
    /// True for exactly one call, after the fade-out reaches zero.
    pub const fn wants_swap(&mut self) -> bool {
        core::mem::replace(&mut self.swap_pending, false)
    }

    /// Advances one sample and returns the gain to apply to it.
    pub fn next_gain(&mut self) -> f32 {
        match self.phase {
            Phase::Idle => 1.0,
            Phase::FadingOut => {
                self.remaining -= 1;
                let gain = raised_cosine(self.remaining, self.fade_samples);
                if self.remaining == 0 {
                    self.finish_fade_out();
                }
                gain
            }
            Phase::Priming => {
                self.remaining -= 1;
                if self.remaining == 0 {
                    self.phase = Phase::FadingIn;
                    self.remaining = self.fade_samples;
                }
                0.0
            }
            Phase::FadingIn => {
                self.remaining -= 1;
                let elapsed = self.fade_samples - self.remaining;
                let gain = raised_cosine(elapsed, self.fade_samples);
                if self.remaining == 0 {
                    self.phase = Phase::Idle;
                }
                gain
            }
        }
    }

    /// Moves from the fade-out into whichever phase comes next.
    const fn finish_fade_out(&mut self) {
        self.swap_pending = true;
        if self.priming_samples > 0 {
            self.phase = Phase::Priming;
            self.remaining = self.priming_samples;
        } else if self.fade_samples > 0 {
            self.phase = Phase::FadingIn;
            self.remaining = self.fade_samples;
        } else {
            self.phase = Phase::Idle;
        }
    }
}

/// A raised-cosine ramp: 0 at `position == 0`, 1 at `position == length`.
///
/// Smoother at both ends than a linear ramp, so the switch has no discontinuity
/// in slope either.
fn raised_cosine(position: usize, length: usize) -> f32 {
    if length == 0 {
        return 1.0;
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "fade lengths are a few thousand samples at most"
    )]
    let fraction = position as f32 / length as f32;
    0.5f32.mul_add(-(fraction * core::f32::consts::PI).cos(), 0.5)
}

#[cfg(test)]
mod tests {
    use super::SwitchRamp;

    /// Collects `count` gains, and how many samples in the swap was requested.
    fn run(ramp: &mut SwitchRamp, count: usize) -> (Vec<f32>, Option<usize>) {
        let mut gains = Vec::with_capacity(count);
        let mut swap_at = None;
        for index in 0..count {
            if ramp.wants_swap() {
                swap_at = Some(index);
            }
            gains.push(ramp.next_gain());
        }
        (gains, swap_at)
    }

    #[test]
    fn idle_ramp_is_unity() {
        let mut ramp = SwitchRamp::new(64);
        assert!(!ramp.is_switching());
        let (gains, swap) = run(&mut ramp, 16);
        assert!(gains.iter().all(|gain| (gain - 1.0).abs() < 1e-6));
        assert_eq!(swap, None);
    }

    #[test]
    fn a_switch_dips_to_silence_and_returns() {
        let fade = 32;
        let priming = 100;
        let mut ramp = SwitchRamp::new(fade);
        ramp.begin(priming);
        assert!(ramp.is_switching());

        let (gains, swap) = run(&mut ramp, fade + priming + fade + 8);

        // The swap is requested exactly when the fade-out reaches zero.
        assert_eq!(swap, Some(fade));
        assert!(gains[fade - 1].abs() < 1e-6, "fade-out did not reach zero");
        // Silence for the whole priming interval.
        for (offset, gain) in gains[fade..fade + priming].iter().enumerate() {
            assert!(gain.abs() < 1e-9, "priming sample {offset} was not silent");
        }
        // Back to unity afterwards, and staying there.
        assert!((gains[fade + priming + fade - 1] - 1.0).abs() < 1e-6);
        assert!(
            gains[fade + priming + fade..]
                .iter()
                .all(|g| (g - 1.0).abs() < 1e-6)
        );
        assert!(!ramp.is_switching());
    }

    #[test]
    fn the_ramp_is_monotonic_and_bounded() {
        let mut ramp = SwitchRamp::new(64);
        ramp.begin(32);
        let (gains, _) = run(&mut ramp, 64 + 32 + 64);
        assert!(gains.iter().all(|g| (0.0..=1.0).contains(g)));

        // Fading out never rises, fading in never falls.
        for pair in gains[..64].windows(2) {
            assert!(pair[1] <= pair[0] + 1e-6);
        }
        for pair in gains[64 + 32..].windows(2) {
            assert!(pair[1] >= pair[0] - 1e-6);
        }
    }

    #[test]
    fn no_step_exceeds_a_small_increment() {
        // The point of the ramp is that nothing clicks, which means no large
        // sample-to-sample jump anywhere in the envelope.
        let mut ramp = SwitchRamp::new(1_440);
        ramp.begin(2_400);
        let (gains, _) = run(&mut ramp, 1_440 + 2_400 + 1_440 + 100);
        for pair in gains.windows(2) {
            assert!(
                (pair[1] - pair[0]).abs() < 0.01,
                "gain jumped from {} to {}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn restarting_mid_fade_does_not_jump() {
        let mut ramp = SwitchRamp::new(64);
        ramp.begin(0);
        // Get partway through the fade-in.
        let (first, _) = run(&mut ramp, 64 + 20);
        let gain_before = *first.last().unwrap();

        ramp.begin(16);
        let (second, _) = run(&mut ramp, 4);
        assert!(
            (second[0] - gain_before).abs() < 0.05,
            "restart jumped from {gain_before} to {}",
            second[0]
        );
    }

    #[test]
    fn a_zero_length_fade_still_requests_the_swap() {
        let mut ramp = SwitchRamp::new(0);
        ramp.begin(0);
        assert!(ramp.wants_swap());
        assert!(!ramp.wants_swap());
        assert!(!ramp.is_switching());
        assert!((ramp.next_gain() - 1.0).abs() < 1e-6);
        assert_eq!(ramp.fade_samples(), 0);
    }
}
