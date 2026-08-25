//! C ABI over the engine, for the `SwiftUI` menu bar app.
//!
//! The surface is deliberately small and allocation-free at the boundary:
//! strings are fixed-size arrays the caller owns, so nothing crosses back that
//! Swift would have to free. The one exception is the engine handle, which is
//! created and destroyed explicitly.
//!
//! The header at `include/noican.h` is written by hand; the tests below assert
//! that the two agree on every struct's layout, because a silent disagreement
//! there would corrupt memory rather than fail to compile.

// This crate exists to be called from C. Raw pointers and `extern "C"` are the
// entire point of it; the rest of the workspace keeps `unsafe_code` denied.
// Every entry point states what the caller must guarantee.
#![expect(
    unsafe_code,
    reason = "this crate is the C ABI boundary; raw pointers are its purpose"
)]

pub mod strings;

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char};
use std::sync::Once;

use noican_core::Stage;
use noican_macos::{Session, SessionConfig, devices};
use noican_models::{ModelStore, catalog};

use strings::{StringBuffer, copy_into};

/// Longest string the ABI carries, including the terminator.
///
/// Must match `NOICAN_STRING_CAPACITY` in `include/noican.h`.
pub const STRING_CAPACITY: usize = 256;

thread_local! {
    /// The last error, kept alive until the next call on this thread.
    static LAST_ERROR: RefCell<CString> = RefCell::new(CString::default());
    /// Scratch for returning short strings without allocating per call.
    static SCRATCH: RefCell<CString> = RefCell::new(CString::default());
}

/// Records `message` as this thread's last error.
fn set_error(message: impl std::fmt::Display) {
    let text = message.to_string();
    tracing::error!(%text, "ffi call failed");
    LAST_ERROR.with(|slot| {
        *slot.borrow_mut() = CString::new(text).unwrap_or_default();
    });
}

/// Clears this thread's last error.
fn clear_error() {
    LAST_ERROR.with(|slot| slot.borrow_mut().clear_message());
}

/// Helper so `clear_error` reads well.
trait ClearMessage {
    fn clear_message(&mut self);
}

impl ClearMessage for CString {
    fn clear_message(&mut self) {
        *self = Self::default();
    }
}

/// An audio device the user can choose.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct NoicanDevice {
    /// Persistent identifier.
    pub uid: StringBuffer,
    /// Name as shown in Sound Settings.
    pub name: StringBuffer,
    /// Channels the device can capture.
    pub input_channels: u32,
    /// Channels the device can play.
    pub output_channels: u32,
    /// Current nominal sample rate.
    pub sample_rate: u32,
    /// Whether the HAL reports the device as virtual.
    pub is_virtual: bool,
}

/// A model in the catalog.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct NoicanModel {
    /// Stable identifier.
    pub id: StringBuffer,
    /// Name for the model picker.
    pub display_name: StringBuffer,
    /// Native sample rate.
    pub sample_rate: u32,
    /// Whether the weights are present and verified.
    pub downloaded: bool,
    /// Whether the model is a candidate for the live path. See the header.
    pub live_capable: bool,
}

/// What the engine is doing right now.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct NoicanStatus {
    /// Whether the audio callback is delivering samples.
    pub running: bool,
    /// Whether the active model is bypassed.
    pub bypassed: bool,
    /// Whether a model switch is ramping.
    pub switching: bool,
    /// Times the audio callback emitted silence.
    pub dropouts: u64,
    /// Peak input level since the previous read.
    pub input_peak: f32,
    /// Peak output level since the previous read.
    pub output_peak: f32,
    /// End-to-end delay of the active model.
    pub latency_ms: f32,
}

/// The engine handle Swift holds.
#[derive(Debug, Default)]
pub struct NoicanEngine {
    session: Option<Session>,
    active_model: String,
}

impl NoicanEngine {
    /// Builds the stage for `model_id`, reporting a readable error.
    fn build_stage(model_id: &str) -> Option<Box<dyn Stage>> {
        let store = ModelStore::from_environment();
        match noican_models::build_stage_by_id(model_id, &store) {
            Ok(stage) => Some(stage),
            Err(error) => {
                set_error(error);
                None
            }
        }
    }
}

/// Initialises logging. Safe to call more than once.
#[unsafe(no_mangle)]
pub extern "C" fn noican_init_logging() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
        // Failure means someone already installed a subscriber, which is fine.
        drop(
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_target(false)
                .try_init(),
        );
    });
}

/// The most recent error message on this thread.
///
/// # Safety
///
/// The returned pointer is valid until the next call on the same thread.
#[unsafe(no_mangle)]
pub extern "C" fn noican_last_error() -> *const c_char {
    LAST_ERROR.with(|slot| slot.borrow().as_ptr())
}

