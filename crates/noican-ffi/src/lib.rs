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
use std::sync::atomic::{AtomicI32, Ordering};

use noican_core::{IntensityControl, Stage, StagePublisher, SwitchingEngine};
use noican_coreaudio::{Runtime, StreamLevels, WORKER_BLOCK_SAMPLES, monitor::MonitorState};
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
    /// The [`MonitorState`] cell shared with each runtime's monitor
    /// control and inference worker. Kept outside the control mutex for
    /// the same reason as `levels`: the UI polls it at 20 Hz and must
    /// never wait on a slow monitor start. Monitor toggles move it
    /// between off and playing, the worker's feedback guard moves it to
    /// tripped, and it reads off whenever no runtime is up.
    monitor_state: Arc<AtomicI32>,
    /// The dry/wet ("strength") control shared with each runtime's
    /// [`SwitchingEngine`]. One atomic, kept outside the control mutex:
    /// slider moves apply instantly without waiting on slow control work
    /// and never rebuild the engine. Owned by the handle (not the
    /// runtime) so the value set while stopped carries into the next
    /// start.
    intensity: IntensityControl,
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
        monitor_state: Arc::new(AtomicI32::new(MonitorState::Off.as_raw())),
        intensity: IntensityControl::default(),
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
    start_with(handle, model, |engine, levels, monitor_state| {
        Runtime::start(aggregate_device, engine, levels, monitor_state)
            .map_err(|error| error.to_string())
    })
}

/// Starts the split transport for a microphone that cannot run at the
/// 48 kHz engine rate (issue #7).
///
/// `input_device` is the microphone's Core Audio device ID and
/// `capture_sample_rate` its current nominal rate in Hz, which must be a
/// proper integer divisor of 48000 (Bluetooth telephony profiles:
/// 8/16/24 kHz). `output_device` is the Noican/`BlackHole` virtual
/// output. No Aggregate Device is involved: the microphone is captured
/// natively and resampled to 48 kHz inside the transport, with clock
/// drift between the two devices compensated by a ring-occupancy servo.
/// The split path adds a 50 ms output cushion; the aggregate path
/// ([`noican_engine_start`]) is unchanged.
///
/// Missing model weights are downloaded first (on this control thread,
/// without holding the control lock).
///
/// # Safety
///
/// `handle` must be a live engine handle and `model_id` must point to a
/// valid NUL-terminated string for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_start_native(
    handle: *mut c_void,
    input_device: u32,
    output_device: u32,
    capture_sample_rate: f64,
    model_id: *const c_char,
) -> i32 {
    let Some(handle) = (unsafe { handle.cast::<EngineHandle>().as_ref() }) else {
        return FAILURE;
    };
    let model = match parse_model_id(model_id) {
        Ok(model) => model,
        Err(error) => return set_error(handle, error),
    };
    let capture_rate = match validate_capture_rate(capture_sample_rate) {
        Ok(rate) => rate,
        Err(error) => return set_error(handle, error),
    };
    start_with(handle, model, |engine, levels, monitor_state| {
        Runtime::start_native(
            input_device,
            output_device,
            capture_rate,
            engine,
            levels,
            monitor_state,
        )
        .map_err(|error| error.to_string())
    })
}

