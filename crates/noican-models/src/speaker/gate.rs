//! The gate's decision and gain logic, independent of any model.
//!
//! Separated from the stage so it can be tested by feeding it similarity scores
//! directly. The interesting behaviour — hysteresis, holding through silence,
//! and a ramp that never steps — has nothing to do with inference, and a test
//! that needed a 30 MB download to run would not get written.

/// Whether the gate is currently passing audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateState {
    /// Passing at unity.
    Open,
    /// Attenuating.
    Closed,
}

/// Thresholds and ramp shape.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GateConfig {
    /// Similarity at or above which a shut gate opens.
    pub open_above: f32,
    /// Similarity below which an open gate shuts. Must be below `open_above`,
    /// which is what makes the decision hysteretic.
    pub close_below: f32,
    /// Linear gain applied when shut.
    pub closed_gain: f32,
    /// Gain change permitted per sample.
    pub ramp_step: f32,
}

impl GateConfig {
    /// The shipped configuration, for audio at `sample_rate`.
    ///
    /// Same-speaker windows score around 0.42 at the shortest usable window and
    /// different-speaker windows around 0.02, so any threshold between them
    /// separates the two. The pair below sits nearer the bottom of that range
    /// because the two errors do not cost the same: wrongly gating the user is
    /// much worse than passing a second of somebody else.
    ///
    /// The shut gain attenuates by about 24 dB rather than muting. A hard mute
    /// is indistinguishable from a dropout, and a gate users cannot tell apart
    /// from a bug gets switched off.
    #[must_use]
    pub fn recommended(sample_rate: u32) -> Self {
        #[expect(
            clippy::cast_precision_loss,
            reason = "audio sample rates are exact in f32"
        )]
        let rate = sample_rate as f32;
        Self {
            open_above: 0.25,
            close_below: 0.15,
            closed_gain: 0.063,
            // 150 ms for the full range: long enough not to click, short enough
            // to matter.
            ramp_step: 1.0 / (rate * 0.15),
        }
    }
}

/// Tracks the gate's state and walks its gain towards the target.
#[derive(Debug)]
pub struct Gate {
    config: GateConfig,
    state: GateState,
    gain: f32,
    similarity: f32,
}

impl Gate {
    /// Builds a gate that starts open, so a gate whose model has not decided
    /// anything yet is transparent rather than silent.
    #[must_use]
    pub const fn new(config: GateConfig) -> Self {
        Self {
            config,
            state: GateState::Open,
            gain: 1.0,
            similarity: 0.0,
        }
    }

    /// The current state.
    #[must_use]
    pub const fn state(&self) -> GateState {
        self.state
    }

    /// The similarity behind the current state.
    #[must_use]
    pub const fn similarity(&self) -> f32 {
        self.similarity
    }

    /// The gain in effect right now, mid-ramp included.
    #[must_use]
    pub const fn gain(&self) -> f32 {
        self.gain
    }

    /// Updates the state from a new similarity score.
    pub fn observe(&mut self, similarity: f32) {
        self.similarity = similarity;
        self.state = match self.state {
            GateState::Open if similarity < self.config.close_below => GateState::Closed,
            GateState::Closed if similarity >= self.config.open_above => GateState::Open,
            unchanged => unchanged,
        };
    }

    /// Copies `input` to `output`, ramping the gain towards the target.
    ///
    /// Panics in debug builds if the slices differ in length; the stage checks
    /// that before calling.
    pub fn apply(&mut self, input: &[f32], output: &mut [f32]) {
        debug_assert_eq!(input.len(), output.len());
        let target = match self.state {
            GateState::Open => 1.0,
            GateState::Closed => self.config.closed_gain,
        };
        for (out, sample) in output.iter_mut().zip(input) {
            if self.gain < target {
                self.gain = (self.gain + self.config.ramp_step).min(target);
            } else if self.gain > target {
                self.gain = (self.gain - self.config.ramp_step).max(target);
            }
            *out = sample * self.gain;
        }
    }

