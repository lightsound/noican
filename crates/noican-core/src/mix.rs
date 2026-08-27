//! Dry/wet intensity blending with a latency-compensated dry path.
//!
//! The intensity ("strength") control blends the processed signal (wet)
//! with the raw input (dry): 100% is fully processed, 0% is the raw
//! microphone. The blend runs on the inference thread inside
//! [`crate::SwitchingEngine::process_block`] — upstream of the output
//! ring and the preview monitor tee, so every consumer hears the same
//! mix — and obeys the real-time rules (docs/tech-research.md §9):
//!
//! - The intensity value crosses threads through **one atomic**
//!   ([`IntensityControl`], an `f32` bit pattern in an `AtomicU32`).
//!   Changing it never rebuilds or locks anything.
//! - The blend itself performs no allocation and no locking: the dry
//!   history lives in a ring buffer preallocated at construction time
//!   (control plane), and per-block work is a fixed multiply-add pass.
//!
//! Naively summing dry and wet would comb-filter (double voice): the wet
//! signal carries the stage's algorithmic and resampling latency
//! (10–30 ms class). The mixer therefore delays the dry signal by the
//! active stage's [`crate::Stage::latency_samples`] before blending, so
//! both paths line up. Intensity changes are additionally smoothed with
//! a per-block linear ramp, so a slider drag never steps ("zipper"
//! noise).

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

/// Capacity of the dry-compensation ring in samples at the engine rate.
/// A power of two (mask indexing, no division on the audio path) covering
/// ~1.37 s at 48 kHz — an order of magnitude above any streaming stage's
/// reported latency. Larger reported latencies are clamped to this bound
/// (partial compensation instead of an out-of-range read).
const DRY_RING_CAPACITY: usize = 1 << 16;

/// Control-plane handle to the dry/wet intensity: one shared atomic.
///
/// `1.0` (the default) is fully processed output; `0.0` is the raw
/// microphone. Cloning shares the same underlying value, so the control
/// plane keeps one handle while the inference thread's mixer reads the
/// other — no lock, no rebuild, safe to update at UI slider rates.
#[derive(Clone, Debug)]
pub struct IntensityControl {
    bits: Arc<AtomicU32>,
}

impl IntensityControl {
    /// Creates a control initialized to `initial` (clamped to `0.0..=1.0`;
    /// a non-finite value falls back to fully processed).
    #[must_use]
    pub fn new(initial: f32) -> Self {
        let control = Self {
            bits: Arc::new(AtomicU32::new(1.0_f32.to_bits())),
        };
        control.set(initial);
        control
    }