/// Shared start flow behind [`noican_engine_start`] and
/// [`noican_engine_start_native`]: claims an epoch (tearing down any
/// running transport), prepares the stage and engine unlocked, starts
/// the runtime through `start_runtime`, and commits unless a newer
/// control operation superseded the epoch.
fn start_with(
    handle: &EngineHandle,
    model: String,
    start_runtime: impl FnOnce(
        SwitchingEngine,
        Arc<StreamLevels>,
        Arc<AtomicI32>,
    ) -> Result<Runtime, String>,
) -> i32 {
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
    let monitor_state = Arc::clone(&handle.monitor_state);
    let intensity = handle.intensity.clone();
    let built = guard_panics(&model, || {
        let stage = prepare_stage(&models_dir, &model)?;
        let (publisher, engine) =
            SwitchingEngine::new(stage, SWITCH_FADE_SAMPLES, WORKER_BLOCK_SAMPLES, intensity)
                .map_err(|error| error.to_string())?;
        let runtime = start_runtime(engine, levels, monitor_state)?;
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

/// Validates a native capture rate: finite, integral, and a proper
/// integer divisor of the 48 kHz engine rate. Rejecting here — before
/// any weight download or audio object — keeps the failure instant and
/// its message precise.
fn validate_capture_rate(rate: f64) -> Result<u32, String> {
    let rounded = rate.round();
    if !rate.is_finite() || (rate - rounded).abs() > 0.5 || !(1.0..48_000.0).contains(&rounded) {
        return Err(format!(
            "invalid capture sample rate {rate} Hz (expected a telephony-profile \
             rate below 48000 Hz)"
        ));
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "bounded to 1..48000 by the range check above"
    )]
    let hertz = rounded as u32;
    if !48_000_u32.is_multiple_of(hertz) {
        return Err(format!(
            "capture rate {hertz} Hz is not an integer divisor of the 48000 Hz \
             engine rate (Bluetooth telephony profiles are 8/16/24 kHz)"
        ));
    }
    Ok(hertz)
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
/// must not receive the preview (a virtual loopback or an aggregate /
/// multi-output device — the built-in speakers are allowed since the
/// self-monitor AEC cancels their echo), or when the monitor AUHAL
/// cannot start; the meeting-facing path is never affected.
/// Disabling is always a success, including while stopped. Toggling holds
/// the control lock for the monitor start/stop transition — starting an
/// output device can take a moment, so callers should serialize their own
/// control calls behind a busy flag while a toggle is in flight (the
/// level and monitor-state getters stay lock-free and are always safe to
/// poll). A toggle in either direction clears a pending feedback trip
/// (see [`noican_engine_monitor_state`]).
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

/// Checks whether the current system default output may receive the
/// preview, without starting or changing anything.
///
/// Returns 0 when preview can target the device. Otherwise copies the
/// human-readable refusal reason as UTF-8 (loopback / aggregate — the
/// same vetting enabling applies; the built-in speakers are allowed
/// since the self-monitor AEC) and returns the required byte count
/// including the terminating NUL. Cheap (a few Core Audio property
/// reads): the UI calls it before enabling and whenever the system
/// default output changes, so an unsafe target disables the Preview
/// control up front instead of failing after the fact.
///
/// # Safety
///
/// A non-null `buffer` must be writable for `capacity` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_monitor_target_error(
    buffer: *mut c_char,
    capacity: usize,
) -> usize {
    match noican_coreaudio::check_monitor_target() {
        Ok(()) => 0,
        Err(error) => unsafe { copy_string(&error.to_string(), buffer, capacity) },
    }
}

/// Checks whether the device the running preview monitor plays on may
/// *still* receive it.
///
/// After enable time the default output can move on while the monitor
/// stays put, and a built-in device can flip its data source from the
/// headphone jack to the internal speakers without any device-list or
/// default-output notification. The Rust side re-vets the monitor's own
/// device — including comparing the current data source against the one
/// recorded at enable time, so a preview deliberately started on the
/// speakers keeps playing while an unintended jack-unplug flip onto
/// them is reported. A vanished device is *not* reported here
/// (unreadable properties fail open by policy); device loss is visible
/// in the device list and is the caller's check.
///
/// Returns 0 while the monitor's device stays safe, no monitor is up,
/// or the handle is null. Otherwise copies the human-readable reason as
/// UTF-8 and returns the required byte count including the terminating
/// NUL. Reads the control mutex (never held across slow work), so it is
/// meant for event-driven and low-rate callers — not the 20 Hz poll
/// path.
///
/// This check exists to *stop* a preview whose safety is gone, so it
/// fails closed: a poisoned control mutex (a panic elsewhere) reports a
/// reason instead of silently reading as safe — unlike the fail-open
/// getters, whose failure mode is merely a stale reading.
///
/// # Safety
///
/// `handle` must be null or a live engine handle. A non-null `buffer`
/// must be writable for `capacity` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_monitor_unsafe_reason(
    handle: *const c_void,
    buffer: *mut c_char,
    capacity: usize,
) -> usize {
    let Some(handle) = (unsafe { handle.cast::<EngineHandle>().as_ref() }) else {
        return 0;
    };
    let Ok(state) = handle.state.lock() else {
        return unsafe { copy_string("the engine control state is unavailable", buffer, capacity) };
    };
    let reason = state
        .runtime
        .as_ref()
        .and_then(Runtime::monitor_unsafe_reason);
    drop(state);
    reason.map_or(0, |error| unsafe {
        copy_string(&error.to_string(), buffer, capacity)
    })
}

/// Device ID the running preview monitor plays on, or 0 while no monitor
/// AUHAL is up (stopped engine, preview off).
///
/// The target is resolved on the Rust side at enable time and stays
/// fixed until the next toggle, so this is how the UI learns which
/// device to watch for losing its safety
/// (`noican_engine_monitor_unsafe_reason`, device-list changes).
///
/// Reads the control mutex (never held across slow work), so it is meant
/// for event-driven and low-rate callers — not the 20 Hz poll path.
///
/// # Safety
///
/// `handle` must be null or a live engine handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_monitor_device(handle: *const c_void) -> u32 {
    let Some(handle) = (unsafe { handle.cast::<EngineHandle>().as_ref() }) else {
        return 0;
    };
    handle.state.lock().map_or(0, |state| {
        state
            .runtime
            .as_ref()
            .and_then(Runtime::monitor_device)
            .unwrap_or(0)
    })
}

