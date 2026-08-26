//! Audio file reading/writing at the engine rate.
//!
//! Input files of any rate/channel count are converted to mono 48 kHz f32
//! (channel average + FFT resampling). WAV is read with `hound` (the
//! long-verified path); AIFF/AIFC, CAF, and M4A (AAC/ALAC) are read with
//! `symphonia`. Output is always 16-bit WAV. This is the offline path;
//! quality matters more than allocation behavior here.
//!
//! Some compressed AIFC variants (e.g. IMA4) are not decodable here;
//! convert those with `afconvert` first (see the README).

use std::fs::File;
use std::path::Path;

use anyhow::Context as _;
use audioadapter_buffers::owned::InterleavedOwned;
use noican_core::ENGINE_SAMPLE_RATE;
use rubato::{Fft, FixedSync, Resampler as _};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Reads an audio file (WAV, AIFF/AIFC, CAF, or M4A) and converts it to
/// mono 48 kHz f32.
///
/// # Errors
///
/// Fails on unreadable files, unsupported encodings, or resampler errors.
pub(crate) fn read_mono_48k(path: &Path) -> anyhow::Result<Vec<f32>> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    let (mono, sample_rate) = if extension.as_deref() == Some("wav") {
        read_wav(path)?
    } else {
        read_symphonia(path, extension.as_deref())?
    };
    to_engine_rate(mono, sample_rate)
}

/// Reads a WAV file via `hound`, returning interleaved-averaged mono
/// samples at the file's native rate.
fn read_wav(path: &Path) -> anyhow::Result<(Vec<f32>, u32)> {
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
            #[expect(
                clippy::cast_precision_loss,
                reason = "power of two up to 2^31 is exactly representable in f64"
            )]
            let scale = (1_u64 << (bits - 1)) as f64;
            reader
                .samples::<i32>()
                .map(|s| {
                    s.map(|v| {
                        #[expect(
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

    Ok((mix_to_mono(&interleaved, channels), spec.sample_rate))
}

/// Reads AIFF/AIFC, CAF, or M4A via `symphonia`, returning
/// interleaved-averaged mono samples at the file's native rate.
fn read_symphonia(path: &Path, extension: Option<&str>) -> anyhow::Result<(Vec<f32>, u32)> {
    let file = File::open(path).with_context(|| format!("cannot open audio {}", path.display()))?;
    let stream = MediaSourceStream::new(
        Box::new(file),
        symphonia::core::io::MediaSourceStreamOptions::default(),
    );
    let mut hint = Hint::new();
    if let Some(extension) = extension {
        hint.with_extension(extension);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            stream,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .with_context(|| {
            format!(
                "unsupported container/encoding in {} — convert it with \
                 `afconvert -f WAVE -d LEI16@48000 <in> out.wav` first (see README)",
                path.display()
            )
        })?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .with_context(|| format!("no decodable audio track in {}", path.display()))?;
    let track_id = track.id;
    let sample_rate = track
        .codec_params
        .sample_rate
        .with_context(|| format!("missing sample rate in {}", path.display()))?;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .with_context(|| {
            format!(
                "unsupported codec in {} — convert it with \
                 `afconvert -f WAVE -d LEI16@48000 <in> out.wav` first (see README)",
                path.display()
            )
        })?;

    let mut mono = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(error))
                if error.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(error) => {
                return Err(anyhow::Error::new(error))
                    .with_context(|| format!("reading {}", path.display()));
            }
        };
        if packet.track_id() != track_id {
            continue;
        }
        let audio_buf = decoder
            .decode(&packet)
            .with_context(|| format!("decoding {}", path.display()))?;
        let spec = *audio_buf.spec();
        let frames = audio_buf.frames();
        if frames == 0 {
            continue;
        }
        let mut buffer = SampleBuffer::<f32>::new(frames as u64, spec);
        buffer.copy_interleaved_ref(audio_buf);
        mono.extend(mix_to_mono(buffer.samples(), spec.channels.count()));
    }
    if mono.is_empty() {
        anyhow::bail!("no audio frames decoded from {}", path.display());
    }
    Ok((mono, sample_rate))
}

/// Averages interleaved frames down to mono.
fn mix_to_mono(interleaved: &[f32], channels: usize) -> Vec<f32> {
    let channels = channels.max(1);
    #[expect(
        clippy::cast_precision_loss,
        reason = "channel counts are tiny; exact f32 representation"
    )]
    let inv_channels = 1.0 / channels as f32;
    interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() * inv_channels)
        .collect()
}

