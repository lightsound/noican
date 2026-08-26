//! C ABI used by the `SwiftUI` menu bar control plane.
//!
//! Everything here runs on the control plane (Swift side): model download,
//! stage construction, and engine lifecycle. Prepared stages reach the
//! inference thread through [`noican_core::StagePublisher`]'s lock-free
//! queue, so no call in this module ever blocks the audio path.
//!
//! Locking discipline: the control mutex is held only for short state
//! transitions, never across weight downloads or model construction, so
//! status queries (`is_running`, `is_faulted`, `last_error`) always return
//! promptly — regardless of what the UI does. Slow work runs unlocked and
//! commits its result only when the operation epoch is unchanged (a
//! concurrent `stop`/`start` supersedes it).
//!
//! The model catalog is projected from `noican-models`'
//! [`noican_models::catalog`] at call time — neither this crate nor the UI
//! hardcodes model identifiers or names.

#![expect(
    unsafe_code,
    reason = "the C ABI must validate and dereference opaque handles and caller-provided byte buffers; all such operations are confined to this crate"
)]

use std::{
    ffi::{CStr, c_char, c_void},
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    ptr,
    sync::Mutex,
};

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use noican_core::{Stage, StagePublisher, SwitchingEngine};
use noican_coreaudio::{Runtime, StreamLevels, WORKER_BLOCK_SAMPLES};
use noican_models::{CatalogEntry, ModelSpec, StageOptions};

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
    /// Bumped by every `start`/`stop`; slow operations only commit when the
    /// epoch they claimed is still current.
    epoch: u64,
}

struct EngineHandle {
    state: Mutex<ControlState>,
    /// Peak meters shared with each runtime's inference worker. Kept
    /// outside the control mutex so level polling reads plain atomics and
    /// never waits on slow control work (weight downloads, model loads).
    /// The worker resets it to silence on start and exit, so readers see
    /// 0 whenever the engine is stopped.
    levels: Arc<StreamLevels>,
    /// Feedback-trip flag shared with each runtime's monitor. Kept
    /// outside the control mutex for the same reason as `levels`: the UI
    /// polls it at 20 Hz and must never wait on a slow monitor start.
    /// Cleared by every monitor toggle and on runtime (re)start.
    monitor_tripped: Arc<AtomicBool>,
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
            epoch: 0,
        }),
        levels: Arc::new(StreamLevels::new()),
        monitor_tripped: Arc::new(AtomicBool::new(false)),
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
/// Missing model weights are downloaded first (on this control thread,
/// without holding the control lock).
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

    // Short lock: tear down any running transport and claim an epoch.
    let (models_dir, epoch, old_runtime) = {
        let mut control = match handle.state.lock() {
            Ok(control) => control,
            Err(error) => return set_error(handle, format!("control state is poisoned: {error}")),
        };
        control.epoch += 1;
        let old_runtime = control.runtime.take();
        control.publisher = None;
        control.active_model = None;
        (control.models_dir.clone(), control.epoch, old_runtime)
    };
    if let Some(mut runtime) = old_runtime {
        runtime.stop();
    }

    // Slow work, unlocked: download/construct the stage, start the runtime.
    let levels = Arc::clone(&handle.levels);
    let monitor_tripped = Arc::clone(&handle.monitor_tripped);
    let built = guard_panics(&model, || {
        let stage = prepare_stage(&models_dir, &model)?;
        let (publisher, engine) =
            SwitchingEngine::new(stage, SWITCH_FADE_SAMPLES, WORKER_BLOCK_SAMPLES)
                .map_err(|error| error.to_string())?;
        let runtime = Runtime::start(aggregate_device, engine, levels, monitor_tripped)
            .map_err(|error| error.to_string())?;
        Ok((publisher, runtime))
    });
    let (publisher, mut runtime) = match built {
        Ok(value) => value,
        Err(error) => return set_error(handle, error),
    };

    // Short lock: commit unless a newer stop/start superseded this epoch.
    let mut control = match handle.state.lock() {
        Ok(control) => control,
        Err(error) => {
            runtime.stop();
            return set_error(handle, format!("control state is poisoned: {error}"));
        }
    };
    if control.epoch != epoch {
        drop(control);
        runtime.stop();
        return set_error(
            handle,
            "start was superseded by a newer control operation".to_owned(),
        );
    }
    control.runtime = Some(runtime);
    control.publisher = Some(publisher);
    control.active_model = Some(model);
    control.last_error.clear();
    SUCCESS
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
    let old_runtime = {
        let Ok(mut state) = handle.state.lock() else {
            return;
        };
        state.epoch += 1;
        state.publisher = None;
        state.active_model = None;
        state.runtime.take()
    };
    if let Some(mut runtime) = old_runtime {
        runtime.stop();
    }
}

