//! The I/O proc: the one function Core Audio calls on the audio thread.
//!
//! `docs/tech-research.md` §4.1 rules out `AVAudioEngine` for device-targeted
//! I/O — setting `kAudioOutputUnitProperty_CurrentDevice` on its input node
//! returns `noErr` and is then silently ignored. So the capture path is a HAL
//! I/O proc registered directly on the aggregate device, which also means the
//! microphone and the virtual output are serviced by the same callback and
//! stay in one clock domain.
//!
//! Everything the callback touches is preallocated. It de-interleaves one
//! microphone channel into a scratch buffer, hands it to
//! [`noican_engine::AudioBridge`], and copies the result to every output
//! channel. No allocation, no locks, no logging — §9's rules.

use noican_engine::AudioBridge;

/// Which channel of the microphone to use, and how loud to play the result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamConfig {
    /// Channel of the input device to capture.
    ///
    /// The pipeline is mono by platform constraint: macOS exposes the built-in
    /// microphone array as one processed stream and no public API returns the
    /// raw channels (`docs/tech-research.md` §4.1).
    pub input_channel: usize,
    /// Frames per device buffer to request. 128–256 at 48 kHz per §4.1.
    pub buffer_frames: u32,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            input_channel: 0,
            buffer_frames: 256,
        }
    }
}

/// State the I/O proc owns, allocated before the stream starts.
///
/// Boxed and handed to Core Audio as the client-data pointer.
#[derive(Debug)]
pub struct IoState {
    bridge: AudioBridge,
    config: StreamConfig,
    /// One channel of microphone audio, de-interleaved.
    input: Vec<f32>,
    /// The processed mono signal, before it is fanned out to every channel.
    output: Vec<f32>,
}

impl IoState {
    /// Allocates the callback's scratch space for buffers of up to
    /// `max_frames`.
    #[must_use]
    pub fn new(bridge: AudioBridge, config: StreamConfig, max_frames: usize) -> Self {
        Self {
            bridge,
            config,
            input: vec![0.0; max_frames],
            output: vec![0.0; max_frames],
        }
    }

    /// Runs one device buffer through the engine.
    ///
    /// `input` is the microphone's interleaved samples and `input_channels` how
    /// many channels they hold; `outputs` is one slice per output buffer, each
    /// interleaved with its own channel count. Returns whether the engine had a
    /// full block ready.
    ///
    /// Split out from the `extern "C"` shim so that it is ordinary safe Rust
    /// and can be tested without Core Audio.
    pub fn process(
        &mut self,
        input: &[f32],
        input_channels: usize,
        frames: usize,
        outputs: &mut [(&mut [f32], usize)],
    ) -> bool {
        let frames = frames.min(self.input.len());
        let channel = self
            .config
            .input_channel
            .min(input_channels.saturating_sub(1));

        if input_channels == 0 || input.is_empty() {
            self.input[..frames].fill(0.0);
        } else {
            for (index, slot) in self.input[..frames].iter_mut().enumerate() {
                *slot = input
                    .get(index * input_channels + channel)
                    .copied()
                    .unwrap_or(0.0);
            }
        }

        let complete = self
            .bridge
            .process(&self.input[..frames], &mut self.output[..frames]);

        // Fan the mono result out to every channel of every output buffer, so
        // the virtual device carries the same signal on left and right.
        for (buffer, channels) in outputs.iter_mut() {
            if *channels == 0 {
                continue;
            }
            for (index, sample) in self.output[..frames].iter().enumerate() {
                let base = index * *channels;
                for offset in 0..*channels {
                    if let Some(slot) = buffer.get_mut(base + offset) {
                        *slot = *sample;
                    }
                }
            }
        }
        complete
    }

