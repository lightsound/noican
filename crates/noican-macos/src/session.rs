//! Ties the aggregate device, the I/O proc, and the engine into one lifetime.
//!
//! Getting the order right matters: the aggregate has to exist before an I/O
//! proc can be registered on it, the engine has to be running before the
//! callback starts asking it for audio, and on the way down everything unwinds
//! in reverse. Doing that in one place means the UI cannot get it wrong.

use noican_core::Stage;
use noican_engine::{Engine, EngineConfig, Snapshot};

use crate::aggregate::{self, AggregateDevice};
use crate::error::{Error, Result};
use crate::io::{IoStream, StreamConfig};

/// Name the aggregate device is created under.
///
/// Never visible in Sound Settings — the aggregate is private — but it does
/// show up in `Audio MIDI Setup` diagnostics and in Console logs.
const AGGREGATE_NAME: &str = "noican (internal)";

/// How to wire up a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionConfig {
    /// UID of the microphone to capture. Also the clock source.
    pub input_uid: String,
    /// UID of the virtual device to feed.
    pub output_uid: String,
    /// Device buffer size and channel selection.
    pub stream: StreamConfigOwned,
    /// Engine sizing.
    pub engine: EngineConfig,
}

/// [`StreamConfig`] in a form that can live in a `PartialEq` struct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamConfigOwned {
    /// Channel of the input device to capture.
    pub input_channel: usize,
    /// Frames per device buffer to request.
    pub buffer_frames: u32,
}

impl Default for StreamConfigOwned {
    fn default() -> Self {
        let defaults = StreamConfig::default();
        Self {
            input_channel: defaults.input_channel,
            buffer_frames: defaults.buffer_frames,
        }
    }
}

impl From<StreamConfigOwned> for StreamConfig {
    fn from(value: StreamConfigOwned) -> Self {
        Self {
            input_channel: value.input_channel,
            buffer_frames: value.buffer_frames,
        }
    }
}

impl SessionConfig {
    /// A session between two devices with everything else defaulted.
    #[must_use]
    pub fn new(input_uid: impl Into<String>, output_uid: impl Into<String>) -> Self {
        Self {
            input_uid: input_uid.into(),
            output_uid: output_uid.into(),
            stream: StreamConfigOwned::default(),
            engine: EngineConfig::default(),
        }
    }

    fn validate(&self) -> Result<()> {
        if self.input_uid.is_empty() || self.output_uid.is_empty() {
            return Err(Error::UnsuitableDevice(
                "both an input and an output device must be chosen".to_owned(),
            ));
        }
        if self.input_uid == self.output_uid {
            return Err(Error::UnsuitableDevice(
                "the microphone and the virtual output must be different devices".to_owned(),
            ));
        }
        Ok(())
    }
}

/// A running capture-process-output path.
///
/// Dropping it stops the stream, tears down the aggregate device, and stops the
/// inference thread, in that order.
#[derive(Debug)]
pub struct Session {
    // Declaration order is drop order: the stream must stop before the
    // aggregate it runs on disappears, and the engine outlives both because the
    // callback holds a bridge into it.
    stream: Option<IoStream>,
    aggregate: Option<AggregateDevice>,
    engine: Engine,
    config: SessionConfig,
}

impl Session {
    /// Builds the aggregate, starts the engine with `stage`, and opens the
    /// stream.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsuitableDevice`] if the two devices are the same or
    /// unset, [`Error::CoreAudio`] if the HAL refuses any step, or
    /// [`Error::Engine`] if the stage cannot be adapted to the host format.
    pub fn start(config: SessionConfig, stage: Box<dyn Stage>) -> Result<Self> {
        config.validate()?;

        // The aggregate comes first, because the rate it settles on decides how
        // the engine has to be configured. The microphone is the clock source:
        // its rate is the one we cannot influence, so the virtual device is the
        // one that has to follow.
        let aggregate = AggregateDevice::create(
            AGGREGATE_NAME,
            &aggregate::default_uid(),
            &config.input_uid,
            &config.output_uid,
        )?;

        // Ask for the host rate but believe the answer. Telling the engine
        // 48 kHz while the device runs at 44.1 would not fail anywhere — it
        // would just transpose everything by a semitone, which is the kind of
        // bug that gets blamed on the model.
        let sample_rate = aggregate.negotiate_sample_rate(config.engine.sample_rate)?;
        let engine_config = EngineConfig {
            sample_rate,
            ..config.engine
        };

        let mut engine = Engine::new(engine_config)?;
        let bridge = engine.start(stage)?;

        let stream = IoStream::start(aggregate.id(), bridge, config.stream.into())?;

        Ok(Self {
            stream: Some(stream),
            aggregate: Some(aggregate),
            engine,
            config,
        })
    }

