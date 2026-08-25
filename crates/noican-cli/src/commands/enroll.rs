//! `noican enroll` — record who the speaker gate should recognise.

use anyhow::{Context, Result, bail};
use std::path::PathBuf;

use noican_models::speaker::embedder::MINIMUM_WINDOW_SECONDS;
use noican_models::speaker::{SpeakerEmbedder, SpeakerProfile};
use noican_models::{ModelStore, catalog};

use crate::offline::resample;
use crate::wav;

/// Arguments for `noican enroll`.
#[derive(Debug, clap::Args)]
pub(crate) struct Args {
    /// Recordings of the speaker to enrol. Several are better than one.
    #[arg(required = true)]
    pub inputs: Vec<PathBuf>,

    /// Where to write the profile. Defaults to the model store.
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}

/// The catalogued model whose embeddings a profile is made of.
const GATE_MODEL: &str = "speaker-gate";

/// Enrols the speaker heard in `inputs`.
///
/// # Errors
///
/// Fails if the model has not been downloaded, if a recording cannot be read,
/// or if none of the recordings contain enough speech to embed.
pub(crate) fn run(args: &Args, store: &ModelStore) -> Result<()> {
    let model = catalog::find(GATE_MODEL)
        .with_context(|| format!("`{GATE_MODEL}` is not in the catalog"))?;
    let graph = store
        .require(model)
        .with_context(|| format!("run `noican fetch {GATE_MODEL}` first"))?;
    let mut embedder = SpeakerEmbedder::load(model.id, &graph)?;

    let window = SpeakerEmbedder::minimum_window_samples();
    let mut embeddings = Vec::new();
    let mut total_seconds = 0.0f32;

    for path in &args.inputs {
        let clip = wav::read(path)?;
        #[expect(
            clippy::cast_precision_loss,
            reason = "clip lengths and sample rates are exact enough in f32 for a duration"
        )]
        let seconds = clip.samples.len() as f32 / clip.sample_rate as f32;

        // The embedding model is 16 kHz only, and a recording at some other rate
        // is the normal case rather than an error.
        let samples = resample(&clip, model.sample_rate)?.samples;

        if samples.len() < window {
            println!(
                "  {}: {seconds:.1} s is shorter than the {MINIMUM_WINDOW_SECONDS:.1} s the model \
                 needs, skipped",
                path.display()
            );
            continue;
        }

        let from_clip = embedder.embed_windows(&samples)?;
        println!(
            "  {}: {seconds:.1} s, {} windows",
            path.display(),
            from_clip.len()
        );
        total_seconds += seconds;
        embeddings.extend(from_clip);
    }

    if embeddings.is_empty() {
        bail!(
            "no recording contained {MINIMUM_WINDOW_SECONDS:.1} s of audio. Record a few seconds \
             of continuous speech and try again."
        );
    }

    let profile = SpeakerProfile::from_embeddings(model.id, &embeddings)?;
    let destination = args
        .output
        .clone()
        .unwrap_or_else(|| store.speaker_profile_path());
    profile.save(&destination)?;

    println!();
    println!(
        "enrolled from {total_seconds:.1} s across {} windows -> {}",
        profile.windows,
        destination.display()
    );

    // Self-similarity is a sanity check the user can act on: a profile averaged
    // from windows that disagree with each other will not recognise anyone, and
    // the usual cause is enrolling from a recording with more than one speaker
    // in it.
    let agreement = embeddings
        .iter()
        .map(|embedding| profile.similarity(embedding))
        .sum::<f32>()
        / f32_len(embeddings.len());
    println!("windows agree with the profile at {agreement:.2} on average");
    if agreement < 0.4 {
        println!(
            "that is low. Check the recording is one person speaking continuously, with no other \
             voice in the background."
        );
    }
    println!("`noican process --model speaker-gate` will now use it.");
    Ok(())
}

/// A count as an `f32`, for averaging.
const fn f32_len(count: usize) -> f32 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "window counts are far below f32's exact integer range"
    )]
    let value = count as f32;
    value
}
