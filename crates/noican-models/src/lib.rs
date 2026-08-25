//! ONNX-backed processing stages and model-weight acquisition.
//!
//! This crate turns a catalog entry into a [`noican_core::Stage`]. Every model
//! in the catalog is switchable at run time because they all end up behind that
//! one trait, and adding a model means adding a catalog entry plus — only if its
//! ONNX signature is genuinely new — one more stage implementation.
//!
//! ```no_run
//! use noican_models::{ModelStore, build_stage, catalog};
//!
//! let store = ModelStore::from_environment();
//! let model = catalog::find("fastenhancer-t").expect("catalogued");
//! let stage = build_stage(model, &store)?;
//! println!("{} runs at {} Hz", model.display_name, stage.spec().sample_rate);
//! # Ok::<(), noican_models::Error>(())
//! ```

pub mod catalog;
pub mod dfn;
pub mod error;
pub mod latency;
pub mod session;
pub mod speaker;
pub mod stages;
pub mod store;

pub use catalog::{
    Architecture, Artifact, ArtifactKind, CATALOG, ModelDescriptor, ModelKind, SpectralParams,
};
pub use error::{Error, Result};
pub use store::{MODEL_DIR_ENV, ModelStore, Progress};

use noican_core::Stage;

/// Instantiates `model`, loading its weights from `store`.
///
/// # Errors
///
/// Returns [`Error::MissingWeights`] if the weights have not been downloaded,
/// [`Error::Runtime`] if ONNX Runtime rejects the graph, or
/// [`Error::UnexpectedSignature`] if the graph does not match the signature its
/// architecture implies.
pub fn build_stage(model: &ModelDescriptor, store: &ModelStore) -> Result<Box<dyn Stage>> {
    let latency = latency::of(model.id);
    // A bundle is required as a directory rather than a file, so the plain
    // presence check only applies to single-graph models.
    let path = if model.architecture == Architecture::DeepFilterNet {
        std::path::PathBuf::new()
    } else {
        store.require(model)?
    };

    let stage: Box<dyn Stage> = match model.architecture {
        Architecture::Waveform => Box::new(stages::WaveformStage::load(
            model.id,
            &path,
            model.sample_rate,
            latency,
        )?),
        Architecture::SpectralSelfDescribing => Box::new(
            stages::SpectralStage::load_self_describing(model.id, &path)?,
        ),
        Architecture::Spectral(params) => Box::new(stages::SpectralStage::load_with_params(
            model.id,
            &path,
            model.sample_rate,
            params,
            latency,
        )?),
        Architecture::SpeakerGate => {
            let profile_path = store.speaker_profile_path();
            if !profile_path.is_file() {
                return Err(Error::Enrolment {
                    detail: format!(
                        "no profile at `{}`; run `noican enroll <recording-of-your-voice.wav>` \
                         first",
                        profile_path.display()
                    ),
                });
            }
            let profile = speaker::SpeakerProfile::load(&profile_path)?;
            Box::new(stages::SpeakerGateStage::new(
                model.id, &path, profile, latency,
            )?)
        }
        Architecture::DeepFilterNet => {
            let directory = store.require_bundle(model)?;
            Box::new(stages::DeepFilterNetStage::load(
                model.id, &directory, latency,
            )?)
        }
    };
    Ok(stage)
}

/// Instantiates a model by identifier.
///
/// # Errors
///
/// Returns [`Error::UnknownModel`] if `id` is not in the catalog, plus anything
/// [`build_stage`] can return.
pub fn build_stage_by_id(id: &str, store: &ModelStore) -> Result<Box<dyn Stage>> {
    let model = catalog::find(id).ok_or_else(|| Error::UnknownModel(id.to_owned()))?;
    build_stage(model, store)
}