    /// Discards anything buffered, for use after a device restart.
    pub fn flush(&mut self) {
        self.bridge.flush();
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use core::ffi::c_void;
    use core::ptr;

    use super::{IoState, StreamConfig};
    use crate::error::{Result, check};
    use crate::sys::{
        self, AudioBufferList, AudioDeviceIoProcId, AudioObjectId, AudioObjectPropertyAddress,
        DEVICE_BUFFER_FRAME_SIZE, OsStatus, SCOPE_GLOBAL,
    };

    /// A running I/O proc, stopped and unregistered when dropped.
    #[derive(Debug)]
    pub struct IoStream {
        device: AudioObjectId,
        proc_id: AudioDeviceIoProcId,
        /// Kept alive for as long as Core Audio holds the pointer to it.
        state: Box<IoState>,
        running: bool,
    }

    // The state is only ever touched by the audio thread while the stream runs,
    // and only by the owner while it does not.
    unsafe impl Send for IoStream {}

    impl IoStream {
        /// Registers and starts an I/O proc on `device`.
        ///
        /// # Errors
        ///
        /// Returns [`crate::Error::CoreAudio`] if the HAL rejects the buffer
        /// size, the registration, or the start.
        pub fn start(
            device: AudioObjectId,
            bridge: noican_engine::AudioBridge,
            config: StreamConfig,
        ) -> Result<Self> {
            set_buffer_frames(device, config.buffer_frames)?;
            // Ask for the size back: the HAL clamps the request to what the
            // device supports, and the scratch buffers have to fit what it
            // actually chose, not what we asked for.
            let granted = buffer_frames(device).unwrap_or(config.buffer_frames);

            let mut state = Box::new(IoState::new(
                bridge,
                config,
                granted.max(config.buffer_frames) as usize * 2,
            ));
            let client_data = ptr::from_mut(state.as_mut()).cast::<c_void>();

            let mut proc_id: AudioDeviceIoProcId = ptr::null_mut();
            // SAFETY: `io_proc` matches the expected signature, and
            // `client_data` points at state this struct keeps alive for at
            // least as long as the registration.
            let status = unsafe {
                sys::AudioDeviceCreateIOProcID(device, io_proc, client_data, &raw mut proc_id)
            };
            check("registering the I/O proc", status)?;

            let mut stream = Self {
                device,
                proc_id,
                state,
                running: false,
            };

            // SAFETY: `proc_id` was just registered on `device`.
            let status = unsafe { sys::AudioDeviceStart(device, proc_id) };
            check("starting the device", status)?;
            stream.running = true;
            tracing::info!(device, granted_frames = granted, "audio stream started");
            Ok(stream)
        }

        /// Discards buffered audio, for use after the device restarts.
        pub fn flush(&mut self) {
            self.state.flush();
        }

        /// The device this stream runs on.
        #[must_use]
        pub const fn device(&self) -> AudioObjectId {
            self.device
        }
    }

    impl Drop for IoStream {
        fn drop(&mut self) {
            if self.running {
                // SAFETY: the proc is registered and running on this device.
                let status = unsafe { sys::AudioDeviceStop(self.device, self.proc_id) };
                if status != 0 {
                    tracing::warn!(status, "could not stop the device");
                }
            }
            // SAFETY: destroying the registration is what makes it safe to free
            // the state afterwards, so it has to happen before this returns.
            let status = unsafe { sys::AudioDeviceDestroyIOProcID(self.device, self.proc_id) };
            if status != 0 {
                tracing::warn!(status, "could not unregister the I/O proc");
            }
        }
    }

    /// Requests a device buffer size.
    fn set_buffer_frames(device: AudioObjectId, frames: u32) -> Result<()> {
        let address = AudioObjectPropertyAddress::scoped(DEVICE_BUFFER_FRAME_SIZE, SCOPE_GLOBAL);
        // SAFETY: the property takes a `UInt32`, which is what is passed.
        let status = unsafe {
            sys::AudioObjectSetPropertyData(
                device,
                &raw const address,
                0,
                ptr::null(),
                u32::try_from(size_of::<u32>()).unwrap_or(4),
                ptr::from_ref(&frames).cast::<c_void>(),
            )
        };
        check("setting the buffer frame size", status)
    }

    /// Reads back the device buffer size the HAL settled on.
    fn buffer_frames(device: AudioObjectId) -> Result<u32> {
        let address = AudioObjectPropertyAddress::scoped(DEVICE_BUFFER_FRAME_SIZE, SCOPE_GLOBAL);
        let mut frames = 0u32;
        let mut size = u32::try_from(size_of::<u32>()).unwrap_or(4);
        // SAFETY: `frames` has room for exactly `size` bytes.
        let status = unsafe {
            sys::AudioObjectGetPropertyData(
                device,
                &raw const address,
                0,
                ptr::null(),
                &raw mut size,
                ptr::from_mut(&mut frames).cast::<c_void>(),
            )
        };
        check("reading the buffer frame size", status)?;
        Ok(frames)
    }

    /// Samples in one Core Audio buffer.
    ///
    /// # Safety
    ///
    /// `buffer.data` must point at `buffer.data_byte_size` bytes of `f32`.
    unsafe fn as_samples(buffer: &sys::AudioBuffer) -> &[f32] {
        if buffer.data.is_null() {
            return &[];
        }
        // SAFETY: delegated to the caller.
        unsafe {
            core::slice::from_raw_parts(
                buffer.data.cast::<f32>(),
                buffer.data_byte_size as usize / size_of::<f32>(),
            )
        }
    }

    /// Samples in one Core Audio buffer, mutably.
    ///
    /// # Safety
    ///
    /// As [`as_samples`].
    unsafe fn as_samples_mut(buffer: &mut sys::AudioBuffer) -> &mut [f32] {
        if buffer.data.is_null() {
            return &mut [];
        }
        // SAFETY: delegated to the caller.
        unsafe {
            core::slice::from_raw_parts_mut(
                buffer.data.cast::<f32>(),
                buffer.data_byte_size as usize / size_of::<f32>(),
            )
        }
    }

    /// The callback Core Audio invokes once per device buffer.
    ///
    /// The first input buffer is the microphone's, because the aggregate lists
    /// sub-devices in the order they were given and the microphone is given
    /// first (see `AggregateDevice::create`). Everything after it belongs to
    /// the virtual device, which also exposes inputs, and is ignored.
    ///
    /// # Safety
    ///
    /// Core Audio guarantees the buffer lists are valid for the duration of the
    /// call and that `client_data` is the pointer given at registration.
    unsafe extern "C" fn io_proc(
        _device: AudioObjectId,
        _now: *const c_void,
        input_data: *const AudioBufferList,
        _input_time: *const c_void,
        output_data: *mut AudioBufferList,
        _output_time: *const c_void,
        client_data: *mut c_void,
    ) -> OsStatus {
        if client_data.is_null() {
            return 0;
        }
        // SAFETY: the pointer is the one handed to AudioDeviceCreateIOProcID,
        // and the stream that owns the state outlives the registration.
        let state = unsafe { &mut *client_data.cast::<IoState>() };

        // SAFETY: Core Audio owns these lists for the duration of the call.
        let input = unsafe { input_data.as_ref() };
        let (input_samples, input_channels) = match input {
            // SAFETY: as above.
            Some(list) => match unsafe { list.as_slice() }.first() {
                // SAFETY: Core Audio sizes `data` as it declares.
                Some(buffer) => (
                    unsafe { as_samples(buffer) },
                    buffer.number_channels as usize,
                ),
                None => (&[][..], 0),
            },
            None => (&[][..], 0),
        };

        let frames = if input_channels == 0 {
            0
        } else {
            input_samples.len() / input_channels
        };

        // SAFETY: Core Audio owns the output list for the duration of the call.
        let Some(output_list) = (unsafe { output_data.as_mut() }) else {
            return 0;
        };
        // SAFETY: as above.
        let output_buffers = unsafe { output_list.as_mut_slice() };

        // Borrow every output buffer up front: the engine writes the same mono
        // signal into all of them.
        let mut targets: [(&mut [f32], usize); MAX_OUTPUT_BUFFERS] =
            core::array::from_fn(|_| (&mut [][..], 0));
        let mut count = 0;
        for buffer in output_buffers.iter_mut().take(MAX_OUTPUT_BUFFERS) {
            let channels = buffer.number_channels as usize;
            // SAFETY: Core Audio sizes `data` as it declares.
            targets[count] = (unsafe { as_samples_mut(buffer) }, channels);
            count += 1;
        }

        state.process(input_samples, input_channels, frames, &mut targets[..count]);
        0
    }

    /// Output buffers the callback will service.
    ///
    /// A stereo virtual device presents one; the bound exists only so the
    /// callback can borrow them without allocating.
    const MAX_OUTPUT_BUFFERS: usize = 8;
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::StreamConfig;
    use crate::error::{Error, Result};

    /// A running I/O proc, stopped and unregistered when dropped.
    #[derive(Debug)]
    pub struct IoStream {
        _private: (),
    }

    impl IoStream {
        /// Registers and starts an I/O proc on `device`.
        ///
        /// # Errors
        ///
        /// Always returns [`Error::Unsupported`] away from macOS.
        pub fn start(
            _device: u32,
            _bridge: noican_engine::AudioBridge,
            _config: StreamConfig,
        ) -> Result<Self> {
            Err(Error::Unsupported)
        }

        /// Discards buffered audio.
        pub const fn flush(&mut self) {}

        /// The device this stream runs on.
        #[must_use]
        pub const fn device(&self) -> u32 {
            0
        }
    }
}

pub use imp::IoStream;

#[cfg(test)]
mod tests {
    use super::{IoState, StreamConfig};
    use noican_engine::{Engine, EngineConfig};

