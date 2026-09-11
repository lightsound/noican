//! The `noican eval` command: builds the SIR mixtures, runs every
//! candidate stage on them, prints the metric table and writes the
//! listening set (see [`crate::eval`] for the metric definitions).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use noican_core::{ENGINE_SAMPLE_RATE, Stage};

use crate::audio;
use crate::eval::{self, Metrics};
use crate::process;

/// Command-line inputs of one evaluation run.
#[derive(Debug)]
pub(crate) struct EvalRequest {
    /// Clean recording(s) of the user's own voice, concatenated in order.
    pub(crate) targets: Vec<PathBuf>,
    /// Recording(s) of the interfering speaker, concatenated in order.
    pub(crate) interferers: Vec<PathBuf>,
    /// Signal-to-interference ratios (dB) of the overlap segment.
    pub(crate) sirs_db: Vec<f64>,
    /// RMS level (dBFS) to normalize the voice to; `None` keeps the
    /// recorded level.
    pub(crate) target_level_dbfs: Option<f64>,
    /// Requested segment length in seconds (clamped to the material).
    pub(crate) segment_seconds: f64,
    /// Model ids to evaluate.
    pub(crate) model_ids: Vec<String>,
    /// Output directory.
    pub(crate) out_dir: PathBuf,
    /// Seed for lettering the blind listening set.
    pub(crate) seed: u64,
}

/// One row of the result table.
#[derive(Debug, Clone)]
struct Row {
    model_id: String,
    sir_db: f64,
    metrics: Metrics,
    latency_ms: f64,
    block_p50_ms: f64,
    block_p99_ms: f64,
}

/// Below this high-band share a recording cannot exercise the high-band
/// metrics (a 16 kHz capture such as Bluetooth HFP sits near −100 dB).
const HIGH_BAND_SHARE_WARN_DB: f64 = -60.0;

/// Shortest segment the metrics are meaningful for (one second: the
/// region guard plus enough speech for a stable RMS).
const MIN_SEGMENT: usize = ENGINE_SAMPLE_RATE as usize;

fn seconds(samples: usize) -> f64 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "sample counts are far below 2^53"
    )]
    let s = samples as f64 / f64::from(ENGINE_SAMPLE_RATE);
    s
}

fn ns_to_ms(ns: u128) -> f64 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "display only; nanosecond exactness is irrelevant"
    )]
    let ms = ns as f64 / 1_000_000.0;
    ms
}

/// Directory name for one SIR, e.g. `sir+06`, `sir-06`.
fn sir_dir_name(sir_db: f64) -> String {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "SIR values are small integers or halves; rounding is intended"
    )]
    let rounded = sir_db.round() as i64;
    format!("sir{rounded:+03}")
}

/// Rejects SIR lists whose directory names collide (the name rounds to
/// whole dB, so `6` and `6.4` would silently overwrite each other's
/// mixtures and blind set) and non-finite values.
fn validate_sirs(sirs_db: &[f64]) -> anyhow::Result<()> {
    if sirs_db.is_empty() {
        anyhow::bail!("--sir needs at least one value");
    }
    let mut seen: Vec<(String, f64)> = Vec::with_capacity(sirs_db.len());
    for &sir_db in sirs_db {
        if !sir_db.is_finite() {
            anyhow::bail!("--sir {sir_db} is not a finite number of dB");
        }
        let name = sir_dir_name(sir_db);
        if let Some((_, other)) = seen.iter().find(|(seen_name, _)| *seen_name == name) {
            anyhow::bail!(
                "--sir {other} and {sir_db} would both be written to {name}/ \
                 (directory names are whole dB); pass values at least 1 dB apart"
            );
        }
        seen.push((name, sir_db));
    }
    Ok(())
}

/// A warning line when `samples` exceed full scale: every written WAV is
/// clamped to ±1.0 on the way to 16-bit, so the listening set would be
/// clipped while the metrics (computed on the unclamped floats) stay
/// clean. Nothing is rescaled here — Hush's behaviour depends on the
/// absolute level, so the level the user chose must reach the model.
fn clipping_warning(label: &str, samples: &[f32]) -> Option<String> {
    let peak = samples.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
    (peak > 1.0).then(|| {
        format!(
            "{label}: WARNING — peak {:+.1} dBFS exceeds full scale; the written WAVs are \
             clipped (the metrics are not). Lower --target-level-dbfs or the SIR range \
             before listening",
            eval::amplitude_db(f64::from(peak))
        )
    })
}

