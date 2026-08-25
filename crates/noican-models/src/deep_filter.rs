//! `DeepFilterNet3` baseline and Hush stages using the upstream Rust runtime.

use std::{
    path::{Path, PathBuf},
    sync::mpsc::{sync_channel, Receiver, SyncSender},
    thread::{self, JoinHandle},
};

use df::tract::{DfParams, DfTract, ReduceMask, RuntimeParams};
use ndarray015::{ArrayView2, ArrayViewMut2};
use noican_engine::{
    validate_frame_lengths, AudioStage, EnrollmentRequirement, StageDescriptor, StageError,
    StageKind,
};

use crate::assets::ModelAsset;

/// DeepFilterNet-family model exposed by this backend.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DeepFilterVariant {
    /// Official upstream `DeepFilterNet3` model embedded by `libDF`.
    DeepFilterNet3,
    /// Weya Hush 16 kHz background-speaker suppression model.
    Hush,
}

impl DeepFilterVariant {
    /// External model file, if this variant does not use embedded weights.
    #[must_use]
    pub const fn asset(self) -> Option<ModelAsset> {
        match self {
            Self::DeepFilterNet3 => None,
            Self::Hush => Some(ModelAsset::Hush),
        }
    }

    const fn id(self) -> &'static str {
        match self {
            Self::DeepFilterNet3 => "deepfilternet3",
            Self::Hush => "hush",
        }
    }

    const fn display_name(self) -> &'static str {
        match self {
            Self::DeepFilterNet3 => "DeepFilterNet3",
            Self::Hush => "Hush 16 kHz",
        }
    }

    const fn kind(self) -> StageKind {
        match self {
            Self::DeepFilterNet3 => StageKind::NoiseSuppression,
            Self::Hush => StageKind::SpeakerSuppression,
        }
    }
}

#[derive(Clone, Debug)]
enum ModelSource {
    Embedded,
    Bundle(PathBuf),
}

/// Stateful DeepFilterNet-family stage.
pub struct DeepFilterStage {
    variant: DeepFilterVariant,
    descriptor: StageDescriptor,
    commands: SyncSender<WorkerCommand>,
    responses: Receiver<WorkerResponse>,
    worker: Option<JoinHandle<()>>,
}

impl DeepFilterStage {
    /// Load the official embedded `DeepFilterNet3` baseline.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Backend`] if the embedded model cannot compile.
    pub fn deep_filter_net3() -> Result<Self, StageError> {
        Self::build(DeepFilterVariant::DeepFilterNet3, ModelSource::Embedded)
    }

    /// Load Hush from its official ONNX tar bundle.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Backend`] if the bundle is missing, malformed, or
    /// rejected by the `DeepFilterNet` runtime.
    pub fn hush(path: impl AsRef<Path>) -> Result<Self, StageError> {
        Self::build(
            DeepFilterVariant::Hush,
            ModelSource::Bundle(path.as_ref().to_path_buf()),
        )
    }

    fn build(variant: DeepFilterVariant, source: ModelSource) -> Result<Self, StageError> {
        let (command_sender, command_receiver) = sync_channel(1);
        let (response_sender, response_receiver) = sync_channel(1);
        let worker = thread::Builder::new()
            .name(format!("noican-{}", variant.id()))
            .spawn(move || run_worker(variant, &source, &command_receiver, &response_sender))
            .map_err(|error| backend_error(variant, error))?;
        let descriptor = match response_receiver.recv() {
            Ok(WorkerResponse::Ready(result)) => {
                result.map_err(|message| backend_error(variant, message))?
            }
            Ok(_unexpected) => {
                return Err(backend_error(
                    variant,
                    "worker returned a processing response before readiness",
                ));
            }
            Err(error) => return Err(backend_error(variant, error)),
        };
        Ok(Self {
            variant,
            descriptor,
            commands: command_sender,
            responses: response_receiver,
            worker: Some(worker),
        })
    }
}

impl AudioStage for DeepFilterStage {
    fn descriptor(&self) -> StageDescriptor {
        self.descriptor
    }

    fn process_frame(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), StageError> {
        validate_frame_lengths(self.descriptor, input, output)?;
        self.commands
            .send(WorkerCommand::Process(input.to_vec()))
            .map_err(|error| backend_error(self.variant, error))?;
        let processed = match self.responses.recv() {
            Ok(WorkerResponse::Processed(result)) => {
                result.map_err(|message| backend_error(self.variant, message))?
            }
            Ok(_unexpected) => {
                return Err(backend_error(
                    self.variant,
                    "worker returned a non-processing response",
                ));
            }
            Err(error) => return Err(backend_error(self.variant, error)),
        };
        if processed.len() != output.len() {
            return Err(backend_error(
                self.variant,
                format!(
                    "worker returned {} samples, expected {}",
                    processed.len(),
                    output.len()
                ),
            ));
        }
        output.copy_from_slice(&processed);
        Ok(())
    }