/// Prepares and lock-free publishes a replacement model.
///
/// Fails fast when the engine is not running; missing model weights are
/// downloaded first (on this control thread, without holding the control
/// lock).
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

    // Short lock: fail fast when stopped, claim the current epoch.
    let (models_dir, publisher, epoch) = {
        let mut control = match handle.state.lock() {
            Ok(control) => control,
            Err(error) => return set_error(handle, format!("control state is poisoned: {error}")),
        };
        let Some(publisher) = control.publisher.clone() else {
            "engine is not running".clone_into(&mut control.last_error);
            return FAILURE;
        };
        (control.models_dir.clone(), publisher, control.epoch)
    };

    // Slow work, unlocked.
    let stage = match guard_panics(&model, || prepare_stage(&models_dir, &model)) {
        Ok(stage) => stage,
        Err(error) => return set_error(handle, error),
    };

    // Short lock: publish unless the engine was stopped/restarted meanwhile.
    let mut control = match handle.state.lock() {
        Ok(control) => control,
        Err(error) => return set_error(handle, format!("control state is poisoned: {error}")),
    };
    if control.epoch != epoch {
        "engine was stopped while the model was loading".clone_into(&mut control.last_error);
        return FAILURE;
    }
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

/// Enables (nonzero) or disables (zero) the preview self-monitor: the
/// processed microphone signal plays on the system default output device.
///
/// The monitor target is resolved to the current default output at enable
/// time; re-enable preview to follow a later default-output change. (A
/// future explicit device selection would arrive as a separate setter,
/// keeping this signature stable.) The monitor does not survive an engine
/// stop/start, so callers re-enable it after `noican_engine_start`.
///
/// Enabling fails when the engine is not running, when the default output
/// must not receive the preview (a virtual loopback, an aggregate /
/// multi-output device, or the built-in speakers), or when the monitor
/// AUHAL cannot start; the meeting-facing path is never affected.
/// Disabling is always a success, including while stopped. Toggling holds
/// the control lock for the monitor start/stop transition — starting an
/// output device can take a moment, so callers should serialize their own
/// control calls behind a busy flag while a toggle is in flight (the
/// level and trip getters stay lock-free and are always safe to poll).
///
/// # Safety
///
/// `handle` must be null or a live engine handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_set_monitor(handle: *mut c_void, enabled: i32) -> i32 {
    let Some(handle) = (unsafe { handle.cast::<EngineHandle>().as_ref() }) else {
        return FAILURE;
    };
    let mut control = match handle.state.lock() {
        Ok(control) => control,
        Err(error) => return set_error(handle, format!("control state is poisoned: {error}")),
    };
    let Some(runtime) = control.runtime.as_mut() else {
        if enabled == 0 {
            control.last_error.clear();
            return SUCCESS;
        }
        "engine is not running".clone_into(&mut control.last_error);
        return FAILURE;
    };
    match runtime.set_monitor(enabled != 0) {
        Ok(()) => {
            control.last_error.clear();
            SUCCESS
        }
        Err(error) => {
            control.last_error = error.to_string();
            FAILURE
        }
    }
}

/// Returns 1 while the preview self-monitor is playing, otherwise 0.
///
/// # Safety
///
/// `handle` must be null or a live engine handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_is_monitoring(handle: *const c_void) -> i32 {
    let Some(handle) = (unsafe { handle.cast::<EngineHandle>().as_ref() }) else {
        return 0;
    };
    handle.state.lock().map_or(0, |state| {
        i32::from(state.runtime.as_ref().is_some_and(Runtime::is_monitoring))
    })
}