/// Loads, trims and concatenates the recordings at `paths`.
fn load_material(paths: &[PathBuf]) -> anyhow::Result<Vec<f32>> {
    let mut samples = Vec::new();
    for path in paths {
        let audio = audio::read_mono_48k(path)?;
        samples.extend_from_slice(eval::trim_silence(&audio));
    }
    Ok(samples)
}

fn describe_material(label: &str, samples: &[f32], progress: &mut impl FnMut(&str)) {
    let share = eval::high_band_share_db(samples);
    progress(&format!(
        "{label}: {:.1} s after trimming, RMS {:.1} dBFS, energy ≥ 8 kHz {share:.1} dB of total",
        seconds(samples.len()),
        eval::amplitude_db(eval::rms(samples)),
    ));
    if share < HIGH_BAND_SHARE_WARN_DB {
        progress(&format!(
            "{label}: WARNING — almost no content above 8 kHz (a 16 kHz capture?); \
             the high-band columns will not be meaningful"
        ));
    }
}

/// The trimmed (and optionally level-normalized) voice and interferer
/// tracks plus the segment length they support.
struct Material {
    target: Vec<f32>,
    interferer: Vec<f32>,
    segment_len: usize,
}

/// Loads both recordings, applies the target normalization, and derives
/// the segment length; reports the material facts through `progress`.
fn prepare_material(
    request: &EvalRequest,
    progress: &mut impl FnMut(&str),
) -> anyhow::Result<Material> {
    let mut target = load_material(&request.targets)?;
    let interferer = load_material(&request.interferers)?;
    if let Some(level_dbfs) = request.target_level_dbfs {
        let gain = eval::level_gain(&target, level_dbfs);
        for sample in &mut target {
            *sample *= gain;
        }
        progress(&format!(
            "target: normalized to {level_dbfs:.1} dBFS RMS (gain {:+.2} dB)",
            eval::amplitude_db(f64::from(gain))
        ));
    }
    describe_material("target", &target, progress);
    describe_material("interferer", &interferer, progress);

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "segment seconds are a small positive user value; clamped below"
    )]
    let requested = (request.segment_seconds.max(0.0) * f64::from(ENGINE_SAMPLE_RATE)) as usize;
    let segment_len = requested.min(target.len() / 2).min(interferer.len() / 2);
    if segment_len < MIN_SEGMENT {
        anyhow::bail!(
            "material too short: need at least {} s of voice and of interferer for 1 s segments \
             (have {:.1} s and {:.1} s after trimming)",
            2 * MIN_SEGMENT / ENGINE_SAMPLE_RATE as usize,
            seconds(target.len()),
            seconds(interferer.len())
        );
    }
    let l = seconds(segment_len);
    progress(&format!(
        "segments: 0–{l:.1} s you only | {l:.1}–{:.1} s you + other | {:.1}–{:.1} s other only \
         (metrics skip the first {:.2} s of each)",
        2.0 * l,
        2.0 * l,
        3.0 * l,
        seconds(eval::REGION_GUARD),
    ));
    Ok(Material {
        target,
        interferer,
        segment_len,
    })
}

