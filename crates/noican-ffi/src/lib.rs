//! C ABI used by the `SwiftUI` menu bar control plane.

#![allow(
    unsafe_code,
    reason = "the C ABI must validate and dereference opaque handles and caller-provided byte buffers; all such operations are confined to this file"
)]

use std::{
    ffi::{c_char, c_void, CStr},
    path::PathBuf,
    ptr,
    sync::Mutex,
};

use noican_coreaudio::Runtime;
use noican_engine::{StagePublisher, SwitchingEngine};
use noican_models::{assets::ModelStore, load_pipeline_stage, LoadRequest, ModelId};

const SUCCESS: i32 = 0;
const FAILURE: i32 = -1;
const SWITCH_FADE_SAMPLES: usize = 240;

struct ControlState {
    store: ModelStore,
    runtime: Option<Runtime>,
    publisher: Option<StagePublisher>,
    active_model: Option<ModelId>,
    last_error: String,
}

struct EngineHandle {
    state: Mutex<ControlState>,
}

/// Create an engine control handle.
///
/// A null `model_directory` selects the platform cache.
///
/// # Safety
///
/// A non-null `model_directory` must point to a valid NUL-terminated string
/// for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_create(model_directory: *const c_char) -> *mut c_void {
    let store = if model_directory.is_null() {
        match ModelStore::platform_default() {
            Ok(store) => store,
            Err(_error) => return ptr::null_mut(),
        }
    } else {
        let path = match unsafe { CStr::from_ptr(model_directory) }.to_str() {
            Ok(path) if !path.is_empty() => PathBuf::from(path),
            Ok(_) | Err(_) => return ptr::null_mut(),
        };
        ModelStore::new(path)
    };
    let handle = Box::new(EngineHandle {
        state: Mutex::new(ControlState {
            store,
            runtime: None,
            publisher: None,
            active_model: None,
            last_error: String::new(),
        }),
    });
    Box::into_raw(handle).cast()
}

/// Stop and destroy an engine handle.
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
    if let Ok(state) = handle.state.get_mut() {
        if let Some(mut runtime) = state.runtime.take() {
            runtime.stop();
        }
    }
}

/// Start AUHAL on an already-created private Aggregate Device.
///
/// # Safety
///
/// `handle` must be a live engine handle and `model_slug` must point to a
/// valid NUL-terminated string for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_start(
    handle: *mut c_void,
    aggregate_device: u32,
    model_slug: *const c_char,
) -> i32 {
    let Some(handle) = (unsafe { handle.cast::<EngineHandle>().as_ref() }) else {
        return FAILURE;
    };
    let model = match parse_model(model_slug) {
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
    let prepared = match prepare_stage(&control.store, model) {
        Ok(prepared) => prepared,
        Err(error) => {
            control.last_error = error;
            return FAILURE;
        }
    };
    let (publisher, engine) = match SwitchingEngine::new(prepared, SWITCH_FADE_SAMPLES) {
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

/// Stop AUHAL while preserving the reusable control handle.
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

/// Prepare and lock-free publish a replacement model.
///
/// # Safety
///
/// `handle` must be a live engine handle and `model_slug` must point to a
/// valid NUL-terminated string for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_set_model(
    handle: *mut c_void,
    model_slug: *const c_char,
) -> i32 {
    let Some(handle) = (unsafe { handle.cast::<EngineHandle>().as_ref() }) else {
        return FAILURE;
    };
    let model = match parse_model(model_slug) {
        Ok(model) => model,
        Err(error) => return set_error(handle, error),
    };
    let mut control = match handle.state.lock() {
        Ok(control) => control,
        Err(error) => return set_error(handle, format!("control state is poisoned: {error}")),
    };
    let prepared = match prepare_stage(&control.store, model) {
        Ok(prepared) => prepared,
        Err(error) => {
            control.last_error = error;
            return FAILURE;
        }
    };
    let Some(publisher) = &control.publisher else {
        "engine is not running".clone_into(&mut control.last_error);
        return FAILURE;
    };
    match publisher.publish(prepared) {
        Ok(_superseded) => {
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

/// Return 1 while AUHAL is running, otherwise 0.
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

/// Return 1 after an audio callback, workgroup, or inference fault.
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

/// Copy the latest control-plane error as UTF-8.
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
    copy_string(&state.last_error, buffer, capacity)
}

/// Number of runtime-selectable models.
#[unsafe(no_mangle)]
pub const extern "C" fn noican_model_count() -> usize {
    ModelId::ALL.len()
}

/// Copy a model slug by catalog index.
///
/// Returns the required byte count including the terminating NUL, or zero for
/// an invalid index.
///
/// # Safety
///
/// A non-null `buffer` must be writable for `capacity` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_model_slug(
    index: usize,
    buffer: *mut c_char,
    capacity: usize,
) -> usize {
    ModelId::ALL
        .get(index)
        .map_or(0, |model| copy_string(model.slug(), buffer, capacity))
}

fn prepare_stage(
    store: &ModelStore,
    model: ModelId,
) -> Result<Box<dyn noican_engine::AudioStage>, String> {
    let token = std::env::var("NOICAN_HF_TOKEN").ok();
    load_pipeline_stage(&LoadRequest {
        model,
        store,
        hugging_face_token: token.as_deref(),
        speaker_embedding: None,
    })
    .map_err(|error| error.to_string())
}

fn parse_model(model_slug: *const c_char) -> Result<ModelId, String> {
    if model_slug.is_null() {
        return Err("model slug is null".to_owned());
    }
    let slug = unsafe { CStr::from_ptr(model_slug) }
        .to_str()
        .map_err(|error| format!("model slug is not UTF-8: {error}"))?;
    slug.parse::<ModelId>().map_err(|error| error.to_string())
}

fn set_error(handle: &EngineHandle, error: String) -> i32 {
    if let Ok(mut state) = handle.state.lock() {
        state.last_error = error;
    }
    FAILURE
}

fn copy_string(value: &str, buffer: *mut c_char, capacity: usize) -> usize {
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
    fn catalog_is_available_without_a_handle() {
        assert_eq!(noican_model_count(), ModelId::ALL.len());
    }

    #[test]
    fn string_copy_reports_required_capacity() {
        let mut buffer = [0_i8; 4];
        let required = copy_string("hello", buffer.as_mut_ptr(), buffer.len());
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
}