/// The preview monitor's state as one value.
///
/// 0 = off (no monitor AUHAL, including a stopped engine and a null
/// handle), 1 = playing, 2 = tripped. The protocol lives in the
/// [`MonitorState`] enum shared with the engine — including that
/// *tripped* means the monitor AUHAL is still up but silenced (the
/// feedback guard disarmed the tee), and that the next
/// `noican_engine_set_monitor` call in either direction clears it
/// (enable re-arms, disable tears down).
///
/// On 2, callers should disable the monitor to release the playback
/// device and tell the user why; the meeting-facing path is unaffected.
///
/// Reads one atomic without taking the control lock, so it never blocks —
/// safe to poll at UI rates even while a monitor start is in progress.
///
/// # Safety
///
/// `handle` must be null or a live engine handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_monitor_state(handle: *const c_void) -> i32 {
    let Some(handle) = (unsafe { handle.cast::<EngineHandle>().as_ref() }) else {
        return MonitorState::Off.as_raw();
    };
    MonitorState::from_raw(handle.monitor_state.load(Ordering::Acquire)).as_raw()
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

/// Sets the dry/wet intensity ("strength").
///
/// 1.0 is fully processed output, 0.0 is the raw microphone, and values
/// between blend the two with the dry path delay-compensated by the
/// active model's reported latency (so a partial mix does not
/// comb-filter).
///
/// One atomic store — never blocks, never rebuilds the engine, safe at
/// UI slider rates and while stopped (the value carries into the next
/// start). Out-of-range values are clamped; non-finite values are
/// ignored. The same mix feeds the virtual microphone and the preview
/// monitor. Returns 0, or -1 for a null handle.
///
/// # Safety
///
/// `handle` must be null or a live engine handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_set_intensity(handle: *mut c_void, intensity: f32) -> i32 {
    let Some(handle) = (unsafe { handle.cast::<EngineHandle>().as_ref() }) else {
        return FAILURE;
    };
    handle.intensity.set(intensity);
    SUCCESS
}

/// Reads the current dry/wet intensity (0.0–1.0; 1.0 for a null handle,
/// matching the default).
///
/// One atomic load — never blocks, regardless of what the control plane
/// is doing.
///
/// # Safety
///
/// `handle` must be null or a live engine handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_intensity(handle: *const c_void) -> f32 {
    let Some(handle) = (unsafe { handle.cast::<EngineHandle>().as_ref() }) else {
        return 1.0;
    };
    handle.intensity.get()
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

