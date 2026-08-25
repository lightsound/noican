//! C ABI used by the `SwiftUI` menu bar control plane.
//!
//! Everything here runs on the control plane (Swift side): model download,
//! stage construction, and engine lifecycle. Prepared stages reach the
//! inference thread through [`noican_core::StagePublisher`]'s lock-free
//! queue, so no call in this module ever blocks the audio path.
//!
//! The model catalog is derived from `noican-models`' registry
//! ([`noican_models::ALL_MODELS`]) at call time — the UI never hardcodes
//! model identifiers.

#![expect(
    unsafe_code,
    reason = "the C ABI must validate and dereference opaque handles and caller-provided byte buffers; all such operations are confined to this crate"
)]

use std::{
    ffi::{CStr, c_char, c_void},
    path::{Path, PathBuf},
    ptr,
    sync::Mutex,
};

use noican_core::{Stage, StagePublisher, SwitchingEngine};
use noican_coreaudio::{Runtime, WORKER_BLOCK_SAMPLES};
use noican_models::{ModelSpec, PASSTHROUGH_ID, StageOptions};

const SUCCESS: i32 = 0;
const FAILURE: i32 = -1;
/// Fade half-length for model switches: 5 ms at the 48 kHz engine rate.
const SWITCH_FADE_SAMPLES: usize = 240;

struct ControlState {
    models_dir: PathBuf,
    runtime: Option<Runtime>,
    publisher: Option<StagePublisher>,
    active_model: Option<String>,
    last_error: String,
}

struct EngineHandle {
    state: Mutex<ControlState>,
}

/// The selectable model catalog: the built-in bypass followed by every
/// registry entry usable as a pipeline stage.
fn catalog() -> impl Iterator<Item = Option<&'static ModelSpec>> {
    std::iter::once(None).chain(ModelSpec::stages().map(Some))
}

fn default_models_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("NOICAN_MODELS_DIR") {
        return PathBuf::from(dir);
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::home_dir() {
        return home.join("Library/Application Support/noican/models");
    }
    PathBuf::from("models")
}

/// Creates an engine control handle.
///
/// A null `models_directory` selects `NOICAN_MODELS_DIR` or the platform
/// default (`~/Library/Application Support/noican/models` on macOS).
///
/// # Safety
///
/// A non-null `models_directory` must point to a valid NUL-terminated
/// string for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_create(models_directory: *const c_char) -> *mut c_void {
    let models_dir = if models_directory.is_null() {
        default_models_dir()
    } else {
        match unsafe { CStr::from_ptr(models_directory) }.to_str() {
            Ok(path) if !path.is_empty() => PathBuf::from(path),
            Ok(_) | Err(_) => return ptr::null_mut(),
        }
    };
    let handle = Box::new(EngineHandle {
        state: Mutex::new(ControlState {
            models_dir,
            runtime: None,
            publisher: None,
            active_model: None,
            last_error: String::new(),
        }),
    });
    Box::into_raw(handle).cast()
}

/// Stops and destroys an engine handle.
///
/// # Safety
///
/// `handle` must be null or a live pointer returned by
/// [`noican_engine_create`], and it must be destroyed at most once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_destroy(handle: *mut c_void) {
    if handle.is_null() {
        return;
    }
    let mut handle = unsafe { Box::from_raw(handle.cast::<EngineHandle>()) };
    if let Ok(state) = handle.state.get_mut()
        && let Some(mut runtime) = state.runtime.take()
    {
        runtime.stop();
    }
}

/// Starts AUHAL on an already-created private Aggregate Device.
///
/// Missing model weights are downloaded first (on this control thread).
///
/// # Safety
///
/// `handle` must be a live engine handle and `model_id` must point to a
/// valid NUL-terminated string for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_start(
    handle: *mut c_void,
    aggregate_device: u32,
    model_id: *const c_char,
) -> i32 {
    let Some(handle) = (unsafe { handle.cast::<EngineHandle>().as_ref() }) else {
        return FAILURE;
    };
    let model = match parse_model_id(model_id) {
        Ok(model) => model,
        Err(error) => return set_error(handle, error),
    };
    let mut control = match handle.state.lock() {
        Ok(control) => control,
        Err(error) => return set_error(handle, format!("control state is poisoned: {error}")),
    };
    if let Some(mut runtime) = control.runtime.take() {
        runtime.stop();
    }
    control.publisher = None;
    control.active_model = None;
    let stage = match prepare_stage(&control.models_dir, &model) {
        Ok(stage) => stage,
        Err(error) => {
            control.last_error = error;
            return FAILURE;
        }
    };
    let (publisher, engine) =
        match SwitchingEngine::new(stage, SWITCH_FADE_SAMPLES, WORKER_BLOCK_SAMPLES) {
            Ok(value) => value,
            Err(error) => {
                control.last_error = error.to_string();
                return FAILURE;
            }
        };
    match Runtime::start(aggregate_device, engine) {
        Ok(runtime) => {
            control.runtime = Some(runtime);
            control.publisher = Some(publisher);
            control.active_model = Some(model);
            control.last_error.clear();
            SUCCESS
        }
        Err(error) => {
            control.last_error = error.to_string();
            FAILURE
        }
    }
}

