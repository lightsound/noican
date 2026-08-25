//! WAV decoding, mono downmixing, and float output.

use std::path::Path;

use anyhow::{bail, Context, Result};
use hound::{SampleFormat, WavReader, WavSpec, WavWriter};

/// Decoded mono audio.
pub struct Audio {
    /// Samples normalized to `[-1, 1]`.
    pub samples: Vec<f32>,
    /// Source sample rate.
    pub sample_rate: u32,
}

/// Read PCM integer or 32-bit float WAV and downmix every channel equally.
pub fn read_mono(path: &Path) -> Result<Audio> {
    let mut reader =
        WavReader::open(path).with_context(|| format!("open WAV {}", path.display()))?;
    let specification = reader.spec();
    if specification.channels == 0 {
        bail!("WAV {} declares zero channels", path.display());
    }
    let interleaved = match specification.sample_format {
        SampleFormat::Float => {
            if specification.bits_per_sample != 32 {
                bail!(
                    "WAV {} uses unsupported {}-bit float samples",
                    path.display(),
                    specification.bits_per_sample
                );
            }
            reader
                .samples::<f32>()
                .collect::<Result<Vec<_>, _>>()
                .with_context(|| format!("decode float WAV {}", path.display()))?
        }
        SampleFormat::Int => {
            let bits = specification.bits_per_sample;
            if !(1..=32).contains(&bits) {
                bail!(
                    "WAV {} uses unsupported {}-bit integer samples",
                    path.display(),
                    bits
                );
            }
            let scale = 2_f32.powi(i32::from(bits) - 1);
            reader
                .samples::<i32>()
                .map(|sample| sample.map(|value| value as f32 / scale))
                .collect::<Result<Vec<_>, _>>()
                .with_context(|| format!("decode integer WAV {}", path.display()))?
        }
    };
    if interleaved.iter().any(|sample| !sample.is_finite()) {
        bail!("WAV {} contains a non-finite sample", path.display());
    }
    let channels = usize::from(specification.channels);
    if !interleaved.len().is_multiple_of(channels) {
        bail!("WAV {} ends inside an interleaved frame", path.display());
    }
    let channel_scale = f32::from(specification.channels);
    let samples = interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channel_scale)
        .collect();
    Ok(Audio {
        samples,
        sample_rate: specification.sample_rate,
    })
}

/// Write mono 48 kHz-compatible float WAV samples.
pub fn write_mono(path: &Path, sample_rate: u32, samples: &[f32]) -> Result<()> {
    if samples.iter().any(|sample| !sample.is_finite()) {
        bail!("refusing to write non-finite audio to {}", path.display());
    }
    let specification = WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };
    let mut writer = WavWriter::create(path, specification)
        .with_context(|| format!("create WAV {}", path.display()))?;
    for sample in samples {
        writer
            .write_sample(sample.clamp(-1.0, 1.0))
            .with_context(|| format!("write WAV {}", path.display()))?;
    }
    writer
        .finalize()
        .with_context(|| format!("finalize WAV {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_integer_input_is_downmixed() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("stereo.wav");
        let specification = WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut writer = WavWriter::create(&path, specification)?;
        writer.write_sample(16_384_i16)?;
        writer.write_sample(-16_384_i16)?;
        writer.finalize()?;

        let audio = read_mono(&path)?;
        assert_eq!(audio.sample_rate, 48_000);
        assert_eq!(audio.samples.len(), 1);
        assert!(audio.samples[0].abs() <= f32::EPSILON);
        Ok(())
    }
}
