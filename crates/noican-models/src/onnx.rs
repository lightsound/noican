//! Shared ONNX Runtime helpers for streaming model stages.

use std::path::Path;

use noican_core::StageError;
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::{DynValue, Tensor};

/// Loads an ONNX session tuned for low-latency streaming inference on CPU:
/// single-threaded (frames are small; thread wakeups cost more than they
/// save) with full graph optimization.
///
/// Single-threading is a measured decision, not a guess — it holds even
/// for the largest streaming model. A/B on 2026-09-02 (x86-64, 4 cores,
/// `examples/block_bench.rs`, 6000 blocks of FastEnhancer-L):
/// p50 4.16 ms / p95 6.23 ms with 1 intra-op thread, 6.30 / 9.53 ms with
/// 2 threads, and 5.77 / 8.72 ms with 4 — synchronization overhead
/// dominates these small per-frame ops, so extra threads make every
/// percentile worse. Inter-op stays at 1 as well: the graphs are
/// sequential, so inter-op parallelism only adds scheduling overhead.
///
/// # Errors
///
/// Returns [`StageError::Inference`] when the file cannot be loaded.
pub fn load_streaming_session(path: &Path) -> Result<Session, StageError> {
    let build = || -> ort::Result<Session> {
        Session::builder()?
            .with_optimization_level(GraphOptimizationLevel::Level3)?
            .with_intra_threads(1)?
            .with_inter_threads(1)?
            .commit_from_file(path)
    };
    build().map_err(|e| StageError::Inference(format!("failed to load {}: {e}", path.display())))
}

/// Maps an [`ort::Error`] into a [`StageError`] with context.
#[must_use]
pub fn inference_error(context: &str, e: &ort::Error) -> StageError {
    StageError::Inference(format!("{context}: {e}"))
}

/// Reads a custom metadata string from the session, erroring when absent.
///
/// # Errors
///
/// Returns [`StageError::Inference`] when the key is missing or metadata
/// cannot be read.
pub fn required_metadata(session: &Session, key: &str) -> Result<String, StageError> {
    session
        .metadata()
        .map_err(|e| inference_error("reading model metadata", &e))?
        .custom(key)
        .ok_or_else(|| StageError::Inference(format!("model metadata key missing: {key}")))
}

/// Returns the static tensor shape of the named session input.
///
/// # Errors
///
/// Returns [`StageError::Inference`] when the input does not exist or has
/// no static tensor shape.
pub fn input_shape(session: &Session, name: &str) -> Result<Vec<usize>, StageError> {
    let input = session
        .inputs()
        .iter()
        .find(|i| i.name() == name)
        .ok_or_else(|| StageError::Inference(format!("model input missing: {name}")))?;
    let dims = input
        .dtype()
        .tensor_shape()
        .ok_or_else(|| StageError::Inference(format!("input {name} is not a tensor")))?;
    dims.iter()
        .map(|d| {
            usize::try_from(*d)
                .map_err(|_| StageError::Inference(format!("input {name} has a dynamic dimension")))
        })
        .collect()
}

/// One recurrent state tensor carried across streaming calls.
#[derive(Debug)]
pub struct StateSlot {
    /// Graph input name.
    pub input_name: String,
    /// Graph output name feeding the next call.
    pub output_name: String,
    /// Static shape.
    pub shape: Vec<usize>,
    /// Current value (updated after every run).
    pub data: Vec<f32>,
    /// Value to restore on [`StateBank::reset`].
    init: Vec<f32>,
}

/// A bank of recurrent state tensors threaded output → input each call.
#[derive(Debug, Default)]
pub struct StateBank {
    slots: Vec<StateSlot>,
}

