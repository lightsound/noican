//! See docs/adr/0011-ffi-boundary.md#ffi-control-locking-and-epoch.

#![expect(
    unsafe_code,
    reason = "the C ABI must validate and dereference opaque handles and caller-provided byte buffers; all such operations are confined to this crate"
)]

use std::{
    ffi::{CStr, c_char, c_void},
    path::PathBuf,
    ptr,
    sync::Mutex,
};

use noican_engine::{Engine, EngineError, Health, ModelState, MonitorState, default_models_dir};
use noican_models::CatalogEntry;

const SUCCESS: i32 = 0;
const FAILURE: i32 = -1;

struct EngineHandle {
    engine: Engine,
    last_error: Mutex<String>,
}

impl EngineHandle {
    fn record(&self, result: Result<(), EngineError>) -> i32 {
        let Ok(mut last_error) = self.last_error.lock() else {
            return FAILURE;
        };
        match result {
            Ok(()) => {
                last_error.clear();
                SUCCESS
            }
            Err(error) => {
                *last_error = error.to_string();
                FAILURE
            }
        }
    }

    fn fail(&self, message: String) -> i32 {
        if let Ok(mut last_error) = self.last_error.lock() {
            *last_error = message;
        }
        FAILURE
    }
}

/// See docs/adr/0005-preview-monitor.md#monitor-state-protocol.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoicanMonitorState {
    /// No preview monitor is up.
    Off = 0,
    /// The preview monitor is playing.
    Playing = 1,
    /// The feedback guard silenced the preview.
    Tripped = 2,
}

/// See docs/adr/0011-ffi-boundary.md#ffi-status-snapshots.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoicanHealth {
    running: bool,
    faulted: bool,
    worker_realtime: bool,
    monitor_device: u32,
    frames_processed: u64,
    output_underruns: u64,
    input_overruns: u64,
    worker_blocks: u64,
    worker_blocks_over_budget: u64,
    worker_block_max_ns: u64,
}

/// See docs/adr/0011-ffi-boundary.md#ffi-status-snapshots.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoicanLevels {
    input: f32,
    output: f32,
    monitor: NoicanMonitorState,
}

/// See docs/adr/0009-model-catalog-fetch.md#model-catalog-projection.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NoicanModelRatings {
    noise_removal: u8,
    voice_quality: u8,
    responsiveness: u8,
    efficiency: u8,
}

/// See docs/adr/0009-model-catalog-fetch.md#model-preparation-status.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoicanModelState {
    /// Weights are not on disk.
    Missing = 0,
    /// A download is in progress.
    Fetching = 1,
    /// Weights are on disk, or the model needs none.
    Ready = 2,
    /// The last preparation found a digest mismatch it could not repair.
    Corrupt = 3,
}

/// See docs/adr/0009-model-catalog-fetch.md#model-preparation-status.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoicanModelStatus {
    state: NoicanModelState,
    received_bytes: u64,
    total_bytes: u64,
}

/// See docs/adr/0011-ffi-boundary.md#ffi-handle-lifecycle.
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
        engine: Engine::new(models_dir),
        last_error: Mutex::new(String::new()),
    });
    Box::into_raw(handle).cast()
}

/// See docs/adr/0011-ffi-boundary.md#ffi-handle-lifecycle.
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
    drop(unsafe { Box::from_raw(handle.cast::<EngineHandle>()) });
}

/// See docs/adr/0004-aggregate-output-routing.md#aggregate-channel-routing.
///
/// # Safety
///
/// `handle` must be a live engine handle and `model_id` must point to a
/// valid NUL-terminated string for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_start(
    handle: *mut c_void,
    aggregate_device: u32,
    virtual_output_first_channel: u32,
    virtual_output_channel_count: u32,
    model_id: *const c_char,
) -> i32 {
    let Some(handle) = (unsafe { engine_handle(handle) }) else {
        return FAILURE;
    };
    let model = match unsafe { model_id_str(model_id) } {
        Ok(model) => model,
        Err(error) => return handle.fail(error),
    };
    handle.record(handle.engine.start_aggregate(
        aggregate_device,
        virtual_output_first_channel,
        virtual_output_channel_count,
        model,
    ))
}