/// Runs the evaluation. `make_stage` maps a model id to a fresh stage;
/// `progress` receives human-readable lines (the table is emitted through
/// it too, so the binary decides where output goes).
///
/// # Errors
///
/// Fails on unreadable material, material too short for one second per
/// segment, stage construction/processing errors, or I/O errors.
pub(crate) fn run(
    request: &EvalRequest,
    mut make_stage: impl FnMut(&str) -> anyhow::Result<Box<dyn Stage>>,
    mut progress: impl FnMut(&str),
) -> anyhow::Result<()> {
    validate_sirs(&request.sirs_db)?;
    let Material {
        target,
        interferer,
        segment_len,
    } = prepare_material(request, &mut progress)?;

    std::fs::create_dir_all(&request.out_dir)
        .with_context(|| format!("cannot create {}", request.out_dir.display()))?;
    audio::write_mono_48k(
        &request.out_dir.join("target.wav"),
        &target[..2 * segment_len],
    )?;

    let mut rows = Vec::new();
    let mut key_lines = Vec::new();
    for (sir_index, &sir_db) in request.sirs_db.iter().enumerate() {
        let mixture = eval::build_mixture(&target, &interferer, sir_db, segment_len)
            .map_err(|message| anyhow::anyhow!(message))?;
        let sir_dir = request.out_dir.join(sir_dir_name(sir_db));
        std::fs::create_dir_all(&sir_dir)
            .with_context(|| format!("cannot create {}", sir_dir.display()))?;
        audio::write_mono_48k(&sir_dir.join("input.wav"), &mixture.input)?;
        progress(&format!(
            "SIR {sir_db:+.0} dB: interferer gain {:.2} dB -> {}",
            eval::amplitude_db(f64::from(mixture.interferer_gain)),
            sir_dir.display()
        ));
        if let Some(warning) =
            clipping_warning(&format!("SIR {sir_db:+.0} dB: mixture"), &mixture.input)
        {
            progress(&warning);
        }

        let mut outputs = Vec::with_capacity(request.model_ids.len());
        for model_id in &request.model_ids {
            let mut stage = make_stage(model_id)?;
            let (output, block_times_ns) =
                process::run_stage_aligned_timed(stage.as_mut(), &mixture.input)?;
            let metrics = eval::evaluate(&output, &mixture);
            let mut sorted = block_times_ns;
            sorted.sort_unstable();
            rows.push(Row {
                model_id: model_id.clone(),
                sir_db,
                metrics,
                latency_ms: seconds(stage.latency_samples()) * 1000.0,
                block_p50_ms: ns_to_ms(eval::quantile(&sorted, 0.50)),
                block_p99_ms: ns_to_ms(eval::quantile(&sorted, 0.99)),
            });
            audio::write_mono_48k(&sir_dir.join(format!("{model_id}.wav")), &output)?;
            progress(&format!("SIR {sir_db:+.0} dB: {model_id} done"));
            if let Some(warning) =
                clipping_warning(&format!("SIR {sir_db:+.0} dB: {model_id}"), &output)
            {
                progress(&warning);
            }
            outputs.push(output);
        }
        write_blind_set(
            &sir_dir,
            &request.model_ids,
            &outputs,
            request.seed.wrapping_add(sir_index as u64),
            &mut key_lines,
        )?;
    }

    let key_path = request.out_dir.join("blind-key.txt");
    std::fs::write(&key_path, key_lines.join("\n") + "\n")
        .with_context(|| format!("cannot write {}", key_path.display()))?;
    let csv_path = request.out_dir.join("metrics.csv");
    std::fs::write(&csv_path, render_csv(&rows))
        .with_context(|| format!("cannot write {}", csv_path.display()))?;

    for line in render_table(&rows) {
        progress(&line);
    }
    progress(&format!(
        "wrote {} and {} (open the key only after listening)",
        csv_path.display(),
        key_path.display()
    ));
    Ok(())
}

/// Copies each model output under a letter chosen by [`eval::blind_order`]
/// and appends the answer lines to `key_lines`.
fn write_blind_set(
    sir_dir: &Path,
    model_ids: &[String],
    outputs: &[Vec<f32>],
    seed: u64,
    key_lines: &mut Vec<String>,
) -> anyhow::Result<()> {
    let blind_dir = sir_dir.join("blind");
    std::fs::create_dir_all(&blind_dir)
        .with_context(|| format!("cannot create {}", blind_dir.display()))?;
    let order = eval::blind_order(model_ids.len(), seed);
    for (letter_index, &model_index) in order.iter().enumerate() {
        let letter = char::from(b'A' + u8::try_from(letter_index % 26).unwrap_or(0));
        let suffix = if letter_index >= 26 {
            format!("{}", letter_index / 26)
        } else {
            String::new()
        };
        let name = format!("{letter}{suffix}");
        audio::write_mono_48k(
            &blind_dir.join(format!("{name}.wav")),
            &outputs[model_index],
        )?;
        key_lines.push(format!(
            "{}/blind/{name}.wav = {}",
            sir_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default(),
            model_ids[model_index]
        ));
    }
    Ok(())
}

/// The metric table, one line per (model, SIR).
fn render_table(rows: &[Row]) -> Vec<String> {
    let mut lines = vec![
        String::new(),
        "columns: HF keep = high band (≥ 8 kHz) of your voice kept, you only (0 dB = all); \
         level = your voice's level vs. the clean recording, you only (0 dB = same loudness); \
         SI-SDR you / both = own-voice fidelity alone / while the other talks (higher = better); \
         resid all / HF = interferer left when only the other talks, full band / ≥ 8 kHz \
         (more negative = better); p50 / p99 = 10 ms block processing time"
            .to_owned(),
        format!(
            "{:<18} {:>5} {:>8} {:>8} {:>10} {:>11} {:>9} {:>9} {:>8} {:>11}",
            "model",
            "SIR",
            "HF keep",
            "level",
            "SI-SDR you",
            "SI-SDR both",
            "resid all",
            "resid HF",
            "latency",
            "p50/p99 ms"
        ),
    ];
    for row in rows {
        let m = &row.metrics;
        lines.push(format!(
            "{:<18} {:>+5.0} {:>6.1} dB {:>+5.1} dB {:>7.1} dB {:>8.1} dB {:>6.1} dB {:>6.1} dB {:>5.1} ms {:>5.2}/{:<5.2}",
            row.model_id,
            row.sir_db,
            m.high_band_retention,
            m.own_voice_level,
            m.own_voice_si_sdr,
            m.both_si_sdr,
            m.interferer_residual,
            m.interferer_residual_high,
            row.latency_ms,
            row.block_p50_ms,
            row.block_p99_ms,
        ));
    }
    lines
}