/// Offline-resamples mono samples from `sample_rate` to the engine rate.
fn to_engine_rate(mono: Vec<f32>, sample_rate: u32) -> anyhow::Result<Vec<f32>> {
    if sample_rate == ENGINE_SAMPLE_RATE {
        return Ok(mono);
    }
    let input_len = mono.len();
    let mut resampler = Fft::<f32>::new(
        sample_rate as usize,
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
pub(crate) fn write_mono_48k(path: &Path, samples: &[f32]) -> anyhow::Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: ENGINE_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .with_context(|| format!("cannot create WAV {}", path.display()))?;
    for &sample in samples {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "value is clamped to i16 range before the cast"
        )]
        let value = (f64::from(sample.clamp(-1.0, 1.0)) * f64::from(i16::MAX)).round() as i16;
        writer.write_sample(value)?;
    }
    writer.finalize()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    /// All fixtures encode the same 0.25 s, 440 Hz, -6 dBFS sine at 48 kHz.
    const FIXTURE_LEN: usize = 12_000;

    fn assert_tone(samples: &[f32], len_tolerance: usize, format: &str) {
        assert!(
            samples.len().abs_diff(FIXTURE_LEN) <= len_tolerance,
            "{format}: unexpected length {}",
            samples.len()
        );
        let peak = samples.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!(
            (peak - 0.5).abs() < 0.05,
            "{format}: unexpected peak {peak}"
        );
        assert!(samples.iter().all(|s| s.is_finite()), "{format}: NaN/inf");
    }

    fn correlation_at_best_lag(a: &[f32], b: &[f32], max_lag: usize) -> f32 {
        let len = a.len().min(b.len()).saturating_sub(max_lag);
        assert!(len > 1000, "not enough overlap");
        let mut best = f32::MIN;
        for lag in 0..=max_lag {
            let mut dot = 0.0_f64;
            let mut norm_a = 0.0_f64;
            let mut norm_b = 0.0_f64;
            for i in 0..len {
                let (x, y) = (f64::from(a[i]), f64::from(b[i + lag]));
                dot = x.mul_add(y, dot);
                norm_a = x.mul_add(x, norm_a);
                norm_b = y.mul_add(y, norm_b);
            }
            #[expect(
                clippy::cast_possible_truncation,
                reason = "normalized correlation is within f32 range"
            )]
            let r = (dot / (norm_a.sqrt() * norm_b.sqrt()).max(f64::MIN_POSITIVE)) as f32;
            best = best.max(r);
        }
        best
    }

    #[test]
    fn wav_fixture_reads_exactly() {
        let samples = read_mono_48k(&fixture("tone.wav")).expect("wav reads");
        assert_eq!(samples.len(), FIXTURE_LEN);
        assert_tone(&samples, 0, "wav");
    }

    #[test]
    fn aiff_matches_wav_reference() {
        let reference = read_mono_48k(&fixture("tone.wav")).expect("wav reads");
        let samples = read_mono_48k(&fixture("tone.aiff")).expect("aiff reads");
        assert_tone(&samples, 0, "aiff");
        assert!(correlation_at_best_lag(&reference, &samples, 0) > 0.999);
    }

    #[test]
    fn aifc_matches_wav_reference() {
        let reference = read_mono_48k(&fixture("tone.wav")).expect("wav reads");
        let samples = read_mono_48k(&fixture("tone.aifc")).expect("aifc reads");
        assert_tone(&samples, 0, "aifc");
        assert!(correlation_at_best_lag(&reference, &samples, 0) > 0.999);
    }

    #[test]
    fn caf_matches_wav_reference() {
        let reference = read_mono_48k(&fixture("tone.wav")).expect("wav reads");
        let samples = read_mono_48k(&fixture("tone.caf")).expect("caf reads");
        assert_tone(&samples, 0, "caf");
        assert!(correlation_at_best_lag(&reference, &samples, 0) > 0.999);
    }

    #[test]
    fn m4a_aac_decodes_the_tone() {
        let reference = read_mono_48k(&fixture("tone.wav")).expect("wav reads");
        let samples = read_mono_48k(&fixture("tone.m4a")).expect("m4a reads");
        // AAC is lossy (edge ringing changes the peak) and carries encoder
        // delay; allow padding differences and search a small lag window.
        assert!(samples.len().abs_diff(FIXTURE_LEN) <= 4096);
        assert!(samples.iter().all(|s| s.is_finite()));
        let peak = samples.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!((0.4..0.75).contains(&peak), "m4a: unexpected peak {peak}");
        assert!(correlation_at_best_lag(&reference, &samples, 3200) > 0.95);
    }

    #[test]
    fn m4a_alac_matches_wav_reference() {
        let reference = read_mono_48k(&fixture("tone-alac.m4a")).expect("alac reads");
        assert_tone(&reference, 0, "alac");
        let wav = read_mono_48k(&fixture("tone.wav")).expect("wav reads");
        assert!(correlation_at_best_lag(&wav, &reference, 0) > 0.999);
    }

    #[test]
    fn stereo_input_is_averaged_not_left_channel_only() {
        // Regression guard against the candidate-B stereo bug (left channel
        // only): the stereo fixture has the tone on the right channel and
        // silence on the left, so channel averaging must yield half
        // amplitude while "left only" would yield silence.
        let samples = read_mono_48k(&fixture("tone-stereo-right.wav")).expect("stereo reads");
        let peak = samples.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!(
            (peak - 0.25).abs() < 0.03,
            "stereo mixdown broken: peak {peak}"
        );
    }

    #[test]
    fn unsupported_input_suggests_afconvert() {
        let error = read_mono_48k(&fixture("not-audio.bin")).expect_err("must fail");
        let message = format!("{error:#}");
        assert!(message.contains("afconvert"), "unhelpful error: {message}");
    }
}