/// Stops AUHAL while preserving the reusable control handle.
///
/// # Safety
///
/// `handle` must be null or a live engine handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_stop(handle: *mut c_void) {
    let Some(handle) = (unsafe { handle.cast::<EngineHandle>().as_ref() }) else {
        return;
    };
    let Ok(mut state) = handle.state.lock() else {
        return;
    };
    if let Some(mut runtime) = state.runtime.take() {
        runtime.stop();
    }
    state.publisher = None;
    state.active_model = None;
}

/// Prepares and lock-free publishes a replacement model.
///
/// Missing model weights are downloaded first (on this control thread).
///
/// # Safety
///
/// `handle` must be a live engine handle and `model_id` must point to a
/// valid NUL-terminated string for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_set_model(
    handle: *mut c_void,
    model_id: *const c_char,
) -> i32 {
    let Some(handle) = (unsafe { handle.cast::<EngineHandle>().as_ref() }) else {
        return FAILURE;
    };
    let model = match parse_model_id(model_id) {
        Ok(model) => model,
        Err(error) => return set_error(handle, error),
    };
    let mut control = match handle.state.lock() {
        Ok(control) => control,
        Err(error) => return set_error(handle, format!("control state is poisoned: {error}")),
    };
    let stage = match prepare_stage(&control.models_dir, &model) {
        Ok(stage) => stage,
        Err(error) => {
            control.last_error = error;
            return FAILURE;
        }
    };
    let Some(publisher) = &control.publisher else {
        "engine is not running".clone_into(&mut control.last_error);
        return FAILURE;
    };
    let _superseded = publisher.publish(stage);
    control.active_model = Some(model);
    control.last_error.clear();
    SUCCESS
}

/// Returns 1 while AUHAL is running, otherwise 0.
///
/// # Safety
///
/// `handle` must be null or a live engine handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_is_running(handle: *const c_void) -> i32 {
    let Some(handle) = (unsafe { handle.cast::<EngineHandle>().as_ref() }) else {
        return 0;
    };
    handle.state.lock().map_or(0, |state| {
        i32::from(state.runtime.as_ref().is_some_and(Runtime::is_running))
    })
}

/// Returns 1 after an audio callback, workgroup, or inference fault.
///
/// # Safety
///
/// `handle` must be null or a live engine handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_is_faulted(handle: *const c_void) -> i32 {
    let Some(handle) = (unsafe { handle.cast::<EngineHandle>().as_ref() }) else {
        return 0;
    };
    handle.state.lock().map_or(1, |state| {
        i32::from(state.runtime.as_ref().is_some_and(Runtime::is_faulted))
    })
}

/// Copies the latest control-plane error as UTF-8.
///
/// Returns the required byte count including the terminating NUL.
///
/// # Safety
///
/// `handle` must be null or live. A non-null `buffer` must be writable for
/// `capacity` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_last_error(
    handle: *const c_void,
    buffer: *mut c_char,
    capacity: usize,
) -> usize {
    let Some(handle) = (unsafe { handle.cast::<EngineHandle>().as_ref() }) else {
        return 1;
    };
    let Ok(state) = handle.state.lock() else {
        return 1;
    };
    unsafe { copy_string(&state.last_error, buffer, capacity) }
}

/// Number of runtime-selectable models (bypass included), taken from the
/// registry.
#[unsafe(no_mangle)]
pub extern "C" fn noican_model_count() -> usize {
    catalog().count()
}

/// Copies a model id by catalog index.
///
/// Returns the required byte count including the terminating NUL, or zero
/// for an invalid index.
///
/// # Safety
///
/// A non-null `buffer` must be writable for `capacity` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_model_id(
    index: usize,
    buffer: *mut c_char,
    capacity: usize,
) -> usize {
    catalog().nth(index).map_or(0, |spec| {
        let id = spec.map_or(PASSTHROUGH_ID, |spec| spec.id);
        unsafe { copy_string(id, buffer, capacity) }
    })
}

/// Copies a model's human-readable display name by catalog index.
///
/// Returns the required byte count including the terminating NUL, or zero
/// for an invalid index.
///
/// # Safety
///
/// A non-null `buffer` must be writable for `capacity` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_model_display_name(
    index: usize,
    buffer: *mut c_char,
    capacity: usize,
) -> usize {
    catalog().nth(index).map_or(0, |spec| {
        let name = spec.map_or("Off (bypass)", |spec| spec.display_name);
        unsafe { copy_string(name, buffer, capacity) }
    })
}

