//! C ABI for the noican engine, consumed by the SwiftUI menu-bar app.
//!
//! Conventions:
//!
//! - All functions are thread-safe as long as a given `NoicanHandle` is
//!   used from one thread at a time (the Swift side calls from the main
//!   actor).
//! - Functions returning `*mut c_char` return UTF-8 strings the caller
//!   must free with [`noican_string_free`]; JSON is used for structured
//!   data to keep the surface tiny.
//! - Functions returning `i32` use `0` for success; on failure call
//!   [`noican_last_error`] for a message.
//!
//! The corresponding header lives at `app/Sources/CNoican/include/noican.h`
//! and must be kept in sync manually.

use std::ffi::{CStr, CString, c_char};
use std::path::PathBuf;

use noican_models::{ModelSpec, PASSTHROUGH_ID, StageOptions};
use noican_rt::{EngineConfig, RtEngine};

/// Opaque engine handle.
#[derive(Debug)]
pub struct NoicanHandle {
    models_dir: PathBuf,
    engine: Option<RtEngine>,
    last_error: Option<CString>,
}

fn escape_json(s: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

fn to_c_string(s: &str) -> *mut c_char {
    CString::new(s).map_or(std::ptr::null_mut(), CString::into_raw)
}

impl NoicanHandle {
    fn set_error(&mut self, message: &str) -> i32 {
        self.last_error = CString::new(message).ok();
        -1
    }

    fn build_stage(
        &mut self,
        model_id: &str,
        enroll_wav: Option<&str>,
    ) -> Option<Box<dyn noican_core::Stage>> {
        let options = match enroll_wav {
            None => StageOptions::default(),
            Some(path) => match enrollment_from_wav(&self.models_dir, path) {
                Ok(embedding) => StageOptions {
                    enrollment: Some(embedding),
                },
                Err(message) => {
                    self.set_error(&message);
                    return None;
                }
            },
        };
        match noican_models::create_stage(model_id, &self.models_dir, &options) {
            Ok(stage) => Some(stage),
            Err(e) => {
                self.set_error(&format!("creating stage {model_id}: {e}"));
                None
            }
        }
    }
}

/// Reads a WAV, converts to 16 kHz mono, and computes the ECAPA
/// enrollment embedding.
fn enrollment_from_wav(models_dir: &std::path::Path, wav_path: &str) -> Result<Vec<f32>, String> {
    let spec =
        ModelSpec::find("ecapa-tdnn").ok_or_else(|| "ecapa-tdnn not in registry".to_owned())?;
    if !noican_models::fetch::is_fetched(models_dir, spec) {
        return Err("ecapa-tdnn model not fetched (run: noican fetch ecapa-tdnn)".to_owned());
    }
    let mut reader =
        hound_free_wav_reader(wav_path).map_err(|e| format!("reading {wav_path}: {e}"))?;
    let audio_16k = std::mem::take(&mut reader);
    let onnx = noican_models::fetch::model_dir(models_dir, spec).join(spec.files[0].name);
    let mut embedder = noican_models::embedding::EcapaEmbedder::new(&onnx)
        .map_err(|e| format!("loading ECAPA: {e}"))?;
    embedder
        .embed(&audio_16k)
        .map_err(|e| format!("embedding: {e}"))
}

/// Minimal 16 kHz mono WAV decode without extra dependencies: the FFI
/// crate reuses the models crate's resampler-free path by requiring the
/// Swift side to hand over a 16 kHz or 48 kHz mono/stereo PCM WAV.
fn hound_free_wav_reader(path: &str) -> Result<Vec<f32>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let wav = parse_wav(&bytes)?;
    match wav.sample_rate {
        16_000 => Ok(wav.samples),
        48_000 => {
            let mut decimator = noican_core::resample::Decimator::new(3, wav.samples.len().max(3));
            let usable = wav.samples.len() - wav.samples.len() % 3;
            let mut out = Vec::with_capacity(usable / 3);
            decimator.process(&wav.samples[..usable], &mut out);
            Ok(out)
        }
        other => Err(format!(
            "enrollment WAV must be 16 or 48 kHz (got {other} Hz)"
        )),
    }
}

struct ParsedWav {
    sample_rate: u32,
    samples: Vec<f32>,
}