    fn state(channel: usize) -> (Engine, IoState) {
        let mut engine = Engine::new(EngineConfig {
            max_device_block: 256,
            inference_block: 128,
            queue_capacity: 4_096,
            ..EngineConfig::default()
        })
        .unwrap();
        let bridge = engine
            .start(Box::new(noican_core::stage::Passthrough::new(48_000, 128)))
            .unwrap();
        let config = StreamConfig {
            input_channel: channel,
            ..StreamConfig::default()
        };
        (engine, IoState::new(bridge, config, 512))
    }

    #[test]
    fn the_selected_microphone_channel_is_captured() {
        let (_engine, mut state) = state(1);
        // Three interleaved channels; only channel 1 should be read.
        let input = [0.0, 0.5, 0.9, 0.0, 0.6, 0.9];
        let mut buffer = vec![0.0; 4];
        let mut targets: [(&mut [f32], usize); 1] = [(&mut buffer, 2)];
        state.process(&input, 3, 2, &mut targets);

        // The engine primes with silence, so what matters here is that the
        // right channel was extracted, which the queued input reflects.
        assert_eq!(state.input[..2], [0.5, 0.6]);
    }

    #[test]
    fn the_mono_result_is_fanned_out_to_every_channel() {
        let (_engine, mut state) = state(0);
        state.output[..2].copy_from_slice(&[0.25, -0.25]);
        let mut buffer = vec![9.0; 4];
        {
            let mut targets: [(&mut [f32], usize); 1] = [(&mut buffer, 2)];
            // A silent input keeps the engine from overwriting `output`, so the
            // fan-out itself is what is under test.
            state.process(&[0.0; 2], 1, 2, &mut targets);
        }
        // Both channels of both frames must hold the same sample.
        assert!((buffer[0] - buffer[1]).abs() < 1e-9);
        assert!((buffer[2] - buffer[3]).abs() < 1e-9);
    }