/// Returns 1 when the feedback guard auto-stopped the preview tee
/// (sustained near-clipping monitor output — the preview was feeding back
/// into the microphone) since the last monitor toggle, otherwise 0.
///
/// On 1, callers should disable the monitor (`noican_engine_set_monitor`
/// with 0) to release the playback device and tell the user why; the
/// meeting-facing path is unaffected. The flag clears on the next monitor
/// toggle in either direction and on engine (re)start.
///
/// Reads one atomic without taking the control lock, so it never blocks —
/// safe to poll at UI rates even while a monitor start is in progress.
///
/// # Safety
///
/// `handle` must be null or a live engine handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_monitor_tripped(handle: *const c_void) -> i32 {
    let Some(handle) = (unsafe { handle.cast::<EngineHandle>().as_ref() }) else {
        return 0;
    };
    i32::from(handle.monitor_tripped.load(Ordering::Acquire))
}

/// Heartbeat: total input frames delivered since the engine started.
///
/// Returns 0 while stopped. A value that stops advancing while
/// `noican_engine_is_running` reports 1 means the device stopped calling
/// back (unplugged microphone, coreaudiod restart, post-sleep stall); the
/// UI polls this once per second.
///
/// # Safety
///
/// `handle` must be null or a live engine handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_frames_processed(handle: *const c_void) -> u64 {
    let Some(handle) = (unsafe { handle.cast::<EngineHandle>().as_ref() }) else {
        return 0;
    };
    handle.state.lock().map_or(0, |state| {
        state.runtime.as_ref().map_or(0, Runtime::frames_processed)
    })
}

/// Decayed linear peak (0.0–1.0) of the model input (pre-processing),
/// measured per 10 ms worker block; 0.0 while the engine is stopped.
///
/// Reads one atomic without taking the control lock, so it never blocks —
/// regardless of what the control plane is doing.
///
/// # Safety
///
/// `handle` must be null or a live engine handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_input_level(handle: *const c_void) -> f32 {
    let Some(handle) = (unsafe { handle.cast::<EngineHandle>().as_ref() }) else {
        return 0.0;
    };
    handle.levels.input()
}

/// Decayed linear peak (0.0–1.0) of the model output (post-processing),
/// measured per 10 ms worker block; 0.0 while the engine is stopped.
///
/// Reads one atomic without taking the control lock, so it never blocks —
/// regardless of what the control plane is doing.
///
/// # Safety
///
/// `handle` must be null or a live engine handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_output_level(handle: *const c_void) -> f32 {
    let Some(handle) = (unsafe { handle.cast::<EngineHandle>().as_ref() }) else {
        return 0.0;
    };
    handle.levels.output()
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
/// registry catalog.
#[unsafe(no_mangle)]
pub extern "C" fn noican_model_count() -> usize {
    noican_models::catalog().count()
}