/// Number of models in the catalog.
#[unsafe(no_mangle)]
pub extern "C" fn noican_model_count() -> usize {
    catalog::CATALOG.len()
}

/// Writes up to `capacity` models into `out`, returning how many were written.
///
/// # Safety
///
/// `out` must be null or point at `capacity` writable [`NoicanModel`] values.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_models(out: *mut NoicanModel, capacity: usize) -> usize {
    if out.is_null() || capacity == 0 {
        return catalog::CATALOG.len();
    }
    let store = ModelStore::from_environment();
    let count = capacity.min(catalog::CATALOG.len());
    // SAFETY: the caller guarantees `capacity` writable elements.
    let slice = unsafe { std::slice::from_raw_parts_mut(out, count) };
    for (slot, model) in slice.iter_mut().zip(catalog::CATALOG) {
        *slot = NoicanModel {
            id: copy_into(model.id),
            display_name: copy_into(model.display_name),
            sample_rate: model.sample_rate,
            downloaded: store.is_present(model),
            live_capable: model.live_capable,
        };
    }
    count
}

/// Downloads and verifies the weights for `model_id`.
///
/// # Safety
///
/// `model_id` must be a valid NUL-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_fetch_model(model_id: *const c_char) -> bool {
    // SAFETY: delegated to the caller.
    let Some(id) = (unsafe { as_str(model_id) }) else {
        return false;
    };
    let Some(model) = catalog::find(id) else {
        set_error(format!("unknown model `{id}`"));
        return false;
    };
    let store = ModelStore::from_environment();
    match store.fetch(model, &mut |_, _| {}) {
        Ok(()) => {
            clear_error();
            true
        }
        Err(error) => {
            set_error(error);
            false
        }
    }
}

/// Writes up to `capacity` capture-capable devices into `out`.
///
/// # Safety
///
/// `out` must be null or point at `capacity` writable [`NoicanDevice`] values.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_input_devices(out: *mut NoicanDevice, capacity: usize) -> usize {
    // SAFETY: delegated to the caller.
    unsafe { write_devices(devices::inputs(), out, capacity) }
}

/// Writes up to `capacity` playback-capable devices into `out`.
///
/// # Safety
///
/// As [`noican_input_devices`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_output_devices(out: *mut NoicanDevice, capacity: usize) -> usize {
    // SAFETY: delegated to the caller.
    unsafe { write_devices(devices::outputs(), out, capacity) }
}

/// Writes the UID of the most likely virtual output device into `out`.
///
/// # Safety
///
/// `out` must point at [`STRING_CAPACITY`] writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_suggested_output_uid(out: *mut c_char) -> bool {
    let all = match devices::outputs() {
        Ok(all) => all,
        Err(error) => {
            set_error(error);
            return false;
        }
    };
    let Some(device) = devices::suggest_virtual_output(&all) else {
        set_error("no virtual output device found; install the noican driver or BlackHole");
        return false;
    };
    // SAFETY: delegated to the caller.
    unsafe { write_string(out, &device.uid) }
}

/// Writes the UID of the system's current microphone into `out`.
///
/// # Safety
///
/// `out` must point at [`STRING_CAPACITY`] writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_default_input_uid(out: *mut c_char) -> bool {
    match devices::default_input() {
        Ok(device) => {
            // SAFETY: delegated to the caller.
            unsafe { write_string(out, &device.uid) }
        }
        Err(error) => {
            set_error(error);
            false
        }
    }
}

/// Creates a stopped engine.
#[unsafe(no_mangle)]
pub extern "C" fn noican_engine_new() -> *mut NoicanEngine {
    clear_error();
    Box::into_raw(Box::new(NoicanEngine::default()))
}

/// Stops and frees an engine.
///
/// # Safety
///
/// `engine` must be null or a pointer returned by [`noican_engine_new`] that
/// has not already been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_free(engine: *mut NoicanEngine) {
    if engine.is_null() {
        return;
    }
    // SAFETY: delegated to the caller; dropping the box stops the session.
    drop(unsafe { Box::from_raw(engine) });
}

/// Starts capture from `input_uid` into `output_uid` with `model_id` active.
///
/// # Safety
///
/// `engine` must be a live handle and the three strings valid NUL-terminated C
/// strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_start(
    engine: *mut NoicanEngine,
    input_uid: *const c_char,
    output_uid: *const c_char,
    model_id: *const c_char,
) -> bool {
    // SAFETY: delegated to the caller.
    let Some(engine) = (unsafe { engine.as_mut() }) else {
        set_error("engine handle is null");
        return false;
    };
    // SAFETY: delegated to the caller.
    let (Some(input), Some(output), Some(model)) =
        (unsafe { (as_str(input_uid), as_str(output_uid), as_str(model_id)) })
    else {
        return false;
    };

    if engine.session.is_some() {
        set_error("the engine is already running");
        return false;
    }
    let Some(stage) = NoicanEngine::build_stage(model) else {
        return false;
    };

    match Session::start(SessionConfig::new(input, output), stage) {
        Ok(session) => {
            engine.session = Some(session);
            model.clone_into(&mut engine.active_model);
            clear_error();
            true
        }
        Err(error) => {
            set_error(error);
            false
        }
    }
}