/// Tiny RIFF/WAVE PCM parser (16-bit PCM or 32-bit float, any channel
/// count, mixed down to mono).
fn parse_wav(bytes: &[u8]) -> Result<ParsedWav, String> {
    const fn u16_at(b: &[u8], off: usize) -> u16 {
        u16::from_le_bytes([b[off], b[off + 1]])
    }
    const fn u32_at(b: &[u8], off: usize) -> u32 {
        u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
    }
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".to_owned());
    }
    let mut pos = 12;
    let mut format: Option<(u16, u16, u32, u16)> = None; // (tag, channels, rate, bits)
    let mut data: Option<&[u8]> = None;
    while pos + 8 <= bytes.len() {
        let chunk_id = &bytes[pos..pos + 4];
        let chunk_len = u32_at(bytes, pos + 4) as usize;
        let body_start = pos + 8;
        let body_end = (body_start + chunk_len).min(bytes.len());
        match chunk_id {
            b"fmt " if chunk_len >= 16 => {
                format = Some((
                    u16_at(bytes, body_start),
                    u16_at(bytes, body_start + 2),
                    u32_at(bytes, body_start + 4),
                    u16_at(bytes, body_start + 14),
                ));
            }
            b"data" => data = Some(&bytes[body_start..body_end]),
            _ => {}
        }
        pos = body_start + chunk_len + (chunk_len & 1);
    }
    let (tag, channels, rate, bits) = format.ok_or_else(|| "missing fmt chunk".to_owned())?;
    let data = data.ok_or_else(|| "missing data chunk".to_owned())?;
    let channels = usize::from(channels.max(1));
    let interleaved: Vec<f32> = match (tag, bits) {
        (1, 16) => data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| f32::from(i16::from_le_bytes(*b)) / 32768.0)
            .collect(),
        (3, 32) => data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect(),
        (tag, bits) => return Err(format!("unsupported WAV encoding: tag {tag}, {bits} bit")),
    };
    #[allow(
        clippy::cast_precision_loss,
        reason = "channel counts are tiny; exact f32 representation"
    )]
    let inv = 1.0 / channels as f32;
    let samples = interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() * inv)
        .collect();
    Ok(ParsedWav {
        sample_rate: rate,
        samples,
    })
}

// ---------------------------------------------------------------------
// C API
// ---------------------------------------------------------------------

/// Creates a handle. `models_dir` is the directory holding downloaded
/// model weights.
///
/// # Safety
///
/// `models_dir` must be a valid NUL-terminated UTF-8 string pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_new(models_dir: *const c_char) -> *mut NoicanHandle {
    if models_dir.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: caller guarantees a valid NUL-terminated string.
    let dir = unsafe { CStr::from_ptr(models_dir) };
    let Ok(dir) = dir.to_str() else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(NoicanHandle {
        models_dir: PathBuf::from(dir),
        engine: None,
        last_error: None,
    }))
}

/// Destroys a handle (stopping the engine if running).
///
/// # Safety
///
/// `handle` must come from [`noican_new`] and not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_free(handle: *mut NoicanHandle) {
    if !handle.is_null() {
        // SAFETY: caller passes ownership back.
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// Returns the last error message (caller frees) or NULL.
///
/// # Safety
///
/// `handle` must be a live handle from [`noican_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_last_error(handle: *mut NoicanHandle) -> *mut c_char {
    // SAFETY: caller guarantees a live handle.
    let handle = unsafe { &mut *handle };
    handle
        .last_error
        .as_ref()
        .map_or(std::ptr::null_mut(), |e| to_c_string(&e.to_string_lossy()))
}

/// Frees a string returned by this library.
///
/// # Safety
///
/// `s` must have been returned by a `noican_*` function and not freed
/// before.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_string_free(s: *mut c_char) {
    if !s.is_null() {
        // SAFETY: reclaiming a CString we handed out.
        drop(unsafe { CString::from_raw(s) });
    }
}

/// JSON array of selectable input devices:
/// `[{"uid": "...", "name": "..."}]`. Caller frees.
#[unsafe(no_mangle)]
pub extern "C" fn noican_list_input_devices() -> *mut c_char {
    let devices = noican_rt::engine::list_input_devices().unwrap_or_default();
    let items: Vec<String> = devices
        .iter()
        .map(|d| {
            format!(
                "{{\"uid\":\"{}\",\"name\":\"{}\"}}",
                escape_json(&d.uid),
                escape_json(&d.name)
            )
        })
        .collect();
    to_c_string(&format!("[{}]", items.join(",")))
}

/// JSON array of selectable models:
/// `[{"id": "...", "name": "...", "fetched": true, "needsEnrollment": false}]`.
/// Includes the built-in passthrough. Caller frees.
///
/// # Safety
///
/// `handle` must be a live handle from [`noican_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_list_models(handle: *mut NoicanHandle) -> *mut c_char {
    // SAFETY: caller guarantees a live handle.
    let handle = unsafe { &mut *handle };
    let mut items = vec![format!(
        "{{\"id\":\"{PASSTHROUGH_ID}\",\"name\":\"Passthrough\",\"fetched\":true,\"needsEnrollment\":false}}"
    )];
    for spec in ModelSpec::stages() {
        let fetched =
            spec.files.is_empty() || noican_models::fetch::is_fetched(&handle.models_dir, spec);
        items.push(format!(
            "{{\"id\":\"{}\",\"name\":\"{}\",\"fetched\":{},\"needsEnrollment\":{}}}",
            escape_json(spec.id),
            escape_json(spec.display_name),
            fetched,
            spec.needs_enrollment
        ));
    }
    to_c_string(&format!("[{}]", items.join(",")))
}