fn catalog_entry(index: usize) -> Option<CatalogEntry> {
    noican_models::catalog().nth(index)
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
    catalog_entry(index).map_or(0, |entry| unsafe {
        copy_string(entry.id, buffer, capacity)
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
    catalog_entry(index).map_or(0, |entry| unsafe {
        copy_string(entry.display_name, buffer, capacity)
    })
}

/// Returns 1 when the model at `index` needs a speaker-enrollment
/// embedding (not yet supported by the menu bar app), 0 otherwise.
#[unsafe(no_mangle)]
pub extern "C" fn noican_model_needs_enrollment(index: usize) -> i32 {
    catalog_entry(index).map_or(0, |entry| i32::from(entry.needs_enrollment))
}

/// Runs slow, panic-capable work (ONNX session construction, tar
/// extraction, ...) behind a panic guard: a Rust panic crossing the C ABI
/// would abort the whole app, so it is converted into an error string
/// instead.
fn guard_panics<T>(model_id: &str, work: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    // Nothing observes the closure's state after a panic; the result is
    // discarded wholesale, so broken invariants cannot leak.
    catch_unwind(AssertUnwindSafe(work)).unwrap_or_else(|panic| {
        let message = panic
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic payload".to_owned());
        Err(format!(
            "internal error while preparing {model_id}: {message}"
        ))
    })
}

fn prepare_stage(models_dir: &Path, model_id: &str) -> Result<Box<dyn Stage>, String> {
    // Registry stages may need weights; the bypass (not in the registry's
    // ModelSpec list) needs nothing.
    if let Some(spec) = ModelSpec::find(model_id) {
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
    if !noican_models::catalog().any(|entry| entry.id == id) {
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
    use noican_models::PASSTHROUGH_ID;

    use super::*;

    fn read_string(copy: impl Fn(*mut c_char, usize) -> usize) -> Option<String> {
        let mut buffer: [c_char; 64] = [0; 64];
        let required = copy(buffer.as_mut_ptr(), buffer.len());
        if required == 0 {
            return None;
        }
        Some(
            unsafe { CStr::from_ptr(buffer.as_ptr()) }
                .to_str()
                .expect("FFI strings are UTF-8")
                .to_owned(),
        )
    }

    #[test]
    fn catalog_lists_bypass_and_every_registry_stage() {
        let stage_count = ModelSpec::stages().count();
        assert_eq!(noican_model_count(), stage_count + 1);

        let ids: Vec<String> = (0..noican_model_count())
            .map(|index| {
                read_string(|buffer, capacity| unsafe { noican_model_id(index, buffer, capacity) })
                    .expect("every catalog index has an id")
            })
            .collect();
        assert_eq!(ids[0], PASSTHROUGH_ID);
        for spec in ModelSpec::stages() {
            assert!(ids.iter().any(|id| id == spec.id), "{} missing", spec.id);
        }
    }

    #[test]
    fn display_names_and_enrollment_flags_are_exposed() {
        let name = read_string(|buffer, capacity| unsafe {
            noican_model_display_name(0, buffer, capacity)
        })
        .expect("bypass has a display name");
        assert_eq!(name, "Passthrough (no processing)");
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
    fn string_copy_reports_required_capacity_and_truncates_with_nul() {
        let mut buffer: [c_char; 4] = [0; 4];
        let required = unsafe { copy_string("hello", buffer.as_mut_ptr(), buffer.len()) };
        assert_eq!(required, 6);
        let text = unsafe { CStr::from_ptr(buffer.as_ptr()) }
            .to_str()
            .expect("truncated copy is still NUL-terminated UTF-8");
        assert_eq!(text, "hel");
    }

    #[test]
    fn unknown_model_ids_are_rejected() {
        let bogus = c"deepfilternet3"; // the old candidate-B id scheme
        assert!(parse_model_id(bogus.as_ptr()).is_err());
        let known = c"dfn3";
        assert_eq!(parse_model_id(known.as_ptr()).as_deref(), Ok("dfn3"));
        let bypass = c"passthrough";
        assert_eq!(
            parse_model_id(bypass.as_ptr()).as_deref(),
            Ok(PASSTHROUGH_ID)
        );
    }

    #[test]
    fn monitor_calls_are_safe_while_stopped() {
        let handle = unsafe { noican_engine_create(ptr::null()) };
        assert!(!handle.is_null());
        assert_eq!(unsafe { noican_engine_is_monitoring(handle) }, 0);
        // Disabling an already-off monitor is an idempotent no-op.
        assert_eq!(unsafe { noican_engine_set_monitor(handle, 0) }, SUCCESS);
        // Enabling without a running engine fails with a clear reason.
        assert_eq!(unsafe { noican_engine_set_monitor(handle, 1) }, FAILURE);
        let error = read_string(|buffer, capacity| unsafe {
            noican_engine_last_error(handle, buffer, capacity)
        })
        .expect("enable failure records an error");
        assert!(error.contains("not running"), "unhelpful message: {error}");
        assert_eq!(unsafe { noican_engine_is_monitoring(handle) }, 0);
        assert_eq!(unsafe { noican_engine_monitor_tripped(handle) }, 0);
        unsafe { noican_engine_destroy(handle) };
    }

    #[test]
    fn levels_read_zero_without_blocking_while_stopped() {
        let handle = unsafe { noican_engine_create(ptr::null()) };
        assert!(!handle.is_null());
        assert!(unsafe { noican_engine_input_level(handle) }.abs() < f32::EPSILON);
        assert!(unsafe { noican_engine_output_level(handle) }.abs() < f32::EPSILON);
        // Null handles are tolerated and also read as silence.
        assert!(unsafe { noican_engine_input_level(ptr::null()) }.abs() < f32::EPSILON);
        assert!(unsafe { noican_engine_output_level(ptr::null()) }.abs() < f32::EPSILON);
        unsafe { noican_engine_destroy(handle) };
    }

    #[test]
    fn panics_become_errors_not_aborts() {
        let result: Result<(), String> = guard_panics("test-model", || panic!("boom"));
        let error = result.expect_err("panic must map to an error");
        assert!(error.contains("test-model"), "unhelpful message: {error}");
        assert!(error.contains("boom"), "payload lost: {error}");
    }
}