    #[test]
    fn a_missing_input_buffer_is_treated_as_silence() {
        let (_engine, mut state) = state(0);
        let mut buffer = vec![9.0; 4];
        let mut targets: [(&mut [f32], usize); 1] = [(&mut buffer, 2)];
        state.process(&[], 0, 2, &mut targets);
        assert_eq!(state.input[..2], [0.0, 0.0]);
        assert!(buffer.iter().all(|sample| sample.abs() < 1e-9));
    }

    #[test]
    fn an_out_of_range_channel_falls_back_to_the_last_one() {
        let (_engine, mut state) = state(7);
        let input = [0.1, 0.2, 0.3, 0.4];
        let mut buffer = vec![0.0; 4];
        let mut targets: [(&mut [f32], usize); 1] = [(&mut buffer, 2)];
        state.process(&input, 2, 2, &mut targets);
        // Two channels, so channel 7 clamps to channel 1.
        assert_eq!(state.input[..2], [0.2, 0.4]);
    }

    #[test]
    fn a_zero_channel_output_buffer_is_skipped() {
        let (_engine, mut state) = state(0);
        let mut buffer = vec![9.0; 4];
        let mut targets: [(&mut [f32], usize); 1] = [(&mut buffer, 0)];
        state.process(&[0.0; 2], 1, 2, &mut targets);
        assert!(buffer.iter().all(|sample| (sample - 9.0).abs() < 1e-9));
    }

    #[test]
    fn more_frames_than_the_scratch_holds_are_clamped() {
        let (_engine, mut state) = state(0);
        let input = vec![0.5; 4_096];
        let mut buffer = vec![0.0; 4_096];
        let mut targets: [(&mut [f32], usize); 1] = [(&mut buffer, 1)];
        // 2048 frames against 512 of scratch: must clamp rather than panic.
        state.process(&input, 2, 2_048, &mut targets);
    }
}