/// Starts the engine: `input_uid` (NULL = default input) → `model_id` →
/// the first output device whose name starts with "BlackHole".
/// `enroll_wav` (nullable) is required by enrollment models.
///
/// # Safety
///
/// `handle` must be live; string pointers must be NULL or valid
/// NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_start(
    handle: *mut NoicanHandle,
    input_uid: *const c_char,
    model_id: *const c_char,
    enroll_wav: *const c_char,
) -> i32 {
    // SAFETY: caller guarantees a live handle and valid string pointers.
    let handle = unsafe { &mut *handle };
    if handle.engine.is_some() {
        return handle.set_error("engine already running");
    }
    let read = |p: *const c_char| -> Option<String> {
        if p.is_null() {
            None
        } else {
            // SAFETY: caller guarantees valid NUL-terminated strings.
            unsafe { CStr::from_ptr(p) }
                .to_str()
                .ok()
                .map(str::to_owned)
        }
    };
    let model_id = read(model_id).unwrap_or_else(|| PASSTHROUGH_ID.to_owned());
    let enroll = read(enroll_wav);
    let Some(stage) = handle.build_stage(&model_id, enroll.as_deref()) else {
        return -1;
    };
    let config = EngineConfig {
        input_device_uid: read(input_uid),
        ..EngineConfig::default()
    };
    match RtEngine::start(&config, stage, &model_id) {
        Ok(engine) => {
            handle.engine = Some(engine);
            0
        }
        Err(e) => handle.set_error(&format!("starting engine: {e}")),
    }
}

/// Stops the engine (no-op when not running).
///
/// # Safety
///
/// `handle` must be a live handle from [`noican_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_stop(handle: *mut NoicanHandle) {
    // SAFETY: caller guarantees a live handle.
    let handle = unsafe { &mut *handle };
    handle.engine = None;
}

/// Switches the active model with a click-free crossfade.
///
/// # Safety
///
/// `handle` must be live; string pointers must be NULL or valid
/// NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_set_model(
    handle: *mut NoicanHandle,
    model_id: *const c_char,
    enroll_wav: *const c_char,
) -> i32 {
    // SAFETY: caller guarantees a live handle and valid string pointers.
    let handle = unsafe { &mut *handle };
    if model_id.is_null() {
        return handle.set_error("model_id is NULL");
    }
    // SAFETY: checked non-null; caller guarantees NUL termination.
    let Ok(model_id) = unsafe { CStr::from_ptr(model_id) }.to_str() else {
        return handle.set_error("model_id is not UTF-8");
    };
    let model_id = model_id.to_owned();
    let enroll = if enroll_wav.is_null() {
        None
    } else {
        // SAFETY: checked non-null; caller guarantees NUL termination.
        unsafe { CStr::from_ptr(enroll_wav) }
            .to_str()
            .ok()
            .map(str::to_owned)
    };
    let Some(stage) = handle.build_stage(&model_id, enroll.as_deref()) else {
        return -1;
    };
    let Some(engine) = handle.engine.as_mut() else {
        return handle.set_error("engine not running");
    };
    match engine.switch_model(&model_id, stage) {
        Ok(()) => 0,
        Err(e) => handle.set_error(&format!("switching model: {e}")),
    }
}

/// JSON status snapshot:
/// `{"running":bool,"model":"id","blocks":n,"underruns":n,"overruns":n,"stageFailed":bool}`.
/// Caller frees.
///
/// # Safety
///
/// `handle` must be a live handle from [`noican_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn noican_status_json(handle: *mut NoicanHandle) -> *mut c_char {
    use std::sync::atomic::Ordering;
    // SAFETY: caller guarantees a live handle.
    let handle = unsafe { &mut *handle };
    let json = handle.engine.as_ref().map_or_else(
        || "{\"running\":false}".to_owned(),
        |engine| {
            let status = engine.status();
            format!(
                "{{\"running\":true,\"model\":\"{}\",\"blocks\":{},\"underruns\":{},\"overruns\":{},\"stageFailed\":{}}}",
                escape_json(engine.current_model()),
                status.blocks_processed.load(Ordering::Relaxed),
                status.underruns.load(Ordering::Relaxed),
                status.overruns.load(Ordering::Relaxed),
                status.stage_failed.load(Ordering::Relaxed)
            )
        },
    );
    to_c_string(&json)
}