/// Copies a model's one-line picker tagline by catalog index.
///
/// Returns the required byte count including the terminating NUL, or zero
/// for an invalid index.
///
/// # Safety
///
/// A non-null `buffer` must be writable for `capacity` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_model_tagline(
    index: usize,
    buffer: *mut c_char,
    capacity: usize,
) -> usize {
    catalog_entry(index).map_or(0, |entry| unsafe {
        copy_string(entry.traits.tagline, buffer, capacity)
    })
}

/// Copies the raw facts behind a model's ratings (native rate, measured
/// delay, size) by catalog index, for tooltips.
///
/// Returns the required byte count including the terminating NUL, or zero
/// for an invalid index.
///
/// # Safety
///
/// A non-null `buffer` must be writable for `capacity` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_model_details(
    index: usize,
    buffer: *mut c_char,
    capacity: usize,
) -> usize {
    catalog_entry(index).map_or(0, |entry| unsafe {
        copy_string(entry.traits.details, buffer, capacity)
    })
}

/// One picker rating for the model at `index`, 0–5 with "more is better".
///
/// `trait_id` follows `NoicanModelTrait` in noican.h: 0 = noise removal,
/// 1 = voice quality, 2 = responsiveness (inverse latency),
/// 3 = efficiency (inverse compute cost).
///
/// Returns -1 for an invalid index or trait selector.
#[unsafe(no_mangle)]
pub extern "C" fn noican_model_rating(index: usize, trait_id: i32) -> i32 {
    let Some(entry) = catalog_entry(index) else {
        return -1;
    };
    let rating = match trait_id {
        0 => entry.traits.noise_removal,
        1 => entry.traits.voice_quality,
        2 => entry.traits.responsiveness,
        3 => entry.traits.efficiency,
        _ => return -1,
    };
    i32::from(rating)
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
        assert_eq!(
            unsafe { noican_engine_monitor_state(handle) },
            MonitorState::Off.as_raw()
        );
        // Disabling an already-off monitor is an idempotent no-op.
        assert_eq!(unsafe { noican_engine_set_monitor(handle, 0) }, SUCCESS);
        assert_eq!(
            unsafe { noican_engine_monitor_state(handle) },
            MonitorState::Off.as_raw()
        );
        // Enabling without a running engine fails with a clear reason and
        // leaves the state off.
        assert_eq!(unsafe { noican_engine_set_monitor(handle, 1) }, FAILURE);
        let error = read_string(|buffer, capacity| unsafe {
            noican_engine_last_error(handle, buffer, capacity)
        })
        .expect("enable failure records an error");
        assert!(error.contains("not running"), "unhelpful message: {error}");
        assert_eq!(
            unsafe { noican_engine_monitor_state(handle) },
            MonitorState::Off.as_raw()
        );
        unsafe { noican_engine_destroy(handle) };
    }

    #[test]
    fn monitor_state_getter_is_null_safe_and_lock_free_under_state_changes() {
        // Null handles read as off.
        assert_eq!(
            unsafe { noican_engine_monitor_state(ptr::null()) },
            MonitorState::Off.as_raw()
        );
        // The getter reflects every protocol value written to the shared
        // cell (as the monitor control and the worker's trip do), and
        // maps unknown values back to off instead of leaking them.
        let handle = unsafe { noican_engine_create(ptr::null()) };
        assert!(!handle.is_null());
        let engine = unsafe { &*handle.cast::<EngineHandle>() };
        for state in [
            MonitorState::Playing,
            MonitorState::Tripped,
            MonitorState::Off,
        ] {
            engine
                .monitor_state
                .store(state.as_raw(), Ordering::Release);
            assert_eq!(
                unsafe { noican_engine_monitor_state(handle) },
                state.as_raw()
            );
        }
        engine.monitor_state.store(99, Ordering::Release);
        assert_eq!(
            unsafe { noican_engine_monitor_state(handle) },
            MonitorState::Off.as_raw()
        );
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
    fn monitor_target_check_is_null_safe_and_consistent() {
        // The outcome depends on the host's audio devices (and on Linux it
        // is always a refusal), but the contract holds everywhere: a null
        // buffer only measures, and a nonzero result re-reads as a
        // NUL-terminated reason string.
        let required = unsafe { noican_monitor_target_error(ptr::null_mut(), 0) };
        if required > 0 {
            let reason = read_string(|buffer, capacity| unsafe {
                noican_monitor_target_error(buffer, capacity)
            });
            assert!(reason.is_some(), "nonzero result must yield a reason");
        }
        #[cfg(not(target_os = "macos"))]
        assert!(required > 0, "portable builds always refuse");
    }

    #[test]
    fn monitor_device_reads_zero_and_null_safe_while_stopped() {
        let handle = unsafe { noican_engine_create(ptr::null()) };
        assert!(!handle.is_null());
        assert_eq!(unsafe { noican_engine_monitor_device(handle) }, 0);
        assert_eq!(unsafe { noican_engine_monitor_device(ptr::null()) }, 0);
        unsafe { noican_engine_destroy(handle) };
    }

    #[test]
    fn monitor_unsafe_reason_is_null_safe_and_quiet_without_a_monitor() {
        // A null handle and a stopped engine (no runtime, no monitor)
        // both read as "still safe" — the watcher only ever runs while
        // a preview is armed, so silence is the correct default.
        assert_eq!(
            unsafe { noican_engine_monitor_unsafe_reason(ptr::null(), ptr::null_mut(), 0) },
            0
        );
        let handle = unsafe { noican_engine_create(ptr::null()) };
        assert!(!handle.is_null());
        assert_eq!(
            unsafe { noican_engine_monitor_unsafe_reason(handle, ptr::null_mut(), 0) },
            0
        );
        unsafe { noican_engine_destroy(handle) };
    }

    #[test]
    fn model_traits_are_exposed_for_every_catalog_entry() {
        for index in 0..noican_model_count() {
            let tagline = read_string(|buffer, capacity| unsafe {
                noican_model_tagline(index, buffer, capacity)
            })
            .expect("every entry has a tagline");
            assert!(!tagline.is_empty(), "entry {index}: empty tagline");
            let details = read_string(|buffer, capacity| unsafe {
                noican_model_details(index, buffer, capacity)
            })
            .expect("every entry has details");
            assert!(!details.is_empty(), "entry {index}: empty details");
            for trait_id in 0..4 {
                let rating = noican_model_rating(index, trait_id);
                assert!(
                    (0..=5).contains(&rating),
                    "entry {index} trait {trait_id}: rating {rating} out of range"
                );
            }
        }
        // The bypass profile is fixed: nothing removed, everything else full.
        assert_eq!(noican_model_rating(0, 0), 0);
        assert_eq!(noican_model_rating(0, 1), 5);
    }

    #[test]
    fn model_trait_getters_reject_invalid_indices_and_selectors() {
        let out_of_range = noican_model_count();
        assert_eq!(noican_model_rating(out_of_range, 0), -1);
        assert_eq!(noican_model_rating(0, 4), -1);
        assert_eq!(noican_model_rating(0, -1), -1);
        assert_eq!(
            unsafe { noican_model_tagline(out_of_range, ptr::null_mut(), 0) },
            0
        );
        assert_eq!(
            unsafe { noican_model_details(out_of_range, ptr::null_mut(), 0) },
            0
        );
    }

    #[test]
    fn intensity_defaults_to_full_clamps_and_round_trips() {
        let handle = unsafe { noican_engine_create(ptr::null()) };
        assert!(!handle.is_null());
        assert!((unsafe { noican_engine_intensity(handle) } - 1.0).abs() < f32::EPSILON);
        assert_eq!(unsafe { noican_engine_set_intensity(handle, 0.3) }, SUCCESS);
        assert!((unsafe { noican_engine_intensity(handle) } - 0.3).abs() < f32::EPSILON);
        // Out-of-range values clamp; non-finite values are ignored.
        assert_eq!(unsafe { noican_engine_set_intensity(handle, 5.0) }, SUCCESS);
        assert!((unsafe { noican_engine_intensity(handle) } - 1.0).abs() < f32::EPSILON);
        assert_eq!(
            unsafe { noican_engine_set_intensity(handle, -1.0) },
            SUCCESS
        );
        assert!(unsafe { noican_engine_intensity(handle) }.abs() < f32::EPSILON);
        assert_eq!(
            unsafe { noican_engine_set_intensity(handle, f32::NAN) },
            SUCCESS
        );
        assert!(unsafe { noican_engine_intensity(handle) }.abs() < f32::EPSILON);
        unsafe { noican_engine_destroy(handle) };
    }

    #[test]
    fn intensity_calls_are_null_safe_and_value_survives_while_stopped() {
        // Null handles: setter fails cleanly, getter reads the default.
        assert_eq!(
            unsafe { noican_engine_set_intensity(ptr::null_mut(), 0.5) },
            FAILURE
        );
        assert!((unsafe { noican_engine_intensity(ptr::null()) } - 1.0).abs() < f32::EPSILON);
        // The value is owned by the handle, not a runtime: setting it
        // while stopped works and persists (it seeds the next start).
        let handle = unsafe { noican_engine_create(ptr::null()) };
        assert!(!handle.is_null());
        assert_eq!(unsafe { noican_engine_set_intensity(handle, 0.7) }, SUCCESS);
        unsafe { noican_engine_stop(handle) };
        assert!((unsafe { noican_engine_intensity(handle) } - 0.7).abs() < f32::EPSILON);
        unsafe { noican_engine_destroy(handle) };
    }

    #[test]
    fn capture_rate_validation_accepts_only_integer_divisors() {
        for rate in [8_000.0, 12_000.0, 16_000.0, 24_000.0] {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "test rates are small integers"
            )]
            let expected = rate as u32;
            assert_eq!(validate_capture_rate(rate), Ok(expected));
        }
        for rate in [
            0.0,
            -16_000.0,
            44_100.0,
            32_000.0,
            48_000.0,
            96_000.0,
            16_000.7,
            f64::NAN,
            f64::INFINITY,
        ] {
            let error = validate_capture_rate(rate).expect_err("must be rejected");
            assert!(
                error.contains("Hz"),
                "unhelpful message for {rate}: {error}"
            );
        }
    }

    #[test]
    fn start_native_rejects_bad_rates_before_any_slow_work() {
        let handle = unsafe { noican_engine_create(ptr::null()) };
        assert!(!handle.is_null());
        let model = c"passthrough";
        let result = unsafe { noican_engine_start_native(handle, 0, 0, 44_100.0, model.as_ptr()) };
        assert_eq!(result, FAILURE);
        let error = read_string(|buffer, capacity| unsafe {
            noican_engine_last_error(handle, buffer, capacity)
        })
        .expect("rate refusal records an error");
        assert!(
            error.contains("44100") && error.contains("divisor"),
            "unhelpful message: {error}"
        );
        // A valid telephony rate passes validation; on portable builds
        // the transport itself then refuses (and macOS refuses the
        // nonexistent device 0), but never with the rate message.
        let result = unsafe { noican_engine_start_native(handle, 0, 0, 16_000.0, model.as_ptr()) };
        assert_eq!(result, FAILURE);
        let error = read_string(|buffer, capacity| unsafe {
            noican_engine_last_error(handle, buffer, capacity)
        })
        .expect("start failure records an error");
        assert!(
            !error.contains("divisor"),
            "a valid rate must not be blamed: {error}"
        );
        #[cfg(not(target_os = "macos"))]
        assert!(
            error.contains("only on macOS"),
            "portable builds refuse the transport: {error}"
        );
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
