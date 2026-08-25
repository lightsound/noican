//! Mono `f32` WAV reading and writing.
//!
//! The engine is mono throughout, so multi-channel input is downmixed on read
//! and written back as mono. That is deliberate: the pipeline is single-channel
//! by platform constraint — macOS exposes the built-in microphone array as one
//! processed stream and no public API returns the raw channels
//! (`docs/tech-research.md` §4.1) — so a stereo comparison file would be
//! comparing something the live path can never receive.

use std::path::Path;

use anyhow::{Context as _, Result, bail};

/// A decoded mono clip.
#[derive(Debug, Clone)]
pub(crate) struct Clip {
    /// Samples in `[-1.0, 1.0]`.
    pub(crate) samples: Vec<f32>,
    /// Sample rate in hertz.
    pub(crate) sample_rate: u32,
}

impl Clip {
    /// Duration in seconds.
    #[must_use]
    pub(crate) fn duration_seconds(&self) -> f64 {
        #[expect(
            clippy::cast_precision_loss,
            reason = "a clip long enough to exceed f64's exact-integer range would be 5000 years                       of audio"
        )]
        let samples = self.samples.len() as f64;
        samples / f64::from(self.sample_rate)
    }

    /// Peak absolute sample value.
    #[must_use]
    pub(crate) fn peak(&self) -> f32 {
        self.samples
            .iter()
            .fold(0.0f32, |peak, sample| peak.max(sample.abs()))
    }

    /// Root-mean-square level in decibels relative to full scale.
    ///
    /// Returns `None` for an empty or entirely silent clip, where the value is
    /// undefined rather than very negative.
    #[must_use]
    pub(crate) fn rms_dbfs(&self) -> Option<f32> {
        if self.samples.is_empty() {
            return None;
        }
        let sum: f64 = self
            .samples
            .iter()
            .map(|sample| f64::from(*sample) * f64::from(*sample))
            .sum();
        #[expect(
            clippy::cast_precision_loss,
            reason = "see duration_seconds: sample counts are far below f64's exact-integer limit"
        )]
        let count = self.samples.len() as f64;
        let mean = sum / count;
        if mean <= 0.0 {
            return None;
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a decibel level is reported to the user, where f32 precision is ample"
        )]
        let db = (10.0 * mean.log10()) as f32;
        Some(db)
    }
}

/// Reads `path`, downmixing to mono and converting to `f32`.
///
/// # Errors
///
/// Returns an error if the file cannot be opened, is not a WAV file, or uses a
/// sample format `hound` cannot decode.
pub(crate) fn read(path: &Path) -> Result<Clip> {
    let mut reader =
        hound::WavReader::open(path).with_context(|| format!("opening {}", path.display()))?;
    let spec = reader.spec();
    if spec.channels == 0 {
        bail!("{} declares zero channels", path.display());
    }

    let interleaved: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Float, 32) => reader
            .samples::<f32>()
            .collect::<std::result::Result<_, _>>()
            .with_context(|| format!("decoding float samples from {}", path.display()))?,
        (hound::SampleFormat::Int, bits) => {
            if bits == 0 || bits > 32 {
                bail!("{} declares {bits} bits per sample", path.display());
            }
            // Derived in f64: at 32 bits the divisor is 2^31, which overflows
            // every integer type narrower than the sample itself. Getting this
            // wrong by a sign inverts the whole file, which is inaudible on its
            // own and only shows up when comparing against another decoder.
            #[expect(
                clippy::cast_possible_truncation,
                reason = "powers of two up to 2^31 are exactly representable in f32"
            )]
            let scale = (1.0 / 2f64.powi(i32::from(bits) - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|sample| {
                    sample.map(|value| {
                        #[expect(
                            clippy::cast_precision_loss,
                            reason = "24-bit and smaller samples are exact; 32-bit input is \
                                      rounded to f32, which is the working precision anyway"
                        )]
                        let value = value as f32;
                        value * scale
                    })
                })
                .collect::<std::result::Result<_, _>>()
                .with_context(|| format!("decoding integer samples from {}", path.display()))?
        }
        (format, bits) => bail!(
            "{} uses an unsupported sample format ({format:?}, {bits} bits)",
            path.display()
        ),
    };

    let channels = usize::from(spec.channels);
    let samples = if channels == 1 {
        interleaved
    } else {
        #[expect(
            clippy::cast_precision_loss,
            reason = "channel counts are tiny integers, exact in f32"
        )]
        let inverse = 1.0 / channels as f32;
        interleaved
            .chunks(channels)
            .map(|frame| frame.iter().sum::<f32>() * inverse)
            .collect()
    };

    Ok(Clip {
        samples,
        sample_rate: spec.sample_rate,
    })
}

