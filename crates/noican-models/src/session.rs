//! A session wrapper that carries recurrent state between calls.
//!
//! Every streaming model in the catalog has the same shape of contract: a fixed
//! number of "primary" inputs (a waveform block, a spectrum frame, or a
//! spectrum plus features) followed by state tensors, and outputs laid out the
//! same way. Threading that state is the part that is easy to get subtly wrong
//! and identical across models, so it lives here once.
//!
//! State buffers are sized from the shapes the graph declares rather than
//! hard-coded, which is what lets one stage implementation serve all five
//! `FastEnhancer` variants — their cache shapes differ.

use ort::session::Session;
use ort::value::TensorRef;

use crate::error::{Error, Result};

/// One state tensor, kept between calls.
#[derive(Debug)]
struct StateTensor {
    shape: Vec<i64>,
    data: Vec<f32>,
    /// Value the tensor is reset to. Usually zeros, but the `DPDFNet` family
    /// seeds its normalisation history from model metadata.
    initial: Vec<f32>,
}

/// An ONNX session plus the state tensors its graph expects.
pub struct CachedSession {
    model_id: String,
    session: Session,
    input_names: Vec<String>,
    output_names: Vec<String>,
    input_shapes: Vec<Vec<i64>>,
    primary_inputs: usize,
    states: Vec<StateTensor>,
}

// `ort::session::Session` is not `Debug`.
impl std::fmt::Debug for CachedSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedSession")
            .field("model_id", &self.model_id)
            .field("primary_inputs", &self.primary_inputs)
            .field("state_tensors", &self.states.len())
            .finish_non_exhaustive()
    }
}

impl CachedSession {
    /// Loads `path` and prepares state buffers for every input past the first
    /// `primary_inputs`.
    ///
    /// Inference runs single-threaded on purpose: the engine already dedicates
    /// one thread to it, and letting ONNX Runtime spawn its own pool would add
    /// scheduling jitter to a path with a 10 ms budget.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Runtime`] if the graph cannot be loaded, or
    /// [`Error::UnexpectedSignature`] if its inputs and outputs do not pair up
    /// as `primary_inputs` primaries followed by matching state tensors.
    pub fn load(
        model_id: &str,
        path: &std::path::Path,
        primary_inputs: usize,
        primary_outputs: usize,
    ) -> Result<Self> {
        // `SessionBuilder` methods hand the builder back inside their error, so
        // their error type differs from the session's and needs one explicit
        // conversion step.
        let session = Session::builder()?
            .with_intra_threads(1)
            .map_err(ort::Error::from)?
            .commit_from_file(path)?;

        let input_names: Vec<String> = session
            .inputs()
            .iter()
            .map(|outlet| outlet.name().to_owned())
            .collect();
        let output_names: Vec<String> = session
            .outputs()
            .iter()
            .map(|outlet| outlet.name().to_owned())
            .collect();

        if input_names.len() < primary_inputs || output_names.len() < primary_outputs {
            return Err(Error::UnexpectedSignature {
                model: model_id.to_owned(),
                detail: format!(
                    "expected at least {primary_inputs} inputs and {primary_outputs} outputs, \
                     found {} and {}",
                    input_names.len(),
                    output_names.len()
                ),
            });
        }

        let state_count = input_names.len() - primary_inputs;
        if output_names.len() - primary_outputs != state_count {
            return Err(Error::UnexpectedSignature {
                model: model_id.to_owned(),
                detail: format!(
                    "{state_count} state inputs but {} state outputs; a streaming graph must \
                     return every state it consumes",
                    output_names.len() - primary_outputs
                ),
            });
        }

        let mut input_shapes = Vec::with_capacity(input_names.len());
        for index in 0..input_names.len() {
            input_shapes.push(declared_shape(model_id, &session, index)?);
        }

        let mut states = Vec::with_capacity(state_count);
        for index in primary_inputs..input_names.len() {
            let shape = input_shapes[index].clone();
            if shape.iter().any(|&dimension| dimension <= 0) {
                return Err(Error::UnexpectedSignature {
                    model: model_id.to_owned(),
                    detail: format!(
                        "state input `{}` has a dynamic shape {shape:?}; state tensors must be \
                         statically sized so they can be preallocated",
                        input_names[index]
                    ),
                });
            }
            let elements = shape.iter().product::<i64>();
            let Ok(elements) = usize::try_from(elements) else {
                return Err(Error::UnexpectedSignature {
                    model: model_id.to_owned(),
                    detail: format!(
                        "state input `{}` has an unusable element count {elements}",
                        input_names[index]
                    ),
                });
            };
            states.push(StateTensor {
                shape,
                data: vec![0.0; elements],
                initial: vec![0.0; elements],
            });
        }

        Ok(Self {
            model_id: model_id.to_owned(),
            session,
            input_names,
            output_names,
            input_shapes,
            primary_inputs,
            states,
        })
    }

    /// The shape the graph declares for primary input `index`.
    #[must_use]
    pub fn primary_input_shape(&self, index: usize) -> &[i64] {
        &self.input_shapes[index]
    }

    /// Number of state tensors threaded between calls.
    #[must_use]
    pub const fn state_count(&self) -> usize {
        self.states.len()
    }