/// See docs/adr/0003-coreaudio-transport.md#split-transport-native-capture.
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
    let Some(handle) = (unsafe { engine_handle(handle) }) else {
        return FAILURE;
    };
    let model = match unsafe { model_id_str(model_id) } {
        Ok(model) => model,
        Err(error) => return handle.fail(error),
    };
    handle.record(handle.engine.start_native(
        input_device,
        output_device,
        capture_sample_rate,
        model,
    ))
}

/// See docs/adr/0011-ffi-boundary.md#ffi-handle-lifecycle.
///
/// # Safety
///
/// `handle` must be null or a live engine handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_stop(handle: *mut c_void) {
    let Some(handle) = (unsafe { engine_handle(handle) }) else {
        return;
    };
    handle.engine.stop();
}

/// See docs/adr/0011-ffi-boundary.md#ffi-control-locking-and-epoch.
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
    let Some(handle) = (unsafe { engine_handle(handle) }) else {
        return FAILURE;
    };
    let model = match unsafe { model_id_str(model_id) } {
        Ok(model) => model,
        Err(error) => return handle.fail(error),
    };
    handle.record(handle.engine.set_model(model))
}

/// See docs/adr/0011-ffi-boundary.md#ffi-status-snapshots.
///
/// # Safety
///
/// `handle` must be null or a live engine handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_health(handle: *const c_void) -> NoicanHealth {
    let health = unsafe { engine_handle(handle) }
        .map_or_else(Health::default, |handle| handle.engine.health());
    NoicanHealth {
        running: health.running,
        faulted: health.faulted,
        worker_realtime: health.worker_realtime,
        monitor_device: health.monitor_device.unwrap_or(0),
        frames_processed: health.frames_processed,
        output_underruns: health.output_underruns,
        input_overruns: health.input_overruns,
        worker_blocks: health.worker_blocks,
        worker_blocks_over_budget: health.worker_blocks_over_budget,
        worker_block_max_ns: health.worker_block_max_ns,
    }
}

/// See docs/adr/0011-ffi-boundary.md#ffi-status-snapshots.
///
/// # Safety
///
/// `handle` must be null or a live engine handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_levels(handle: *const c_void) -> NoicanLevels {
    let Some(handle) = (unsafe { engine_handle(handle) }) else {
        return NoicanLevels {
            input: 0.0,
            output: 0.0,
            monitor: NoicanMonitorState::Off,
        };
    };
    let levels = handle.engine.levels();
    NoicanLevels {
        input: levels.input,
        output: levels.output,
        monitor: match levels.monitor {
            MonitorState::Off => NoicanMonitorState::Off,
            MonitorState::Playing => NoicanMonitorState::Playing,
            MonitorState::Tripped => NoicanMonitorState::Tripped,
        },
    }
}

/// See docs/adr/0009-model-catalog-fetch.md#model-preparation-status.
///
/// # Safety
///
/// `handle` must be null or a live engine handle, and `model_id` null or
/// a valid NUL-terminated string for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_model_status(
    handle: *const c_void,
    model_id: *const c_char,
) -> NoicanModelStatus {
    let status = unsafe { engine_handle(handle) }
        .zip(unsafe { model_id_str(model_id) }.ok())
        .map(|(handle, model)| handle.engine.model_status(model));
    let Some(status) = status else {
        return NoicanModelStatus {
            state: NoicanModelState::Missing,
            received_bytes: 0,
            total_bytes: 0,
        };
    };
    NoicanModelStatus {
        state: match status.state {
            ModelState::Missing => NoicanModelState::Missing,
            ModelState::Fetching => NoicanModelState::Fetching,
            ModelState::Ready => NoicanModelState::Ready,
            ModelState::Corrupt => NoicanModelState::Corrupt,
        },
        received_bytes: status.received_bytes,
        total_bytes: status.total_bytes,
    }
}