/// Writes `clip` to `path` as mono 32-bit float.
///
/// Float output avoids the quantisation and clipping that a 16-bit write would
/// introduce, which matters when the point of the file is to be listened to
/// against another one.
///
/// # Errors
///
/// Returns an error if the file cannot be created or written.
pub(crate) fn write(path: &Path, clip: &Clip) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: clip.sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .with_context(|| format!("creating {}", path.display()))?;
    for &sample in &clip.samples {
        writer
            .write_sample(sample)
            .with_context(|| format!("writing {}", path.display()))?;
    }
    writer
        .finalize()
        .with_context(|| format!("finalizing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::{Clip, read, write};

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("noican-wav-test-{name}.wav"))
    }

    #[test]
    fn round_trips_a_mono_clip() {
        let path = temp_path("mono");
        let clip = Clip {
            samples: vec![0.0, 0.5, -0.5, 0.25],
            sample_rate: 48_000,
        };
        write(&path, &clip).unwrap();
        let read_back = read(&path).unwrap();
        assert_eq!(read_back.sample_rate, 48_000);
        for (a, b) in read_back.samples.iter().zip(&clip.samples) {
            assert!((a - b).abs() < 1e-7);
        }
    }

    #[test]
    fn downmixes_a_stereo_file() {
        let path = temp_path("stereo");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 16_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for &sample in &[1.0f32, 0.0, 0.5, 0.5] {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();

        let clip = read(&path).unwrap();
        assert_eq!(clip.samples.len(), 2);
        assert!((clip.samples[0] - 0.5).abs() < 1e-6);
        assert!((clip.samples[1] - 0.5).abs() < 1e-6);
    }

    /// Polarity as well as magnitude: an inverted decode is inaudible in
    /// isolation and only shows up when a file is compared against another
    /// decoder's output, so it has to be asserted here.
    #[test]
    fn decodes_sixteen_bit_integers_with_the_right_polarity() {
        let path = temp_path("i16");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for &sample in &[0i16, 16_384, -16_384, i16::MAX] {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();

        let clip = read(&path).unwrap();
        assert!(clip.samples[0].abs() < 1e-6);
        assert!(
            (clip.samples[1] - 0.5).abs() < 1e-6,
            "got {}",
            clip.samples[1]
        );
        assert!(
            (clip.samples[2] + 0.5).abs() < 1e-6,
            "got {}",
            clip.samples[2]
        );
        assert!(
            (clip.samples[3] - 1.0).abs() < 1e-4,
            "got {}",
            clip.samples[3]
        );
    }

    #[test]
    fn decodes_twenty_four_bit_integers_with_the_right_polarity() {
        let path = temp_path("i24");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 24,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for &sample in &[0i32, 1 << 22, -(1 << 22)] {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();

        let clip = read(&path).unwrap();
        assert!(
            (clip.samples[1] - 0.5).abs() < 1e-6,
            "got {}",
            clip.samples[1]
        );
        assert!(
            (clip.samples[2] + 0.5).abs() < 1e-6,
            "got {}",
            clip.samples[2]
        );
    }

    #[test]
    fn reports_level_statistics() {
        let clip = Clip {
            samples: vec![0.5, -0.5, 0.5, -0.5],
            sample_rate: 48_000,
        };
        assert!((clip.peak() - 0.5).abs() < 1e-6);
        // An RMS of 0.5 is -6.02 dBFS.
        let rms = clip.rms_dbfs().unwrap();
        assert!((rms + 6.02).abs() < 0.05, "rms = {rms}");
        assert!((clip.duration_seconds() - 4.0 / 48_000.0).abs() < 1e-12);

        let silent = Clip {
            samples: vec![0.0; 8],
            sample_rate: 48_000,
        };
        assert!(silent.rms_dbfs().is_none());
        let empty = Clip {
            samples: Vec::new(),
            sample_rate: 48_000,
        };
        assert!(empty.rms_dbfs().is_none());
    }

    #[test]
    fn rejects_a_non_wav_file() {
        let path = temp_path("garbage");
        std::fs::write(&path, b"not a wav").unwrap();
        assert!(read(&path).is_err());
    }
}