    /// The declared shape of state tensor `index`.
    #[must_use]
    pub fn state_shape(&self, index: usize) -> &[i64] {
        &self.states[index].shape
    }

    /// Overwrites the reset value of state tensor `index`.
    ///
    /// Used by the `DPDFNet` family, whose state tensor begins with a
    /// normalisation history that has to start from values recorded in the
    /// model's metadata rather than from zero. The state is reset immediately so
    /// the new seed takes effect.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnexpectedSignature`] if `initial` is the wrong length.
    pub fn set_initial_state(&mut self, index: usize, initial: Vec<f32>) -> Result<()> {
        let state = &mut self.states[index];
        if initial.len() != state.data.len() {
            return Err(Error::UnexpectedSignature {
                model: self.model_id.clone(),
                detail: format!(
                    "initial state {index} has {} elements, graph declares {}",
                    initial.len(),
                    state.data.len()
                ),
            });
        }
        state.data.copy_from_slice(&initial);
        state.initial = initial;
        Ok(())
    }

    /// Returns all state tensors to their initial values.
    pub fn reset(&mut self) {
        for state in &mut self.states {
            state.data.copy_from_slice(&state.initial);
        }
    }

    /// Runs one step.
    ///
    /// `primaries` supplies the leading inputs as `(shape, data)` pairs;
    /// `primary_outputs` receives the leading outputs, each of which must
    /// already be the right length. State tensors are supplied and updated
    /// automatically.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Runtime`] if inference fails, or
    /// [`Error::UnexpectedSignature`] if the graph returns a tensor whose size
    /// does not match the destination.
    pub fn run(
        &mut self,
        primaries: &[(&[i64], &[f32])],
        primary_outputs: &mut [&mut [f32]],
    ) -> Result<()> {
        if primaries.len() != self.primary_inputs {
            return Err(Error::UnexpectedSignature {
                model: self.model_id.clone(),
                detail: format!(
                    "caller supplied {} primary inputs, graph takes {}",
                    primaries.len(),
                    self.primary_inputs
                ),
            });
        }

        let mut values = Vec::with_capacity(self.input_names.len());
        for (shape, data) in primaries {
            values.push(TensorRef::from_array_view((*shape, *data))?.into());
        }
        for state in &self.states {
            values.push(
                TensorRef::from_array_view((state.shape.as_slice(), state.data.as_slice()))?.into(),
            );
        }

        let outputs = self.session.run(values.as_slice())?;

        for (index, destination) in primary_outputs.iter_mut().enumerate() {
            let (_, data) =
                outputs[self.output_names[index].as_str()].try_extract_tensor::<f32>()?;
            if data.len() != destination.len() {
                return Err(Error::UnexpectedSignature {
                    model: self.model_id.clone(),
                    detail: format!(
                        "output `{}` returned {} elements, expected {}",
                        self.output_names[index],
                        data.len(),
                        destination.len()
                    ),
                });
            }
            destination.copy_from_slice(data);
        }

        let state_output_offset = self.output_names.len() - self.states.len();
        for (index, state) in self.states.iter_mut().enumerate() {
            let name = self.output_names[state_output_offset + index].as_str();
            let (_, data) = outputs[name].try_extract_tensor::<f32>()?;
            if data.len() != state.data.len() {
                return Err(Error::UnexpectedSignature {
                    model: self.model_id.clone(),
                    detail: format!(
                        "state output `{name}` returned {} elements, expected {}",
                        data.len(),
                        state.data.len()
                    ),
                });
            }
            state.data.copy_from_slice(data);
        }

        Ok(())
    }

    /// Reads a string entry from the model's metadata.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Metadata`] if the key is absent.
    pub fn metadata_string(&self, key: &str) -> Result<String> {
        self.session
            .metadata()?
            .custom(key)
            .ok_or_else(|| Error::Metadata {
                model: self.model_id.clone(),
                key: key.to_owned(),
            })
    }

    /// Reads an integer entry from the model's metadata.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Metadata`] if the key is absent or not an integer.
    pub fn metadata_usize(&self, key: &str) -> Result<usize> {
        self.metadata_string(key)?
            .trim()
            .parse()
            .map_err(|_| Error::Metadata {
                model: self.model_id.clone(),
                key: key.to_owned(),
            })
    }

    /// Reads a comma-separated float list from the model's metadata.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Metadata`] if the key is absent or any element fails to
    /// parse.
    pub fn metadata_floats(&self, key: &str) -> Result<Vec<f32>> {
        let raw = self.metadata_string(key)?;
        raw.split(',')
            .map(|piece| {
                piece.trim().parse::<f32>().map_err(|_| Error::Metadata {
                    model: self.model_id.clone(),
                    key: key.to_owned(),
                })
            })
            .collect()
    }
}

/// The shape input `index` declares.
///
/// Dynamic dimensions come back as non-positive values; callers that cannot
/// tolerate them reject them themselves.
fn declared_shape(model_id: &str, session: &Session, index: usize) -> Result<Vec<i64>> {
    let outlet = &session.inputs()[index];
    let shape = outlet
        .dtype()
        .tensor_shape()
        .ok_or_else(|| Error::UnexpectedSignature {
            model: model_id.to_owned(),
            detail: format!("input `{}` is not a tensor", outlet.name()),
        })?;
    Ok(shape.iter().copied().collect())
}