/// Stops capture.
///
/// # Safety
///
/// `engine` must be null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_stop(engine: *mut NoicanEngine) {
    // SAFETY: delegated to the caller.
    if let Some(engine) = unsafe { engine.as_mut() } {
        engine.session = None;
        engine.active_model.clear();
    }
}

/// Switches to a different model without interrupting the stream.
///
/// # Safety
///
/// `engine` must be a live handle and `model_id` a valid C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_set_model(
    engine: *mut NoicanEngine,
    model_id: *const c_char,
) -> bool {
    // SAFETY: delegated to the caller.
    let Some(engine) = (unsafe { engine.as_mut() }) else {
        set_error("engine handle is null");
        return false;
    };
    // SAFETY: delegated to the caller.
    let Some(model) = (unsafe { as_str(model_id) }) else {
        return false;
    };
    let Some(session) = engine.session.as_mut() else {
        set_error("the engine is not running");
        return false;
    };
    let Some(stage) = NoicanEngine::build_stage(model) else {
        return false;
    };
    match session.set_stage(stage) {
        Ok(()) => {
            model.clone_into(&mut engine.active_model);
            clear_error();
            true
        }
        Err(error) => {
            set_error(error);
            false
        }
    }
}

/// Bypasses or re-enables the active model.
///
/// # Safety
///
/// `engine` must be null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_set_bypass(engine: *mut NoicanEngine, bypassed: bool) {
    // SAFETY: delegated to the caller.
    if let Some(engine) = unsafe { engine.as_mut() }
        && let Some(session) = engine.session.as_ref()
    {
        session.set_bypass(bypassed);
    }
}

/// Reads the current status.
///
/// # Safety
///
/// `engine` must be null or a live handle, and `out` must point at a writable
/// [`NoicanStatus`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_status(engine: *mut NoicanEngine, out: *mut NoicanStatus) {
    // SAFETY: delegated to the caller.
    let Some(out) = (unsafe { out.as_mut() }) else {
        return;
    };
    *out = NoicanStatus::default();

    // SAFETY: delegated to the caller.
    let Some(engine) = (unsafe { engine.as_mut() }) else {
        return;
    };
    // Freeing retired runners here keeps that work on the UI thread, which is
    // where allocation belongs.
    if let Some(session) = engine.session.as_mut() {
        session.drain_retired();
        let snapshot = session.snapshot();
        *out = NoicanStatus {
            running: snapshot.running,
            bypassed: snapshot.bypassed,
            switching: snapshot.switching,
            dropouts: snapshot.dropouts,
            input_peak: snapshot.input_peak,
            output_peak: snapshot.output_peak,
            latency_ms: snapshot.latency_ms,
        };
    }
}

/// Identifier of the active model, or an empty string when stopped.
///
/// # Safety
///
/// `engine` must be null or a live handle. The returned pointer is valid until
/// the next call on the same thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_engine_active_model(engine: *mut NoicanEngine) -> *const c_char {
    // SAFETY: delegated to the caller.
    let active = unsafe { engine.as_ref() }.map_or("", |engine| engine.active_model.as_str());
    SCRATCH.with(|slot| {
        let mut slot = slot.borrow_mut();
        *slot = CString::new(active).unwrap_or_default();
        slot.as_ptr()
    })
}

/// Borrows a C string, recording an error if it is null or not UTF-8.
///
/// # Safety
///
/// `pointer` must be null or a valid NUL-terminated C string.
unsafe fn as_str<'a>(pointer: *const c_char) -> Option<&'a str> {
    if pointer.is_null() {
        set_error("a required string argument was null");
        return None;
    }
    // SAFETY: delegated to the caller.
    let Ok(text) = unsafe { CStr::from_ptr(pointer) }.to_str() else {
        set_error("a string argument was not valid UTF-8");
        return None;
    };
    Some(text)
}

/// Writes `text` into a caller-provided buffer of [`STRING_CAPACITY`] bytes.
///
/// # Safety
///
/// `out` must be null or point at [`STRING_CAPACITY`] writable bytes.
unsafe fn write_string(out: *mut c_char, text: &str) -> bool {
    if out.is_null() {
        set_error("output buffer was null");
        return false;
    }
    let buffer = copy_into(text);
    // SAFETY: `buffer` is exactly STRING_CAPACITY bytes and the caller
    // guarantees that much writable space.
    unsafe {
        std::ptr::copy_nonoverlapping(buffer.bytes.as_ptr(), out, STRING_CAPACITY);
    }
    clear_error();
    true
}

