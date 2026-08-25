//! The real-time engine: aggregate-device I/O, SPSC plumbing, inference
//! thread, and lock-free model switching.

use std::sync::atomic::{AtomicBool, AtomicU64};

#[cfg(not(target_os = "macos"))]
use noican_core::Stage;

/// Engine block size in samples at 48 kHz (10 ms — matches the dominant
/// model hop so most blocks map 1:1 onto model frames).
pub const BLOCK_LEN: usize = 480;

/// Errors from the real-time engine.
#[derive(Debug, thiserror::Error)]
pub enum RtError {
    /// A Core Audio call failed.
    #[error("Core Audio error while {context}: OSStatus {status}")]
    CoreAudio {
        /// What the engine was doing.
        context: String,
        /// The raw `OSStatus`.
        status: i32,
    },
    /// Invalid configuration or environment.
    #[error("configuration error: {0}")]
    Config(String),
    /// A processing stage failed.
    #[error("stage error: {0}")]
    Stage(#[from] noican_core::StageError),
    /// The engine is not available on this platform.
    #[error("the real-time engine is only available on macOS")]
    Unsupported,
}

/// An input device selectable by the user.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    /// Stable device UID.
    pub uid: String,
    /// Human-readable name.
    pub name: String,
}

/// Engine configuration.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// UID of the physical input device (`None` = system default input).
    pub input_device_uid: Option<String>,
    /// Name prefix of the virtual output device to feed.
    pub output_device_name_prefix: String,
    /// Hardware I/O buffer size in frames.
    pub buffer_frames: u32,
    /// Zeros pre-queued on the output ring: absorbs inference jitter at
    /// the cost of latency.
    pub prime_output_samples: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            input_device_uid: None,
            output_device_name_prefix: "BlackHole".to_owned(),
            buffer_frames: 256,
            // 20 ms of scheduling headroom for the inference thread.
            prime_output_samples: 960,
        }
    }
}

/// Live counters shared between the audio thread, the inference thread,
/// and the control plane.
#[derive(Debug, Default)]
pub struct EngineStatus {
    /// Blocks processed by the inference thread.
    pub blocks_processed: AtomicU64,
    /// Output-ring underruns observed by the audio thread (glitches).
    pub underruns: AtomicU64,
    /// Input-ring overruns (inference thread too slow; samples dropped).
    pub overruns: AtomicU64,
    /// Set when the inference thread hit a stage error and went to bypass.
    pub stage_failed: AtomicBool,
}

#[cfg(target_os = "macos")]
mod macos {
    use std::cell::UnsafeCell;
    use std::ffi::c_void;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    use noican_core::{ENGINE_SAMPLE_RATE, Stage, StageSwitcher, SwitchHandle, switch};

    use super::{BLOCK_LEN, DeviceInfo, EngineConfig, EngineStatus, RtError};
    use crate::coreaudio::{
        self, AggregateDevice, IoProcHandle,
        ffi::{AudioBufferList, AudioObjectID, AudioTimeStamp, OSStatus},
    };

    /// Lists devices that have at least one input stream.
    ///
    /// # Errors
    ///
    /// Returns [`RtError::CoreAudio`] when device enumeration fails.
    pub fn list_input_devices() -> Result<Vec<DeviceInfo>, RtError> {
        let mut out = Vec::new();
        for device in coreaudio::all_devices()? {
            let inputs = coreaudio::stream_count(device, coreaudio::input_scope()).unwrap_or(0);
            if inputs == 0 {
                continue;
            }
            let name = coreaudio::device_name(device).unwrap_or_default();
            let uid = coreaudio::device_uid(device).unwrap_or_default();
            if uid.is_empty() {
                continue;
            }
            out.push(DeviceInfo { uid, name });
        }
        Ok(out)
    }

