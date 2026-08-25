//! WAV reading/writing at the engine rate.
//!
//! Input files of any rate/channel count are converted to mono 48 kHz f32
//! (channel average + FFT resampling). This is the offline path; quality
//! matters more than allocation behavior here.

use std::path::Path;

use anyhow::Context as _;
use audioadapter_buffers::owned::InterleavedOwned;
use noican_core::ENGINE_SAMPLE_RATE;
use rubato::{Fft, FixedSync, Resampler as _};

/// Reads a WAV file and converts it to mono 48 kHz f32.
///
/// # Errors
///
/// Fails on unreadable files, unsupported encodings, or resampler errors.
pub fn read_mono_48k(path: &Path) -> anyhow::Result<Vec<f32>> {
    let mut reader = hound::WavReader::open(path)
        .with_context(|| format!("cannot open WAV {}", path.display()))?;
    let spec = reader.spec();
    let channels = usize::from(spec.channels.max(1));

    let interleaved: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Float, 32) => reader
            .samples::<f32>()
            .collect::<Result<_, _>>()
            .context("bad float sample")?,
        (hound::SampleFormat::Int, bits @ 1..=32) => {
            #[allow(
                clippy::cast_precision_loss,
                reason = "power of two up to 2^31 is exactly representable in f64"
            )]
            let scale = (1_u64 << (bits - 1)) as f64;
            reader
                .samples::<i32>()
                .map(|s| {
                    s.map(|v| {
                        #[allow(
                            clippy::cast_possible_truncation,
                            reason = "normalized samples are within f32 range"
                        )]
                        let x = (f64::from(v) / scale) as f32;
                        x
                    })
                })
                .collect::<Result<_, _>>()
                .context("bad int sample")?
        }
        (format, bits) => anyhow::bail!("unsupported WAV encoding: {format:?} {bits} bit"),
    };

    // Mix down to mono.
    #[allow(
        clippy::cast_precision_loss,
        reason = "channel counts are tiny; exact f32 representation"
    )]
    let inv_channels = 1.0 / channels as f32;
    let mono: Vec<f32> = interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() * inv_channels)
        .collect();

    if spec.sample_rate == ENGINE_SAMPLE_RATE {
        return Ok(mono);
    }

    // Offline resample to the engine rate.
    let input_len = mono.len();
    let mut resampler = Fft::<f32>::new(
        spec.sample_rate as usize,
        ENGINE_SAMPLE_RATE as usize,
        1024,
        1,
        FixedSync::Input,
    )
    .context("failed to create resampler")?;
    let input = InterleavedOwned::new_from(mono, 1, input_len)
        .map_err(|e| anyhow::anyhow!("buffer wrap failed: {e}"))?;
    let needed = resampler.process_all_needed_output_len(input_len);
    let mut output = InterleavedOwned::<f32>::new(0.0, 1, needed);
    let (_, produced) = resampler
        .process_all_into_buffer(&input, &mut output, input_len, None)
        .map_err(|e| anyhow::anyhow!("resampling failed: {e}"))?;
    let mut data = output.take_data();
    data.truncate(produced);
    Ok(data)
}

/// Writes mono 48 kHz f32 samples as a 16-bit PCM WAV (the most portable
/// format for listening tests).
///
/// # Errors
///
/// Fails when the file cannot be written.
pub fn write_mono_48k(path: &Path, samples: &[f32]) -> anyhow::Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: ENGINE_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .with_context(|| format!("cannot create WAV {}", path.display()))?;
    for &sample in samples {
        #[allow(
            clippy::cast_possible_truncation,
            reason = "value is clamped to i16 range before the cast"
        )]
        let value = (f64::from(sample.clamp(-1.0, 1.0)) * f64::from(i16::MAX)).round() as i16;
        writer.write_sample(value)?;
    }
    writer.finalize()?;
    Ok(())
}