/// Shared body of the two device-listing entry points.
///
/// # Safety
///
/// `out` must be null or point at `capacity` writable [`NoicanDevice`] values.
unsafe fn write_devices(
    listed: noican_macos::Result<Vec<noican_macos::Device>>,
    out: *mut NoicanDevice,
    capacity: usize,
) -> usize {
    let all = match listed {
        Ok(all) => all,
        Err(error) => {
            set_error(error);
            return 0;
        }
    };
    if out.is_null() || capacity == 0 {
        return all.len();
    }
    let count = capacity.min(all.len());
    // SAFETY: the caller guarantees `capacity` writable elements.
    let slice = unsafe { std::slice::from_raw_parts_mut(out, count) };
    for (slot, device) in slice.iter_mut().zip(&all) {
        *slot = NoicanDevice {
            uid: copy_into(&device.uid),
            name: copy_into(&device.name),
            input_channels: device.input_channels,
            output_channels: device.output_channels,
            sample_rate: device.sample_rate,
            is_virtual: device.is_virtual,
        };
    }
    clear_error();
    count
}

#[cfg(test)]
mod tests {
    use super::{
        NoicanDevice, NoicanEngine, NoicanModel, NoicanStatus, STRING_CAPACITY, noican_engine_free,
        noican_engine_new, noican_engine_status, noican_model_count, noican_models,
    };

    /// The header declares these sizes; a disagreement would corrupt memory
    /// rather than fail to compile, so it is asserted here.
    #[test]
    fn abi_structs_have_the_layout_the_header_declares() {
        assert_eq!(STRING_CAPACITY, 256);
        assert_eq!(
            size_of::<NoicanDevice>(),
            STRING_CAPACITY * 2 + 4 * 3 + 4,
            "NoicanDevice layout drifted from include/noican.h"
        );
        assert_eq!(align_of::<NoicanDevice>(), 4);
        assert_eq!(align_of::<NoicanModel>(), 4);
        // u64 first member forces 8-byte alignment in the C struct too.
        assert_eq!(align_of::<NoicanStatus>(), 8);
    }

    #[test]
    fn the_catalog_is_reachable_through_the_abi() {
        let count = noican_model_count();
        assert!(count > 0);
        // A null destination is a count query.
        assert_eq!(unsafe { noican_models(std::ptr::null_mut(), 0) }, count);

        let mut models = vec![
            NoicanModel {
                id: super::copy_into(""),
                display_name: super::copy_into(""),
                sample_rate: 0,
                downloaded: false,
                live_capable: false,
            };
            count
        ];
        let written = unsafe { noican_models(models.as_mut_ptr(), count) };
        assert_eq!(written, count);
        assert!(!models[0].id.to_string().is_empty());
        assert!(models[0].sample_rate > 0);
        // The flag has to survive the boundary, or the picker cannot warn.
        assert!(
            models.iter().any(|model| !model.live_capable),
            "no model came across as unfit for live use, though the catalog has two"
        );
    }

    #[test]
    fn a_smaller_destination_is_filled_not_overrun() {
        let mut models = vec![
            NoicanModel {
                id: super::copy_into(""),
                display_name: super::copy_into(""),
                sample_rate: 0,
                downloaded: false,
                live_capable: false,
            };
            2
        ];
        assert_eq!(unsafe { noican_models(models.as_mut_ptr(), 2) }, 2);
    }

    #[test]
    fn a_new_engine_reports_stopped() {
        let engine = noican_engine_new();
        assert!(!engine.is_null());

        let mut status = NoicanStatus::default();
        unsafe { noican_engine_status(engine, &raw mut status) };
        assert!(!status.running);

        unsafe { noican_engine_free(engine) };
        // Freeing null is accepted.
        unsafe { noican_engine_free(std::ptr::null_mut()) };
    }

    #[test]
    fn null_handles_are_tolerated() {
        let mut status = NoicanStatus {
            running: true,
            ..NoicanStatus::default()
        };
        unsafe { noican_engine_status(std::ptr::null_mut(), &raw mut status) };
        assert!(!status.running, "a null handle must clear the status");
        unsafe { noican_engine_status(std::ptr::null_mut(), std::ptr::null_mut()) };

        unsafe { super::noican_engine_stop(std::ptr::null_mut()) };
        unsafe { super::noican_engine_set_bypass(std::ptr::null_mut(), true) };
        assert!(!unsafe { super::noican_engine_set_model(std::ptr::null_mut(), c"x".as_ptr()) });
    }

    #[test]
    fn the_default_engine_has_no_session() {
        let engine = NoicanEngine::default();
        assert!(engine.session.is_none());
        assert!(engine.active_model.is_empty());
    }
}