/// Returns 1 when the model at `index` needs a speaker-enrollment
/// embedding (not yet supported by the menu bar app), 0 otherwise.
#[unsafe(no_mangle)]
pub extern "C" fn noican_model_needs_enrollment(index: usize) -> i32 {
    catalog().nth(index).map_or(0, |spec| {
        i32::from(spec.is_some_and(|spec| spec.needs_enrollment))
    })
}

fn prepare_stage(models_dir: &Path, model_id: &str) -> Result<Box<dyn Stage>, String> {
    if model_id != PASSTHROUGH_ID {
        let spec = ModelSpec::find(model_id).ok_or_else(|| format!("unknown model: {model_id}"))?;
        if spec.needs_enrollment {
            return Err(format!(
                "{model_id} needs a speaker enrollment, which the menu bar app does not support yet"
            ));
        }
        if !noican_models::fetch::is_fetched(models_dir, spec) {
            noican_models::fetch::fetch_model(models_dir, spec, |_line| {})
                .map_err(|error| format!("downloading {model_id} weights failed: {error}"))?;
        }
    }
    noican_models::create_stage(model_id, models_dir, &StageOptions::default())
        .map_err(|error| format!("loading {model_id} failed: {error}"))
}

fn parse_model_id(model_id: *const c_char) -> Result<String, String> {
    if model_id.is_null() {
        return Err("model id is null".to_owned());
    }
    let id = unsafe { CStr::from_ptr(model_id) }
        .to_str()
        .map_err(|error| format!("model id is not UTF-8: {error}"))?;
    if id != PASSTHROUGH_ID && ModelSpec::find(id).is_none() {
        return Err(format!("unknown model: {id}"));
    }
    Ok(id.to_owned())
}

fn set_error(handle: &EngineHandle, error: String) -> i32 {
    if let Ok(mut state) = handle.state.lock() {
        state.last_error = error;
    }
    FAILURE
}

unsafe fn copy_string(value: &str, buffer: *mut c_char, capacity: usize) -> usize {
    let required = value.len().saturating_add(1);
    if buffer.is_null() || capacity == 0 {
        return required;
    }
    let copied = value.len().min(capacity - 1);
    unsafe {
        ptr::copy_nonoverlapping(value.as_ptr(), buffer.cast::<u8>(), copied);
        *buffer.add(copied) = 0;
    }
    required
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_lists_bypass_and_every_registry_stage() {
        let stage_count = ModelSpec::stages().count();
        assert_eq!(noican_model_count(), stage_count + 1);

        let mut ids = Vec::new();
        for index in 0..noican_model_count() {
            let mut buffer = [0_i8; 64];
            let required = unsafe { noican_model_id(index, buffer.as_mut_ptr(), buffer.len()) };
            assert!(required > 1, "catalog index {index} has no id");
            let id = unsafe { CStr::from_ptr(buffer.as_ptr()) }
                .to_str()
                .expect("model ids are UTF-8")
                .to_owned();
            ids.push(id);
        }
        assert_eq!(ids[0], PASSTHROUGH_ID);
        for spec in ModelSpec::stages() {
            assert!(ids.iter().any(|id| id == spec.id), "{} missing", spec.id);
        }
    }

    #[test]
    fn display_names_and_enrollment_flags_are_exposed() {
        let mut buffer = [0_i8; 64];
        let required = unsafe { noican_model_display_name(0, buffer.as_mut_ptr(), buffer.len()) };
        assert!(required > 1);
        let name = unsafe { CStr::from_ptr(buffer.as_ptr()) }
            .to_str()
            .expect("display names are UTF-8");
        assert_eq!(name, "Off (bypass)");
        assert_eq!(noican_model_needs_enrollment(0), 0);

        // tse-48k is the only enrollment-gated stage in the registry today.
        let enrollment_gated = (0..noican_model_count())
            .filter(|&index| noican_model_needs_enrollment(index) == 1)
            .count();
        assert_eq!(
            enrollment_gated,
            ModelSpec::stages().filter(|s| s.needs_enrollment).count()
        );
    }

    #[test]
    fn string_copy_reports_required_capacity() {
        let mut buffer = [0_i8; 4];
        let required = unsafe { copy_string("hello", buffer.as_mut_ptr(), buffer.len()) };
        assert_eq!(required, 6);
        assert_eq!(
            &buffer,
            &[
                b'h'.cast_signed(),
                b'e'.cast_signed(),
                b'l'.cast_signed(),
                0
            ]
        );
    }

    #[test]
    fn unknown_model_ids_are_rejected() {
        let bogus = c"deepfilternet3"; // the old candidate-B id scheme
        assert!(parse_model_id(bogus.as_ptr()).is_err());
        let known = c"dfn3";
        assert_eq!(parse_model_id(known.as_ptr()).as_deref(), Ok("dfn3"));
    }
}