    /// Publishes a new intensity, clamped to `0.0..=1.0`. Non-finite
    /// values are ignored (the previous intensity stays), so a corrupt
    /// caller value can never poison the audio path. One atomic store —
    /// never blocks, safe from any thread.
    pub fn set(&self, intensity: f32) {
        if !intensity.is_finite() {
            return;
        }
        self.bits
            .store(intensity.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    /// Reads the current intensity (`0.0..=1.0`). One atomic load —
    /// never blocks, safe from any thread including the inference worker.
    #[must_use]
    pub fn get(&self) -> f32 {
        f32::from_bits(self.bits.load(Ordering::Relaxed))
    }
}

impl Default for IntensityControl {
    /// Fully processed (100%): the engine behaves exactly as it did
    /// before the intensity control existed until someone moves it.
    fn default() -> Self {
        Self::new(1.0)
    }
}

/// Inference-thread side of the intensity control: blends the delayed dry
/// signal into the wet block in place.
///
/// Owned by [`crate::SwitchingEngine`]; not constructed on the audio
/// path (the ring allocation happens on the control plane).
#[derive(Debug)]
pub(crate) struct DryWetMixer {
    control: IntensityControl,
    /// Intensity the previous block ended on; each block ramps linearly
    /// from here to the freshly read target, so changes never step.
    smoothed: f32,
    /// Dry history ring; length is [`DRY_RING_CAPACITY`] (power of two).
    ring: Box<[f32]>,
    /// Monotonic write index (wrapped by masking on access).
    write: usize,
}

impl DryWetMixer {
    /// Creates a mixer reading `control`, with a silent dry history.
    pub(crate) fn new(control: IntensityControl) -> Self {
        let smoothed = control.get();
        Self {
            control,
            smoothed,
            ring: vec![0.0; DRY_RING_CAPACITY].into_boxed_slice(),
            write: 0,
        }
    }

    /// Blends `dry` (the raw engine-rate input) into `wet` in place:
    /// `wet[i] = w * wet[i] + (1 - w) * dry[i - dry_delay]`, with `w`
    /// ramping linearly across the block toward the control's value.
    ///
    /// `dry_delay` is the active stage's reported latency in samples;
    /// it aligns the dry path with the wet path so a partial mix does
    /// not comb-filter. The dry history is fed continuously (even at
    /// 100% wet), so lowering the intensity mid-stream blends against
    /// real history, not silence. Real-time safe: no allocation, no
    /// locks, one atomic read per block.
    pub(crate) fn blend(&mut self, dry: &[f32], wet: &mut [f32], dry_delay: usize) {
        debug_assert_eq!(dry.len(), wet.len());
        if dry.is_empty() {
            return;
        }
        let target = self.control.get();
        // Lossless `u16 → f32` ramp arithmetic, mirroring `switch::ratio`
        // (blocks are 10 ms; anything above 65535 samples still ramps,
        // just marginally faster than one block).
        let len = u16::try_from(dry.len()).unwrap_or(u16::MAX);
        let step = (target - self.smoothed) / f32::from(len);
        let delay = dry_delay.min(DRY_RING_CAPACITY - 1);
        let mask = DRY_RING_CAPACITY - 1;
        let mut intensity = self.smoothed;
        for (sample, out) in dry.iter().zip(wet.iter_mut()) {
            self.ring[self.write & mask] = *sample;
            let delayed = self.ring[self.write.wrapping_sub(delay) & mask];
            self.write = self.write.wrapping_add(1);
            intensity += step;
            // At intensity 1.0 this is exactly `wet` (adding a true zero),
            // so full strength is bit-identical to the pre-mixer engine.
            *out = intensity.mul_add(*out, (1.0 - intensity) * delayed);
        }
        // Land exactly on the target: the per-sample accumulation may
        // carry rounding, and the next block must start from the truth.
        self.smoothed = target;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(len: usize) -> Vec<f32> {
        #[expect(clippy::cast_precision_loss, reason = "test signal indices are small")]
        (0..len).map(|n| n as f32 * 1e-3).collect()
    }

    #[test]
    fn control_defaults_to_full_and_clamps() {
        let control = IntensityControl::default();
        assert!((control.get() - 1.0).abs() < f32::EPSILON);
        control.set(0.25);
        assert!((control.get() - 0.25).abs() < f32::EPSILON);
        control.set(7.0);
        assert!((control.get() - 1.0).abs() < f32::EPSILON, "clamps high");
        control.set(-3.0);
        assert!(control.get().abs() < f32::EPSILON, "clamps low");
    }

    #[test]
    fn non_finite_values_are_ignored() {
        let control = IntensityControl::new(0.5);
        control.set(f32::NAN);
        assert!((control.get() - 0.5).abs() < f32::EPSILON);
        control.set(f32::INFINITY);
        assert!((control.get() - 0.5).abs() < f32::EPSILON);
        assert!(
            (IntensityControl::new(f32::NAN).get() - 1.0).abs() < f32::EPSILON,
            "a non-finite initial value falls back to fully processed"
        );
    }

    #[test]
    fn full_intensity_is_bit_identical_wet() {
        let mut mixer = DryWetMixer::new(IntensityControl::default());
        let dry = ramp(480);
        let wet_reference: Vec<f32> = dry.iter().map(|s| s * -2.0).collect();
        let mut wet = wet_reference.clone();
        mixer.blend(&dry, &mut wet, 100);
        assert_eq!(wet, wet_reference, "100% must not alter the wet path");
    }

    #[test]
    fn zero_intensity_is_the_delayed_dry_signal() {
        let mut mixer = DryWetMixer::new(IntensityControl::new(0.0));
        let delay = 37;
        let dry = ramp(480);
        let mut wet = vec![0.5_f32; 480];
        mixer.blend(&dry, &mut wet, delay);
        for (n, out) in wet.iter().enumerate() {
            let expected = if n < delay { 0.0 } else { dry[n - delay] };
            assert!(
                (out - expected).abs() < 1e-6,
                "sample {n}: {out} != {expected}"
            );
        }
    }

    #[test]
    fn half_intensity_blends_aligned_paths() {
        // Wet is the dry signal negated and delayed like a real stage;
        // with compensation the two cancel exactly at 50%.
        let delay = 64;
        let mut mixer = DryWetMixer::new(IntensityControl::new(0.5));
        let dry = ramp(960);
        let mut wet = vec![0.0_f32; 960];
        for n in delay..960 {
            wet[n] = -dry[n - delay];
        }
        mixer.blend(&dry, &mut wet, delay);
        for (n, out) in wet.iter().enumerate().skip(delay) {
            assert!(out.abs() < 1e-6, "sample {n} should cancel, got {out}");
        }
    }

    #[test]
    fn intensity_changes_ramp_without_steps() {
        let control = IntensityControl::new(1.0);
        let mut mixer = DryWetMixer::new(control.clone());
        // DC dry vs. silent wet: the output *is* the (1 − w) curve.
        let dry = [1.0_f32; 480];
        let mut previous_tail = 0.0;
        let mut outputs = Vec::new();
        for target in [1.0, 0.0, 1.0] {
            control.set(target);
            let mut wet = [0.0_f32; 480];
            mixer.blend(&dry, &mut wet, 0);
            outputs.push((previous_tail, wet));
            previous_tail = wet[479];
        }
        for (entry, wet) in &outputs {
            let mut last = *entry;
            for sample in wet {
                // Bound: one nominal ramp step plus per-sample float
                // accumulation slack.
                assert!(
                    (sample - last).abs() <= 1.0 / 480.0 + 1e-4,
                    "zipper step {last} -> {sample}"
                );
                last = *sample;
            }
        }
    }

    #[test]
    fn oversized_delay_is_clamped_not_out_of_range() {
        let mut mixer = DryWetMixer::new(IntensityControl::new(0.0));
        let dry = ramp(480);
        let mut wet = vec![0.0_f32; 480];
        mixer.blend(&dry, &mut wet, usize::MAX);
        assert!(wet.iter().all(|s| s.is_finite()));
        assert!(
            wet.iter().all(|s| s.abs() < f32::EPSILON),
            "history beyond the clamped delay is silence"
        );
    }

    #[test]
    fn dry_history_is_fed_even_at_full_intensity() {
        let control = IntensityControl::new(1.0);
        let mut mixer = DryWetMixer::new(control.clone());
        let dry = vec![0.75_f32; 480];
        let mut wet = vec![0.0_f32; 480];
        mixer.blend(&dry, &mut wet, 0);
        // Drop to raw: the very next block must read real history, not
        // the silence the ring was born with.
        control.set(0.0);
        let mut wet = vec![0.0_f32; 480];
        mixer.blend(&dry, &mut wet, 240);
        assert!(
            (wet[479] - 0.75).abs() < 1e-4,
            "delayed dry must come from fed history, got {}",
            wet[479]
        );
    }
}