    /// Returns the gate to its initial open, unity-gain state.
    pub const fn reset(&mut self) {
        self.state = GateState::Open;
        self.gain = 1.0;
        self.similarity = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::{Gate, GateConfig, GateState};

    const RATE: u32 = 16_000;

    fn gate() -> Gate {
        Gate::new(GateConfig::recommended(RATE))
    }

    #[test]
    fn a_new_gate_is_transparent() {
        let mut gate = gate();
        assert_eq!(gate.state(), GateState::Open);
        let input = [0.5f32; 64];
        let mut output = [0.0f32; 64];
        gate.apply(&input, &mut output);
        for (out, sample) in output.iter().zip(&input) {
            assert!(
                (out - sample).abs() < 1e-6,
                "a fresh gate altered the signal"
            );
        }
    }

    /// The whole point of two thresholds: a score between them changes nothing,
    /// so a borderline speaker does not make the gate chatter.
    #[test]
    fn a_score_between_the_thresholds_holds_the_current_state() {
        let config = GateConfig::recommended(RATE);
        let between = f32::midpoint(config.close_below, config.open_above);

        let mut gate = gate();
        gate.observe(between);
        assert_eq!(gate.state(), GateState::Open, "an open gate closed");

        gate.observe(0.0);
        assert_eq!(gate.state(), GateState::Closed);
        gate.observe(between);
        assert_eq!(gate.state(), GateState::Closed, "a closed gate opened");

        gate.observe(0.9);
        assert_eq!(gate.state(), GateState::Open);
    }

    #[test]
    fn the_gain_reaches_its_target_and_stays() {
        let config = GateConfig::recommended(RATE);
        let mut gate = gate();
        gate.observe(0.0);

        // The ramp spans 150 ms; run twice that and it must have arrived.
        let input = vec![1.0f32; RATE as usize / 2];
        let mut output = vec![0.0f32; input.len()];
        gate.apply(&input, &mut output);
        assert!(
            (gate.gain() - config.closed_gain).abs() < 1e-6,
            "gain stalled at {}",
            gate.gain()
        );

        gate.apply(&input, &mut output);
        assert!(
            (gate.gain() - config.closed_gain).abs() < 1e-6,
            "gain overshot its target"
        );

        gate.observe(1.0);
        gate.apply(&input, &mut output);
        assert!(
            (gate.gain() - 1.0).abs() < 1e-6,
            "gain did not return to unity"
        );
    }

    /// A gain that jumps is a click. Every step has to be within the configured
    /// increment, across the whole transition.
    #[test]
    fn the_gain_never_steps_further_than_the_ramp_allows() {
        let config = GateConfig::recommended(RATE);
        let mut gate = gate();
        let input = vec![1.0f32; 4_000];
        let mut output = vec![0.0f32; input.len()];

        let mut previous = 1.0f32;
        for round in 0..8 {
            gate.observe(if round % 2 == 0 { 0.0 } else { 1.0 });
            gate.apply(&input, &mut output);
            for value in &output {
                // Slack for f32 accumulation: the gain is built by thousands
                // of additions, so a single step's measured size drifts a
                // little from the configured one.
                assert!(
                    (value - previous).abs() <= config.ramp_step + 1e-6,
                    "gain jumped from {previous} to {value}"
                );
                previous = *value;
            }
        }
    }

    /// Attenuation, not silence: a shut gate has to still pass something, or it
    /// is indistinguishable from a dropout.
    #[test]
    fn a_shut_gate_attenuates_without_muting() {
        let config = GateConfig::recommended(RATE);
        let decibels = 20.0 * config.closed_gain.log10();
        assert!(
            (-30.0..=-18.0).contains(&decibels),
            "the shut gate sits at {decibels} dB"
        );

        let mut gate = gate();
        gate.observe(0.0);
        let input = vec![0.5f32; RATE as usize / 2];
        let mut output = vec![0.0f32; input.len()];
        gate.apply(&input, &mut output);

        let tail = &output[output.len() - 100..];
        assert!(
            tail.iter().all(|value| value.abs() > 0.0),
            "the gate muted rather than attenuated"
        );
        let expected = 0.5 * config.closed_gain;
        assert!((tail[99] - expected).abs() < 1e-6);
    }

    #[test]
    fn reset_returns_the_gate_to_transparent() {
        let mut gate = gate();
        gate.observe(0.0);
        let input = vec![1.0f32; RATE as usize / 2];
        let mut output = vec![0.0f32; input.len()];
        gate.apply(&input, &mut output);
        assert_eq!(gate.state(), GateState::Closed);

        gate.reset();
        assert_eq!(gate.state(), GateState::Open);
        assert!((gate.gain() - 1.0).abs() < f32::EPSILON);
        assert!(gate.similarity().abs() < f32::EPSILON);
    }

    #[test]
    fn the_recommended_thresholds_are_hysteretic_at_every_rate() {
        for rate in [16_000, 44_100, 48_000] {
            let config = GateConfig::recommended(rate);
            assert!(
                config.close_below < config.open_above,
                "thresholds at {rate} Hz are not hysteretic"
            );
            assert!(config.ramp_step > 0.0);
            #[expect(clippy::cast_precision_loss, reason = "test fixture")]
            let samples = 1.0 / config.ramp_step / rate as f32;
            assert!(
                (0.05..=0.5).contains(&samples),
                "the ramp at {rate} Hz takes {samples} s"
            );
        }
    }
}