/// See docs/adr/0005-preview-monitor.md#preview-monitor-contract.
///
/// # Safety
///
/// `handle` must be null or a live engine handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_set_monitor(handle: *mut c_void, enabled: i32) -> i32 {
    let Some(handle) = (unsafe { engine_handle(handle) }) else {
        return FAILURE;
    };
    handle.record(handle.engine.set_monitor(enabled != 0))
}

/// See docs/adr/0005-preview-monitor.md#preview-monitor-contract.
///
/// # Safety
///
/// A non-null `buffer` must be writable for `capacity` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_monitor_target_error(
    buffer: *mut c_char,
    capacity: usize,
) -> usize {
    match noican_engine::check_monitor_target() {
        Ok(()) => 0,
        Err(error) => unsafe { copy_string(&error.to_string(), buffer, capacity) },
    }
}

/// See docs/adr/0005-preview-monitor.md#preview-monitor-contract.
///
/// # Safety
///
/// A non-null `buffer` must be writable for `capacity` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_monitor_device_error(
    device: u32,
    buffer: *mut c_char,
    capacity: usize,
) -> usize {
    match noican_engine::check_monitor_device(device) {
        Ok(()) => 0,
        Err(error) => unsafe { copy_string(&error.to_string(), buffer, capacity) },
    }
}

/// See docs/adr/0004-aggregate-output-routing.md#transport-routing-diagnostics.
///
/// # Safety
///
/// `handle` must be null or a live engine handle. A non-null `buffer`
/// must be writable for `capacity` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_routing_description(
    handle: *const c_void,
    buffer: *mut c_char,
    capacity: usize,
) -> usize {
    let Some(handle) = (unsafe { engine_handle(handle) }) else {
        return 0;
    };
    let description = handle.engine.routing_description();
    if description.is_empty() {
        0
    } else {
        unsafe { copy_string(&description, buffer, capacity) }
    }
}

/// See docs/adr/0002-engine-diagnostics.md#engine-diagnostics.
///
/// # Safety
///
/// `handle` must be null or a live engine handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_reset_debug_stats(handle: *mut c_void) {
    if let Some(handle) = unsafe { engine_handle(handle) } {
        handle.engine.reset_debug_stats();
    }
}

/// See docs/adr/0007-dsp-stages.md#intensity-dry-wet-mix.
///
/// # Safety
///
/// `handle` must be null or a live engine handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_set_intensity(handle: *mut c_void, intensity: f32) -> i32 {
    let Some(handle) = (unsafe { engine_handle(handle) }) else {
        return FAILURE;
    };
    handle.engine.set_intensity(intensity);
    SUCCESS
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
    let Some(handle) = (unsafe { engine_handle(handle) }) else {
        return 1;
    };
    let Ok(last_error) = handle.last_error.lock() else {
        return 1;
    };
    unsafe { copy_string(&last_error, buffer, capacity) }
}

/// See docs/adr/0009-model-catalog-fetch.md#model-catalog-projection.
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

/// See docs/adr/0009-model-catalog-fetch.md#model-catalog-projection.
#[unsafe(no_mangle)]
pub extern "C" fn noican_model_ratings(index: usize) -> NoicanModelRatings {
    catalog_entry(index).map_or_else(NoicanModelRatings::default, |entry| NoicanModelRatings {
        noise_removal: entry.traits.noise_removal,
        voice_quality: entry.traits.voice_quality,
        responsiveness: entry.traits.responsiveness,
        efficiency: entry.traits.efficiency,
    })
}

unsafe fn model_id_str<'a>(model_id: *const c_char) -> Result<&'a str, String> {
    if model_id.is_null() {
        return Err("model id is null".to_owned());
    }
    unsafe { CStr::from_ptr(model_id) }
        .to_str()
        .map_err(|error| format!("model id is not UTF-8: {error}"))
}

const unsafe fn engine_handle<'a>(handle: *const c_void) -> Option<&'a EngineHandle> {
    unsafe { handle.cast::<EngineHandle>().as_ref() }
}