/// The same rows as CSV (header + one line per row).
fn render_csv(rows: &[Row]) -> String {
    let mut csv = String::from(
        "model,sir_db,high_band_retention_db,own_voice_level_db,own_voice_si_sdr_db,both_si_sdr_db,\
         interferer_residual_db,interferer_residual_high_db,latency_ms,block_p50_ms,block_p99_ms\n",
    );
    for row in rows {
        let m = &row.metrics;
        // Writing to a String cannot fail.
        let _ = writeln!(
            csv,
            "{},{},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.3},{:.3}",
            row.model_id,
            row.sir_db,
            m.high_band_retention,
            m.own_voice_level,
            m.own_voice_si_sdr,
            m.both_si_sdr,
            m.interferer_residual,
            m.interferer_residual_high,
            row.latency_ms,
            row.block_p50_ms,
            row.block_p99_ms,
        );
    }
    csv
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sir_dir_names_are_signed_and_zero_padded() {
        assert_eq!(sir_dir_name(12.0), "sir+12");
        assert_eq!(sir_dir_name(6.0), "sir+06");
        assert_eq!(sir_dir_name(0.0), "sir+00");
        assert_eq!(sir_dir_name(-6.0), "sir-06");
    }

    #[test]
    fn sirs_that_share_a_directory_name_are_rejected() {
        validate_sirs(&[12.0, 6.0, 0.0, -6.0]).expect("distinct whole-dB values");
        let err = validate_sirs(&[6.0, 6.4]).expect_err("6 and 6.4 both round to sir+06");
        assert!(err.to_string().contains("sir+06"), "{err}");
        assert!(validate_sirs(&[]).is_err());
        assert!(validate_sirs(&[f64::NAN]).is_err());
        assert!(validate_sirs(&[f64::INFINITY]).is_err());
    }

    #[test]
    fn clipping_is_reported_only_above_full_scale() {
        assert!(clipping_warning("x", &[0.0, 1.0, -1.0]).is_none());
        let warning = clipping_warning("SIR +0 dB: mixture", &[0.2, -1.5]).expect("clips");
        assert!(
            warning.starts_with("SIR +0 dB: mixture: WARNING"),
            "{warning}"
        );
        assert!(warning.contains("+3.5 dBFS"), "{warning}");
    }

    /// A fresh directory under the system temp dir, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "noican-eval-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos())
            ));
            std::fs::create_dir_all(&dir).expect("temp dir");
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            // Best effort: a leftover temp dir is not a test failure.
            drop(std::fs::remove_dir_all(&self.0));
        }
    }

    fn synthetic(seconds: usize, freq: f32, amplitude: f32, seed: u32) -> Vec<f32> {
        let mut state = seed;
        (0..seconds * ENGINE_SAMPLE_RATE as usize)
            .map(|n| {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "sample indices fit f32 for a few seconds"
                )]
                let t = n as f32 / 48_000.0;
                // A tone plus a little deterministic noise so trimming
                // and the band metrics have something to work with.
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "uniform noise from the top bits; precision is irrelevant"
                )]
                let noise = (state >> 8) as f32 / 16_777_216.0 - 0.5;
                0.02f32.mul_add(
                    noise,
                    amplitude * (2.0 * std::f32::consts::PI * freq * t).sin(),
                )
            })
            .collect()
    }

    /// The whole command path with synthetic material and a passthrough
    /// stage: files land where the docs say, one row per (model, SIR),
    /// and every `blind-key.txt` line names a file whose samples equal
    /// the model output it claims to be.
    #[test]
    fn run_writes_mixtures_rows_and_a_consistent_blind_key() {
        let tmp = TempDir::new("run");
        let target_path = tmp.0.join("voice.wav");
        let interferer_path = tmp.0.join("other.wav");
        audio::write_mono_48k(&target_path, &synthetic(5, 220.0, 0.3, 1)).expect("write");
        audio::write_mono_48k(&interferer_path, &synthetic(5, 3_000.0, 0.3, 2)).expect("write");
        let out_dir = tmp.0.join("out");
        let request = EvalRequest {
            targets: vec![target_path],
            interferers: vec![interferer_path],
            sirs_db: vec![6.0, -6.0],
            target_level_dbfs: None,
            segment_seconds: 1.5,
            model_ids: vec!["passthrough".to_owned(), "twice".to_owned()],
            out_dir: out_dir.clone(),
            seed: 7,
        };
        let mut lines = Vec::new();
        run(
            &request,
            |id| -> anyhow::Result<Box<dyn Stage>> {
                Ok(match id {
                    "passthrough" => Box::new(noican_core::Passthrough),
                    _ => Box::new(Twice),
                })
            },
            |line| lines.push(line.to_owned()),
        )
        .expect("run succeeds");

        // Segment length clamps to the material (5 s → 1.5 s requested).
        assert!(
            lines.iter().any(|l| l.starts_with("segments: 0–1.5 s")),
            "{lines:#?}"
        );
        assert!(out_dir.join("target.wav").is_file());
        for dir in ["sir+06", "sir-06"] {
            for file in [
                "input.wav",
                "passthrough.wav",
                "twice.wav",
                "blind/A.wav",
                "blind/B.wav",
            ] {
                assert!(
                    out_dir.join(dir).join(file).is_file(),
                    "{dir}/{file} missing"
                );
            }
        }
        let csv = std::fs::read_to_string(out_dir.join("metrics.csv")).expect("csv");
        assert_eq!(csv.lines().count(), 1 + 2 * 2, "{csv}");

        // The passthrough row is the anchor: HF keep 0, level 0, SI-SDR
        // 100, residual 0.
        let anchor = csv
            .lines()
            .find(|l| l.starts_with("passthrough,6,"))
            .expect("anchor row");
        assert!(
            anchor.starts_with("passthrough,6,0.00,0.00,100.00,"),
            "{anchor}"
        );

        let key = std::fs::read_to_string(out_dir.join("blind-key.txt")).expect("key");
        let key_lines: Vec<&str> = key.lines().collect();
        assert_eq!(key_lines.len(), 4, "{key}");
        let mut seen_models = Vec::new();
        for line in key_lines {
            let (blind, model) = line.split_once(" = ").expect("`path = model`");
            let blind_samples = audio::read_mono_48k(&out_dir.join(blind)).expect("blind wav");
            let dir = blind.split('/').next().expect("sir dir");
            let model_samples =
                audio::read_mono_48k(&out_dir.join(dir).join(format!("{model}.wav")))
                    .expect("model wav");
            assert_eq!(blind_samples, model_samples, "{line}: blind copy differs");
            seen_models.push(model.to_owned());
        }
        seen_models.sort_unstable();
        assert_eq!(
            seen_models,
            ["passthrough", "passthrough", "twice", "twice"]
        );

        // The "twice" stage doubles a 0.3-amplitude mixture past full
        // scale at SIR −6 dB, which the run must report.
        assert!(
            lines.iter().any(|l| l.contains("twice: WARNING")),
            "no clipping warning in {lines:#?}"
        );
    }

    /// A stage that doubles the signal (level +6 dB, no delay).
    struct Twice;

    impl Stage for Twice {
        fn id(&self) -> &'static str {
            "twice"
        }

        fn process_block(
            &mut self,
            input: &[f32],
            output: &mut [f32],
        ) -> Result<(), noican_core::StageError> {
            for (o, i) in output.iter_mut().zip(input) {
                *o = 2.0 * i;
            }
            Ok(())
        }

        fn latency_samples(&self) -> usize {
            0
        }

        fn reset(&mut self) {}
    }

    #[test]
    fn csv_has_one_line_per_row_plus_header() {
        let rows = vec![Row {
            model_id: "passthrough".to_owned(),
            sir_db: 6.0,
            metrics: Metrics {
                high_band_retention: 0.0,
                own_voice_level: 0.0,
                own_voice_si_sdr: 100.0,
                both_si_sdr: 6.0,
                interferer_residual: 0.0,
                interferer_residual_high: 0.0,
            },
            latency_ms: 0.0,
            block_p50_ms: 0.01,
            block_p99_ms: 0.02,
        }];
        let csv = render_csv(&rows);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("model,sir_db,"));
        assert!(lines[1].starts_with("passthrough,6,0.00,0.00,100.00,6.00,"));
        assert_eq!(render_table(&rows).len(), 4);
    }
}