    fn reset(&mut self) -> Result<(), StageError> {
        self.commands
            .send(WorkerCommand::Reset)
            .map_err(|error| backend_error(self.variant, error))?;
        match self.responses.recv() {
            Ok(WorkerResponse::Reset(result)) => {
                result.map_err(|message| backend_error(self.variant, message))
            }
            Ok(_unexpected) => Err(backend_error(
                self.variant,
                "worker returned a non-reset response",
            )),
            Err(error) => Err(backend_error(self.variant, error)),
        }
    }
}

impl Drop for DeepFilterStage {
    fn drop(&mut self) {
        let _ignored = self.commands.send(WorkerCommand::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ignored = worker.join();
        }
    }
}

enum WorkerCommand {
    Process(Vec<f32>),
    Reset,
    Shutdown,
}

enum WorkerResponse {
    Ready(Result<StageDescriptor, String>),
    Processed(Result<Vec<f32>, String>),
    Reset(Result<(), String>),
}

fn run_worker(
    variant: DeepFilterVariant,
    source: &ModelSource,
    commands: &Receiver<WorkerCommand>,
    responses: &SyncSender<WorkerResponse>,
) {
    let built = build_worker_model(variant, source);
    let (mut model, descriptor) = match built {
        Ok(value) => value,
        Err(message) => {
            let _ignored = responses.send(WorkerResponse::Ready(Err(message)));
            return;
        }
    };
    if responses
        .send(WorkerResponse::Ready(Ok(descriptor)))
        .is_err()
    {
        return;
    }
    while let Ok(command) = commands.recv() {
        match command {
            WorkerCommand::Process(input) => {
                let result = process_worker_frame(&mut model, descriptor, &input);
                if responses.send(WorkerResponse::Processed(result)).is_err() {
                    return;
                }
            }
            WorkerCommand::Reset => {
                let result = build_worker_model(variant, source)
                    .map(|(replacement, _descriptor)| model = replacement);
                if responses.send(WorkerResponse::Reset(result)).is_err() {
                    return;
                }
            }
            WorkerCommand::Shutdown => return,
        }
    }
}

fn build_worker_model(
    variant: DeepFilterVariant,
    source: &ModelSource,
) -> Result<(DfTract, StageDescriptor), String> {
    let params = load_params(variant, source).map_err(|error| error.to_string())?;
    let model =
        DfTract::new(params, &runtime_params(variant)).map_err(|error| error.to_string())?;
    let sample_rate = u32::try_from(model.sr).map_err(|error| error.to_string())?;
    let algorithmic_delay_samples = model
        .lookahead
        .checked_add(1)
        .and_then(|frames| frames.checked_mul(model.hop_size))
        .ok_or_else(|| "DeepFilterNet latency overflow".to_owned())?;
    let descriptor = StageDescriptor {
        id: variant.id(),
        display_name: variant.display_name(),
        kind: variant.kind(),
        sample_rate,
        frame_samples: model.hop_size,
        algorithmic_delay_samples,
        tail_frames: model.lookahead + 1,
        enrollment: EnrollmentRequirement::None,
    };
    Ok((model, descriptor))
}

fn process_worker_frame(
    model: &mut DfTract,
    descriptor: StageDescriptor,
    input: &[f32],
) -> Result<Vec<f32>, String> {
    let input =
        ArrayView2::from_shape((1, input.len()), input).map_err(|error| error.to_string())?;
    let mut output = vec![0.0_f32; descriptor.frame_samples];
    let output_view = ArrayViewMut2::from_shape((1, output.len()), &mut output)
        .map_err(|error| error.to_string())?;
    model
        .process(input, output_view)
        .map_err(|error| error.to_string())?;
    Ok(output)
}

fn load_params(variant: DeepFilterVariant, source: &ModelSource) -> Result<DfParams, StageError> {
    match source {
        ModelSource::Embedded => Ok(DfParams::default()),
        ModelSource::Bundle(path) => {
            DfParams::new(path.clone()).map_err(|error| backend_error(variant, error))
        }
    }
}

fn runtime_params(variant: DeepFilterVariant) -> RuntimeParams {
    match variant {
        DeepFilterVariant::DeepFilterNet3 => RuntimeParams::default_with_ch(1),
        DeepFilterVariant::Hush => {
            RuntimeParams::new(1, 0.0, 100.0, -15.0, 35.0, 35.0, ReduceMask::MAX)
        }
    }
}

fn backend_error(variant: DeepFilterVariant, error: impl std::fmt::Display) -> StageError {
    StageError::Backend {
        stage: variant.id(),
        message: error.to_string(),
    }
}