unsafe fn copy_string(value: &str, buffer: *mut c_char, capacity: usize) -> usize {
    let required = value.len().saturating_add(1);
    if buffer.is_null() || capacity == 0 {
        return required;
    }
    let mut copied = value.len().min(capacity - 1);
    while !value.is_char_boundary(copied) {
        copied -= 1;
    }
    unsafe {
        ptr::copy_nonoverlapping(value.as_ptr(), buffer.cast::<u8>(), copied);
        *buffer.add(copied) = 0;
    }
    required
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use noican_models::{ALL_MODELS, PASSTHROUGH_ID};

    use super::*;

    const HEADER: &str = "../../macos/Sources/CNoican/include/noican.h";

    #[test]
    fn the_checked_in_header_matches_the_abi() {
        let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let config = cbindgen::Config::from_file(crate_dir.join("cbindgen.toml"))
            .expect("cbindgen.toml loads");
        let mut generated = Vec::new();
        cbindgen::Builder::new()
            .with_src(crate_dir.join("src/lib.rs"))
            .with_config(config)
            .generate()
            .expect("the C ABI is expressible in C")
            .write(&mut generated);
        let generated = String::from_utf8(generated).expect("the header is UTF-8");
        let path = crate_dir.join(HEADER);
        if std::env::var_os("NOICAN_UPDATE_HEADER").is_some() {
            std::fs::write(&path, &generated).expect("the header is writable");
        }
        let checked_in = std::fs::read_to_string(&path).expect("the header exists");
        assert!(
            checked_in == generated,
            "{} is out of date; regenerate it with \
             `NOICAN_UPDATE_HEADER=1 cargo test -p noican-ffi header`",
            path.display()
        );
    }

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
        assert_eq!(noican_model_count(), ALL_MODELS.len() + 1);

        let ids: Vec<String> = (0..noican_model_count())
            .map(|index| {
                read_string(|buffer, capacity| unsafe { noican_model_id(index, buffer, capacity) })
                    .expect("every catalog index has an id")
            })
            .collect();
        assert_eq!(ids[0], PASSTHROUGH_ID);
        for spec in ALL_MODELS {
            assert!(ids.iter().any(|id| id == spec.id), "{} missing", spec.id);
        }
    }

    #[test]
    fn display_names_are_exposed() {
        let name = read_string(|buffer, capacity| unsafe {
            noican_model_display_name(0, buffer, capacity)
        })
        .expect("bypass has a display name");
        assert_eq!(name, "Passthrough (no processing)");
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
        let handle = unsafe { noican_engine_create(ptr::null()) };
        assert!(!handle.is_null());
        let last_error = || {
            read_string(|buffer, capacity| unsafe {
                noican_engine_last_error(handle, buffer, capacity)
            })
        };
        let bogus = c"deepfilternet3";
        assert_eq!(
            unsafe { noican_engine_set_model(handle, bogus.as_ptr()) },
            FAILURE
        );
        assert_eq!(
            last_error().as_deref(),
            Some("unknown model: deepfilternet3")
        );
        for known in [c"dfn3", c"passthrough"] {
            assert_eq!(
                unsafe { noican_engine_set_model(handle, known.as_ptr()) },
                FAILURE
            );
            assert_eq!(last_error().as_deref(), Some("engine is not running"));
        }
        assert_eq!(
            unsafe { noican_engine_set_model(handle, ptr::null()) },
            FAILURE
        );
        assert_eq!(last_error().as_deref(), Some("model id is null"));
        unsafe { noican_engine_destroy(handle) };
    }

    const STOPPED: NoicanHealth = NoicanHealth {
        running: false,
        faulted: false,
        worker_realtime: false,
        monitor_device: 0,
        frames_processed: 0,
        output_underruns: 0,
        input_overruns: 0,
        worker_blocks: 0,
        worker_blocks_over_budget: 0,
        worker_block_max_ns: 0,
    };

    fn assert_silent(levels: NoicanLevels) {
        assert!(levels.input.abs() < f32::EPSILON);
        assert!(levels.output.abs() < f32::EPSILON);
        assert_eq!(levels.monitor, NoicanMonitorState::Off);
    }

    #[test]
    fn monitor_calls_are_safe_while_stopped() {
        let handle = unsafe { noican_engine_create(ptr::null()) };
        assert!(!handle.is_null());
        assert_silent(unsafe { noican_engine_levels(handle) });
        assert_eq!(unsafe { noican_engine_set_monitor(handle, 0) }, SUCCESS);
        assert_silent(unsafe { noican_engine_levels(handle) });
        assert_eq!(unsafe { noican_engine_set_monitor(handle, 1) }, FAILURE);
        let error = read_string(|buffer, capacity| unsafe {
            noican_engine_last_error(handle, buffer, capacity)
        })
        .expect("enable failure records an error");
        assert!(error.contains("not running"), "unhelpful message: {error}");
        assert_silent(unsafe { noican_engine_levels(handle) });
        unsafe { noican_engine_destroy(handle) };
    }

    #[test]
    fn snapshots_read_the_stopped_values_for_a_null_or_stopped_handle() {
        assert_eq!(unsafe { noican_engine_health(ptr::null()) }, STOPPED);
        assert_silent(unsafe { noican_engine_levels(ptr::null()) });
        let handle = unsafe { noican_engine_create(ptr::null()) };
        assert!(!handle.is_null());
        assert_eq!(unsafe { noican_engine_health(handle) }, STOPPED);
        assert_silent(unsafe { noican_engine_levels(handle) });
        unsafe { noican_engine_stop(handle) };
        assert_eq!(unsafe { noican_engine_health(handle) }, STOPPED);
        unsafe { noican_engine_destroy(handle) };
    }

    #[test]
    fn model_status_is_null_safe_and_reads_the_catalog() {
        let state = |handle, model: Option<&CStr>| {
            unsafe { noican_engine_model_status(handle, model.map_or(ptr::null(), CStr::as_ptr)) }
                .state
        };
        assert_eq!(
            state(ptr::null(), Some(c"bypass")),
            NoicanModelState::Missing
        );
        let handle = unsafe { noican_engine_create(ptr::null()) };
        assert!(!handle.is_null());
        assert_eq!(state(handle, None), NoicanModelState::Missing);
        assert_eq!(
            state(handle, Some(c"no-such-model")),
            NoicanModelState::Missing
        );
        let passthrough = std::ffi::CString::new(PASSTHROUGH_ID).expect("no interior NUL");
        assert_eq!(state(handle, Some(&passthrough)), NoicanModelState::Ready);
        unsafe { noican_engine_destroy(handle) };
    }

    #[test]
    fn monitor_target_check_is_null_safe_and_consistent() {
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
    fn monitor_device_check_is_null_safe_and_consistent() {
        let required = unsafe { noican_monitor_device_error(0, ptr::null_mut(), 0) };
        if required > 0 {
            let reason = read_string(|buffer, capacity| unsafe {
                noican_monitor_device_error(0, buffer, capacity)
            });
            assert!(reason.is_some(), "nonzero result must yield a reason");
        }
        #[cfg(not(target_os = "macos"))]
        assert!(required > 0, "portable builds always refuse");
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
            let ratings = noican_model_ratings(index);
            for rating in [
                ratings.noise_removal,
                ratings.voice_quality,
                ratings.responsiveness,
                ratings.efficiency,
            ] {
                assert!(rating <= 5, "entry {index}: rating {rating} out of range");
            }
        }
        let bypass = noican_model_ratings(0);
        assert_eq!(bypass.noise_removal, 0);
        assert_eq!(bypass.voice_quality, 5);
    }

    #[test]
    fn model_getters_reject_invalid_indices() {
        let out_of_range = noican_model_count();
        assert_eq!(
            noican_model_ratings(out_of_range),
            NoicanModelRatings::default()
        );
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
    fn intensity_setter_is_null_safe() {
        assert_eq!(
            unsafe { noican_engine_set_intensity(ptr::null_mut(), 0.5) },
            FAILURE
        );
        let handle = unsafe { noican_engine_create(ptr::null()) };
        assert!(!handle.is_null());
        assert_eq!(unsafe { noican_engine_set_intensity(handle, 0.7) }, SUCCESS);
        assert_eq!(
            unsafe { noican_engine_set_intensity(handle, f32::NAN) },
            SUCCESS
        );
        unsafe { noican_engine_destroy(handle) };
    }

    #[test]
    fn start_native_rejects_bad_rates_before_any_slow_work() {
        let handle = unsafe { noican_engine_create(ptr::null()) };
        assert!(!handle.is_null());
        let model = c"passthrough";
        let result = unsafe { noican_engine_start_native(handle, 0, 0, 4_000.0, model.as_ptr()) };
        assert_eq!(result, FAILURE);
        let error = read_string(|buffer, capacity| unsafe {
            noican_engine_last_error(handle, buffer, capacity)
        })
        .expect("rate refusal records an error");
        assert!(
            error.contains("4000") && error.contains("invalid capture sample rate"),
            "unhelpful message: {error}"
        );
        for rate in [16_000.0, 44_100.0] {
            let result = unsafe { noican_engine_start_native(handle, 0, 0, rate, model.as_ptr()) };
            assert_eq!(result, FAILURE);
            let error = read_string(|buffer, capacity| unsafe {
                noican_engine_last_error(handle, buffer, capacity)
            })
            .expect("start failure records an error");
            assert!(
                !error.contains("invalid capture sample rate"),
                "a valid rate must not be blamed: {error}"
            );
            #[cfg(not(target_os = "macos"))]
            assert!(
                error.contains("only on macOS"),
                "portable builds refuse the transport: {error}"
            );
        }
        unsafe { noican_engine_destroy(handle) };
    }

    #[test]
    fn start_rejects_an_empty_virtual_output_range_before_any_slow_work() {
        let handle = unsafe { noican_engine_create(ptr::null()) };
        assert!(!handle.is_null());
        let model = c"passthrough";
        let result = unsafe { noican_engine_start(handle, 0, 2, 0, model.as_ptr()) };
        assert_eq!(result, FAILURE);
        let error = read_string(|buffer, capacity| unsafe {
            noican_engine_last_error(handle, buffer, capacity)
        })
        .expect("range refusal records an error");
        assert!(
            error.starts_with("virtual output routing failed"),
            "unhelpful message: {error}"
        );
        for first in [0, 2] {
            let result = unsafe { noican_engine_start(handle, 0, first, 2, model.as_ptr()) };
            assert_eq!(result, FAILURE);
            let error = read_string(|buffer, capacity| unsafe {
                noican_engine_last_error(handle, buffer, capacity)
            })
            .expect("start failure records an error");
            assert!(
                !error.starts_with("virtual output routing failed: the virtual output reports"),
                "a valid layout must not be blamed: {error}"
            );
            #[cfg(not(target_os = "macos"))]
            assert!(
                error.contains("only on macOS"),
                "portable builds refuse the transport: {error}"
            );
        }
        unsafe { noican_engine_destroy(handle) };
    }

    #[test]
    fn routing_description_is_empty_and_null_safe_while_stopped() {
        assert_eq!(
            unsafe { noican_engine_routing_description(ptr::null(), ptr::null_mut(), 0) },
            0
        );
        let handle = unsafe { noican_engine_create(ptr::null()) };
        assert!(!handle.is_null());
        assert_eq!(
            unsafe { noican_engine_routing_description(handle, ptr::null_mut(), 0) },
            0
        );
        let mut buffer: [c_char; 8] = [0; 8];
        assert_eq!(
            unsafe { noican_engine_routing_description(handle, buffer.as_mut_ptr(), buffer.len()) },
            0
        );
        unsafe { noican_engine_destroy(handle) };
    }

    #[test]
    fn debug_stats_reset_is_null_safe_and_a_no_op_while_stopped() {
        unsafe { noican_engine_reset_debug_stats(ptr::null_mut()) };
        let handle = unsafe { noican_engine_create(ptr::null()) };
        assert!(!handle.is_null());
        unsafe { noican_engine_reset_debug_stats(handle) };
        assert_eq!(unsafe { noican_engine_health(handle) }, STOPPED);
        unsafe { noican_engine_stop(handle) };
        unsafe { noican_engine_reset_debug_stats(handle) };
        unsafe { noican_engine_destroy(handle) };
    }
}