    fn find_device_by_uid(uid: &str) -> Result<AudioObjectID, RtError> {
        for device in coreaudio::all_devices()? {
            if coreaudio::device_uid(device).unwrap_or_default() == uid {
                return Ok(device);
            }
        }
        Err(RtError::Config(format!("no device with uid {uid}")))
    }

    fn find_output_by_name_prefix(prefix: &str) -> Result<AudioObjectID, RtError> {
        for device in coreaudio::all_devices()? {
            let outputs = coreaudio::stream_count(device, coreaudio::output_scope()).unwrap_or(0);
            if outputs == 0 {
                continue;
            }
            if coreaudio::device_name(device)
                .unwrap_or_default()
                .starts_with(prefix)
            {
                return Ok(device);
            }
        }
        Err(RtError::Config(format!(
            "no output device named {prefix}* found — is the virtual device installed?"
        )))
    }

    /// Shared state between the IOProc (audio thread) and everything else.
    ///
    /// The `UnsafeCell`s hold the audio-thread ends of the SPSC rings and a
    /// preallocated scratch buffer; the audio thread is their only
    /// accessor after start.
    struct IoContext {
        input_producer: UnsafeCell<rtrb::Producer<f32>>,
        output_consumer: UnsafeCell<rtrb::Consumer<f32>>,
        scratch: UnsafeCell<Vec<f32>>,
        /// Input buffers `[0, mic_input_buffers)` belong to the mic.
        mic_input_buffers: usize,
        /// Output buffers `[offset, ..)` belong to the virtual device.
        output_buffer_offset: usize,
        status: Arc<EngineStatus>,
    }