    /// The sample rate the engine is actually running at.
    ///
    /// May differ from the requested rate if the device refused it.
    #[must_use]
    pub const fn sample_rate(&self) -> u32 {
        self.engine.config().sample_rate
    }

    /// The configuration this session was started with.
    #[must_use]
    pub const fn config(&self) -> &SessionConfig {
        &self.config
    }

    /// What the engine is doing.
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        self.engine.snapshot()
    }

    /// Switches to a different model without interrupting the stream.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Engine`] if the stage cannot be adapted or a switch is
    /// already in flight.
    pub fn set_stage(&mut self, stage: Box<dyn Stage>) -> Result<()> {
        self.engine.set_stage(stage)?;
        Ok(())
    }

    /// Bypasses or re-enables the active model.
    pub fn set_bypass(&self, bypassed: bool) {
        self.engine.set_bypass(bypassed);
    }

    /// Frees anything the inference thread has handed back.
    pub fn drain_retired(&mut self) {
        self.engine.drain_retired();
    }

    /// Discards buffered audio, for use after the device restarts.
    // `allow` rather than `expect`: clippy only suggests `const` here away from
    // macOS, where the stub's `flush` happens to be const-compatible. On macOS
    // it calls into Core Audio and cannot be.
    #[cfg_attr(
        not(target_os = "macos"),
        allow(
            clippy::missing_const_for_fn,
            reason = "the macOS implementation of IoStream::flush is not const"
        )
    )]
    pub fn flush(&mut self) {
        if let Some(stream) = self.stream.as_mut() {
            stream.flush();
        }
    }

    /// Stops everything in the right order.
    ///
    /// Called by `Drop` as well; calling it early is how a UI stops the stream
    /// without dropping the session.
    pub fn stop(&mut self) {
        // The stream first: the callback must stop before the aggregate it runs
        // on goes away, and before the engine it writes into does.
        self.stream = None;
        self.aggregate = None;
        self.engine.stop();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::{Session, SessionConfig, StreamConfigOwned};
    use crate::error::Error;
    use crate::io::StreamConfig;

    #[test]
    fn a_session_needs_two_different_devices() {
        let same = SessionConfig::new("uid-a", "uid-a");
        assert!(matches!(
            Session::start(
                same,
                Box::new(noican_core::stage::Passthrough::new(48_000, 128))
            ),
            Err(Error::UnsuitableDevice(_))
        ));

        let missing = SessionConfig::new("", "uid-b");
        assert!(matches!(
            Session::start(
                missing,
                Box::new(noican_core::stage::Passthrough::new(48_000, 128))
            ),
            Err(Error::UnsuitableDevice(_))
        ));
    }

    #[test]
    fn stream_defaults_survive_the_round_trip() {
        let owned = StreamConfigOwned::default();
        let converted: StreamConfig = owned.into();
        assert_eq!(converted.input_channel, owned.input_channel);
        assert_eq!(converted.buffer_frames, owned.buffer_frames);
        // 128-256 frames at 48 kHz, per docs/tech-research.md section 4.1.
        assert!((128..=256).contains(&owned.buffer_frames));
    }

    #[test]
    fn config_records_both_devices() {
        let config = SessionConfig::new("uid-mic", "uid-virtual");
        assert_eq!(config.input_uid, "uid-mic");
        assert_eq!(config.output_uid, "uid-virtual");
    }
}