impl StateBank {
    /// Builds slots for every session input whose name starts with
    /// `input_prefix` followed by an index, pairing it with
    /// `output_prefix` + the same index (e.g. `cache_in_0` → `cache_out_0`).
    /// All states initialize to zeros.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Inference`] when a paired input has a dynamic
    /// shape.
    pub fn from_indexed_prefix(
        session: &Session,
        input_prefix: &str,
        output_prefix: &str,
    ) -> Result<Self, StageError> {
        let mut indexed: Vec<(usize, String)> = session
            .inputs()
            .iter()
            .filter_map(|i| {
                i.name()
                    .strip_prefix(input_prefix)
                    .and_then(|suffix| suffix.parse::<usize>().ok())
                    .map(|idx| (idx, i.name().to_owned()))
            })
            .collect();
        indexed.sort_unstable_by_key(|(idx, _)| *idx);
        let mut slots = Vec::with_capacity(indexed.len());
        for (idx, name) in indexed {
            let shape = input_shape(session, &name)?;
            let len = shape.iter().product();
            slots.push(StateSlot {
                input_name: name,
                output_name: format!("{output_prefix}{idx}"),
                shape,
                data: vec![0.0; len],
                init: vec![0.0; len],
            });
        }
        Ok(Self { slots })
    }

    /// Builds slots from explicit `(input_name, output_name)` pairs, all
    /// initialized to zeros.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Inference`] when an input is missing or has a
    /// dynamic shape.
    pub fn from_pairs(session: &Session, pairs: &[(&str, &str)]) -> Result<Self, StageError> {
        let mut slots = Vec::with_capacity(pairs.len());
        for (input_name, output_name) in pairs {
            let shape = input_shape(session, input_name)?;
            let len = shape.iter().product();
            slots.push(StateSlot {
                input_name: (*input_name).to_owned(),
                output_name: (*output_name).to_owned(),
                shape,
                data: vec![0.0; len],
                init: vec![0.0; len],
            });
        }
        Ok(Self { slots })
    }

    /// Overrides the initial (and current) value of the slot whose input is
    /// `input_name`.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Inference`] on unknown names or length
    /// mismatch.
    pub fn set_init(&mut self, input_name: &str, init: &[f32]) -> Result<(), StageError> {
        let slot = self
            .slots
            .iter_mut()
            .find(|s| s.input_name == input_name)
            .ok_or_else(|| StageError::Inference(format!("unknown state slot: {input_name}")))?;
        if init.len() != slot.init.len() {
            return Err(StageError::BufferLen {
                expected: slot.init.len(),
                got: init.len(),
            });
        }
        slot.init.copy_from_slice(init);
        slot.data.copy_from_slice(init);
        Ok(())
    }

    /// Number of slots.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.slots.len()
    }

    /// True when the bank has no slots.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Appends the current state values as named tensors to `inputs`.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Inference`] on tensor construction failure.
    pub fn append_inputs(
        &self,
        inputs: &mut Vec<(
            std::borrow::Cow<'static, str>,
            ort::session::SessionInputValue<'static>,
        )>,
    ) -> Result<(), StageError> {
        for slot in &self.slots {
            let tensor = Tensor::from_array((slot.shape.clone(), slot.data.clone()))
                .map_err(|e| inference_error("building state tensor", &e))?;
            inputs.push((
                std::borrow::Cow::Owned(slot.input_name.clone()),
                tensor.into(),
            ));
        }
        Ok(())
    }

    /// Copies each slot's next value out of the run `outputs`.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Inference`] when an output is missing or has
    /// the wrong element count.
    pub fn update_from_outputs(
        &mut self,
        outputs: &ort::session::SessionOutputs<'_>,
    ) -> Result<(), StageError> {
        for slot in &mut self.slots {
            let value: &DynValue = outputs.get(slot.output_name.as_str()).ok_or_else(|| {
                StageError::Inference(format!("model output missing: {}", slot.output_name))
            })?;
            let (_, data) = value
                .try_extract_tensor::<f32>()
                .map_err(|e| inference_error("extracting state output", &e))?;
            if data.len() != slot.data.len() {
                return Err(StageError::BufferLen {
                    expected: slot.data.len(),
                    got: data.len(),
                });
            }
            slot.data.copy_from_slice(data);
        }
        Ok(())
    }

    /// Restores every slot to its initial value.
    pub fn reset(&mut self) {
        for slot in &mut self.slots {
            slot.data.copy_from_slice(&slot.init);
        }
    }
}