    // SAFETY: the UnsafeCell contents are accessed exclusively from the
    // audio thread (single IOProc); the atomics are thread-safe. The
    // struct is shared only so the control thread can keep it alive.
    #[allow(
        unsafe_code,
        reason = "audio-thread-exclusive cells; see IoContext docs"
    )]
    unsafe impl Sync for IoContext {}
    #[allow(
        unsafe_code,
        reason = "audio-thread-exclusive cells; see IoContext docs"
    )]
    unsafe impl Send for IoContext {}

    /// The audio-thread callback: hardware buffers ↔ SPSC rings only.
    /// No allocation, no locks, no inference (docs/tech-research.md §9).
    unsafe extern "C-unwind" fn io_proc(
        _device: AudioObjectID,
        _now: *const AudioTimeStamp,
        input_data: *const AudioBufferList,
        _input_time: *const AudioTimeStamp,
        output_data: *mut AudioBufferList,
        _output_time: *const AudioTimeStamp,
        client_data: *mut c_void,
    ) -> OSStatus {
        // SAFETY: client_data is the IoContext kept alive by RtEngine until
        // after the IOProc is destroyed.
        let ctx = unsafe { &*client_data.cast::<IoContext>() };

        // ---- Input: mic buffers → input ring (mono mixdown). ----
        if !input_data.is_null() {
            // SAFETY: the HAL passes a valid buffer list for this cycle.
            let list = unsafe { &*input_data };
            let buffers = unsafe {
                std::slice::from_raw_parts(list.mBuffers.as_ptr(), list.mNumberBuffers as usize)
            };
            // SAFETY: sole audio-thread accessor of the producer cell.
            let producer = unsafe { &mut *ctx.input_producer.get() };
            let mut overrun = 0_u64;
            for buffer in buffers.iter().take(ctx.mic_input_buffers) {
                let channels = buffer.mNumberChannels.max(1) as usize;
                let samples = buffer.mDataByteSize as usize / size_of::<f32>();
                if buffer.mData.is_null() || samples == 0 {
                    continue;
                }
                // SAFETY: HAL guarantees mData holds mDataByteSize bytes of
                // f32 samples for this cycle.
                let data =
                    unsafe { std::slice::from_raw_parts(buffer.mData.cast::<f32>(), samples) };
                #[allow(
                    clippy::cast_precision_loss,
                    reason = "channel counts are tiny; exact f32 representation"
                )]
                let inv = 1.0 / channels as f32;
                for frame in data.chunks_exact(channels) {
                    let mono: f32 = frame.iter().sum::<f32>() * inv;
                    if producer.push(mono).is_err() {
                        overrun += 1;
                    }
                }
            }
            if overrun > 0 {
                ctx.status.overruns.fetch_add(overrun, Ordering::Relaxed);
            }
        }

        // ---- Output: output ring → virtual-device buffers. ----
        if !output_data.is_null() {
            // SAFETY: the HAL passes a valid, writable buffer list.
            let list = unsafe { &mut *output_data };
            let buffers = unsafe {
                std::slice::from_raw_parts_mut(
                    list.mBuffers.as_mut_ptr(),
                    list.mNumberBuffers as usize,
                )
            };
            // Zero everything first (including any hardware-output buffers
            // of the input device — never leak audio to speakers).
            let mut max_frames = 0_usize;
            for buffer in buffers.iter_mut() {
                if buffer.mData.is_null() {
                    continue;
                }
                let samples = buffer.mDataByteSize as usize / size_of::<f32>();
                // SAFETY: writable per HAL contract for this cycle.
                let data =
                    unsafe { std::slice::from_raw_parts_mut(buffer.mData.cast::<f32>(), samples) };
                data.fill(0.0);
                let channels = buffer.mNumberChannels.max(1) as usize;
                max_frames = max_frames.max(samples / channels);
            }
            // SAFETY: sole audio-thread accessor of the cells.
            let consumer = unsafe { &mut *ctx.output_consumer.get() };
            let scratch = unsafe { &mut *ctx.scratch.get() };
            let frames = max_frames.min(scratch.len());
            let mut underrun = 0_u64;
            for slot in scratch.iter_mut().take(frames) {
                *slot = consumer.pop().unwrap_or_else(|_| {
                    underrun += 1;
                    0.0
                });
            }
            if underrun > 0 {
                ctx.status.underruns.fetch_add(underrun, Ordering::Relaxed);
            }
            for buffer in buffers.iter_mut().skip(ctx.output_buffer_offset) {
                if buffer.mData.is_null() {
                    continue;
                }
                let channels = buffer.mNumberChannels.max(1) as usize;
                let samples = buffer.mDataByteSize as usize / size_of::<f32>();
                // SAFETY: writable per HAL contract for this cycle.
                let data =
                    unsafe { std::slice::from_raw_parts_mut(buffer.mData.cast::<f32>(), samples) };
                for (frame_idx, frame) in data.chunks_exact_mut(channels).enumerate() {
                    let sample = scratch.get(frame_idx).copied().unwrap_or(0.0);
                    frame.fill(sample);
                }
            }
        }
        0
    }

    /// Running real-time engine (macOS).
    pub struct RtEngine {
        switch: SwitchHandle,
        stop: Arc<AtomicBool>,
        inference: Option<std::thread::JoinHandle<()>>,
        status: Arc<EngineStatus>,
        current_model: String,
        // Drop order: the IOProc must stop before the context and the
        // aggregate device are torn down (fields drop in declaration
        // order).
        _ioproc: IoProcHandle,
        _ctx: Box<IoContext>,
        _aggregate: AggregateDevice,
    }

    impl std::fmt::Debug for RtEngine {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("RtEngine")
                .field("current_model", &self.current_model)
                .finish_non_exhaustive()
        }
    }

    impl RtEngine {
        /// Builds the aggregate device, primes the rings, starts the
        /// inference thread and the IOProc.
        ///
        /// # Errors
        ///
        /// Returns [`RtError`] when devices are missing or the HAL rejects
        /// any step.
        pub fn start(
            config: &EngineConfig,
            initial_stage: Box<dyn Stage>,
            initial_model_id: &str,
        ) -> Result<Self, RtError> {
            let input_device = match &config.input_device_uid {
                Some(uid) => find_device_by_uid(uid)?,
                None => coreaudio::default_input_device()?,
            };
            let input_uid = coreaudio::device_uid(input_device)?;
            let output_device = find_output_by_name_prefix(&config.output_device_name_prefix)?;
            let output_uid = coreaudio::device_uid(output_device)?;
            if input_uid == output_uid {
                return Err(RtError::Config(
                    "input and output devices must differ".to_owned(),
                ));
            }

            // The engine runs at 48 kHz end to end.
            coreaudio::set_nominal_sample_rate(input_device, f64::from(ENGINE_SAMPLE_RATE))?;
            coreaudio::set_nominal_sample_rate(output_device, f64::from(ENGINE_SAMPLE_RATE))?;

            let mic_input_buffers =
                coreaudio::stream_count(input_device, coreaudio::input_scope())?;
            // In the aggregate's buffer lists, streams appear in sub-device
            // order: the mic's output streams (usually zero) come before
            // the virtual device's.
            let output_buffer_offset =
                coreaudio::stream_count(input_device, coreaudio::output_scope())?;

            let aggregate = AggregateDevice::create(&input_uid, &output_uid)?;
            coreaudio::set_nominal_sample_rate(aggregate.id(), f64::from(ENGINE_SAMPLE_RATE))?;
            coreaudio::set_buffer_frame_size(aggregate.id(), config.buffer_frames)?;

            // One second of headroom on both rings.
            let ring_len = ENGINE_SAMPLE_RATE as usize;
            let (input_producer, mut input_consumer) = rtrb::RingBuffer::new(ring_len);
            let (mut output_producer, output_consumer) = rtrb::RingBuffer::new(ring_len);
            for _ in 0..config.prime_output_samples {
                let _ = output_producer.push(0.0);
            }

            let status = Arc::new(EngineStatus::default());
            let stop = Arc::new(AtomicBool::new(false));

            let (switch, mut switcher) =
                StageSwitcher::new(initial_stage, BLOCK_LEN, switch::DEFAULT_CROSSFADE_SAMPLES);

            let inference = {
                let status = Arc::clone(&status);
                let stop = Arc::clone(&stop);
                std::thread::Builder::new()
                    .name("noican-inference".to_owned())
                    .spawn(move || {
                        inference_loop(
                            &mut input_consumer,
                            &mut output_producer,
                            &mut switcher,
                            &status,
                            &stop,
                        );
                    })
                    .map_err(|e| RtError::Config(format!("spawning inference thread: {e}")))?
            };

            let ctx = Box::new(IoContext {
                input_producer: UnsafeCell::new(input_producer),
                output_consumer: UnsafeCell::new(output_consumer),
                scratch: UnsafeCell::new(vec![0.0; 8192]),
                mic_input_buffers: mic_input_buffers.max(1),
                output_buffer_offset,
                status: Arc::clone(&status),
            });
            let ctx_ptr = std::ptr::from_ref::<IoContext>(&ctx).cast_mut().cast();
            // SAFETY: `ctx` outlives the IOProc handle (field drop order in
            // RtEngine destroys the IOProc first).
            let ioproc =
                unsafe { IoProcHandle::install_and_start(aggregate.id(), io_proc, ctx_ptr)? };

            Ok(Self {
                switch,
                stop,
                inference: Some(inference),
                status,
                current_model: initial_model_id.to_owned(),
                _ioproc: ioproc,
                _ctx: ctx,
                _aggregate: aggregate,
            })
        }

        /// Queues a switch to `stage` (crossfaded on the processing
        /// thread).
        ///
        /// # Errors
        ///
        /// Returns [`RtError::Config`] when the switch queue is full.
        pub fn switch_model(
            &mut self,
            model_id: &str,
            stage: Box<dyn Stage>,
        ) -> Result<(), RtError> {
            self.switch
                .switch_to(stage)
                .map_err(|_| RtError::Config("switch queue full; retry shortly".to_owned()))?;
            model_id.clone_into(&mut self.current_model);
            Ok(())
        }

        /// Identifier of the most recently requested model.
        #[must_use]
        pub fn current_model(&self) -> &str {
            &self.current_model
        }

        /// Live counters.
        #[must_use]
        pub fn status(&self) -> &EngineStatus {
            &self.status
        }
    }

    impl Drop for RtEngine {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(handle) = self.inference.take() {
                let _ = handle.join();
            }
            // IOProc, context, and aggregate are torn down by field drops.
        }
    }

    fn inference_loop(
        input: &mut rtrb::Consumer<f32>,
        output: &mut rtrb::Producer<f32>,
        switcher: &mut StageSwitcher,
        status: &EngineStatus,
        stop: &AtomicBool,
    ) {
        let mut block_in = [0.0_f32; BLOCK_LEN];
        let mut block_out = [0.0_f32; BLOCK_LEN];
        let mut bypass = false;
        while !stop.load(Ordering::Acquire) {
            if input.slots() < BLOCK_LEN {
                // ~1/10 of a block; cheap enough and low-latency enough.
                std::thread::sleep(std::time::Duration::from_millis(1));
                continue;
            }
            for slot in &mut block_in {
                *slot = input.pop().unwrap_or(0.0);
            }
            let result = if bypass {
                block_out.copy_from_slice(&block_in);
                Ok(())
            } else {
                switcher.process_block(&block_in, &mut block_out)
            };
            if result.is_err() {
                // Fail-open: pass the microphone through rather than going
                // silent mid-meeting; surface the failure via status.
                status.stage_failed.store(true, Ordering::Relaxed);
                bypass = true;
                block_out.copy_from_slice(&block_in);
            }
            for &sample in &block_out {
                if output.push(sample).is_err() {
                    break;
                }
            }
            status.blocks_processed.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// `AtomicU64` compile-time reference for the shared status struct.
    const _: fn() = || {
        let _ = size_of::<AtomicU64>();
    };
}

#[cfg(target_os = "macos")]
pub use macos::{RtEngine, list_input_devices};

/// Stub for non-macOS hosts so the workspace builds and gates run
/// everywhere. All operations fail with [`RtError::Unsupported`].
#[cfg(not(target_os = "macos"))]
#[derive(Debug)]
pub struct RtEngine {
    _private: (),
}

#[cfg(not(target_os = "macos"))]
impl RtEngine {
    /// Unavailable off macOS.
    ///
    /// # Errors
    ///
    /// Always returns [`RtError::Unsupported`].
    pub fn start(
        _config: &EngineConfig,
        _initial_stage: Box<dyn Stage>,
        _initial_model_id: &str,
    ) -> Result<Self, RtError> {
        Err(RtError::Unsupported)
    }

    /// Unavailable off macOS.
    ///
    /// # Errors
    ///
    /// Always returns [`RtError::Unsupported`].
    pub fn switch_model(&mut self, _model_id: &str, _stage: Box<dyn Stage>) -> Result<(), RtError> {
        Err(RtError::Unsupported)
    }

    /// Unavailable off macOS.
    #[must_use]
    pub const fn current_model(&self) -> &'static str {
        ""
    }

    /// Unavailable off macOS: returns a static zeroed status.
    #[must_use]
    pub fn status(&self) -> &EngineStatus {
        static ZERO: std::sync::OnceLock<EngineStatus> = std::sync::OnceLock::new();
        ZERO.get_or_init(EngineStatus::default)
    }
}

/// Lists input devices (empty off macOS).
///
/// # Errors
///
/// Returns [`RtError`] when device enumeration fails.
#[cfg(not(target_os = "macos"))]
pub const fn list_input_devices() -> Result<Vec<DeviceInfo>, RtError> {
    Ok(Vec::new())
}
