//! See docs/adr/0007-dsp-stages.md#dry-wet-intensity-mix.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

const DRY_RING_CAPACITY: usize = 1 << 16;

/// See docs/adr/0007-dsp-stages.md#dry-wet-intensity-mix.
#[derive(Clone, Debug)]
pub struct IntensityControl {
    bits: Arc<AtomicU32>,
}

impl IntensityControl {
    /// See docs/adr/0007-dsp-stages.md#dry-wet-intensity-mix.
    #[must_use]
    pub fn new(initial: f32) -> Self {
        let control = Self {
            bits: Arc::new(AtomicU32::new(1.0_f32.to_bits())),
        };
        control.set(initial);
        control
    }

    /// See docs/adr/0007-dsp-stages.md#dry-wet-intensity-mix.
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
    fn default() -> Self {
        Self::new(1.0)
    }
}

#[derive(Debug)]
pub(crate) struct DryWetMixer {
    control: IntensityControl,
    smoothed: f32,
    ring: Box<[f32]>,
    write: usize,
}

impl DryWetMixer {
    pub(crate) fn new(control: IntensityControl) -> Self {
        let smoothed = control.get();
        Self {
            control,
            smoothed,
            ring: vec![0.0; DRY_RING_CAPACITY].into_boxed_slice(),
            write: 0,
        }
    }

    pub(crate) fn blend(&mut self, dry: &[f32], wet: &mut [f32], dry_delay: usize) {
        debug_assert_eq!(dry.len(), wet.len());
        if dry.is_empty() {
            return;
        }
        let target = self.control.get();
        let len = u16::try_from(dry.len()).unwrap_or(u16::MAX);
        let step = (target - self.smoothed) / f32::from(len);
        let delay = dry_delay.min(DRY_RING_CAPACITY - 1);
        let mask = DRY_RING_CAPACITY - 1;
        let mut intensity = self.smoothed;
        let ramp_lo = self.smoothed.min(target);
        let ramp_hi = self.smoothed.max(target);
        for (sample, out) in dry.iter().zip(wet.iter_mut()) {
            self.ring[self.write & mask] = *sample;
            let delayed = self.ring[self.write.wrapping_sub(delay) & mask];
            self.write = self.write.wrapping_add(1);
            intensity = (intensity + step).clamp(ramp_lo, ramp_hi);
            *out = intensity.mul_add(*out, (1.0 - intensity) * delayed);
        }
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
