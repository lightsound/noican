//! See docs/adr/0011-ffi-boundary.md#engine-crate.

use std::{
    collections::BTreeSet,
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicI32, AtomicU64, Ordering},
    },
};

use noican_core::capture::{MAX_NATIVE_RATE, MIN_NATIVE_RATE};
use noican_core::{IntensityControl, Stage, StageError, StagePublisher, SwitchingEngine};
use noican_coreaudio::{
    CoreAudioError, Runtime, StreamLevels, VirtualOutputChannels, WORKER_BLOCK_SAMPLES,
};
use noican_models::{
    ModelSpec,
    fetch::{FetchError, FetchEvent, FetchStep},
};
use thiserror::Error;

pub use noican_coreaudio::monitor::MonitorState;
pub use noican_coreaudio::{check_monitor_device, check_monitor_target};

const SWITCH_FADE_SAMPLES: usize = 240;

/// See docs/adr/0011-ffi-boundary.md#engine-crate.
#[derive(Debug, Error)]
pub enum EngineError {
    /// The id names no catalog entry.
    #[error("unknown model: {0}")]
    UnknownModel(String),
    /// See docs/adr/0003-coreaudio-transport.md#split-transport-native-capture.
    #[error(
        "invalid capture sample rate {0} Hz (the split transport captures \
         {MIN_NATIVE_RATE}–{MAX_NATIVE_RATE} Hz microphones at whole-hertz rates)"
    )]
    InvalidCaptureRate(f64),
    /// The operation needs a running session.
    #[error("engine is not running")]
    NotRunning,
    /// See docs/adr/0011-ffi-boundary.md#ffi-control-locking-and-epoch.
    #[error("start was superseded by a newer control operation")]
    Superseded,
    /// See docs/adr/0011-ffi-boundary.md#ffi-control-locking-and-epoch.
    #[error("engine was stopped while the model was loading")]
    StoppedWhileLoading,
    /// Downloading the weights of the named model failed.
    #[error("downloading {0} weights failed: {1}")]
    Fetch(String, #[source] FetchError),
    /// Building the stage of the named model failed.
    #[error("loading {0} failed: {1}")]
    Load(String, #[source] StageError),
    /// See docs/adr/0011-ffi-boundary.md#ffi-panic-guard.
    #[error("internal error while preparing {0}: {1}")]
    Panicked(String, String),
    /// The switching engine rejected its configuration.
    #[error(transparent)]
    Stage(#[from] StageError),
    /// The audio transport failed.
    #[error(transparent)]
    Transport(#[from] CoreAudioError),
    /// See docs/adr/0011-ffi-boundary.md#engine-crate.
    #[error("the engine control state is unusable after an internal panic")]
    Poisoned,
}

/// See docs/adr/0002-engine-diagnostics.md#engine-diagnostics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Health {
    /// The transport's audio units are running.
    pub running: bool,
    /// See docs/adr/0012-app-state-reducer.md#engine-fault-semantics.
    pub faulted: bool,
    /// Device the preview monitor plays on, while one is up.
    pub monitor_device: Option<u32>,
    /// Frames the transport has rendered since start.
    pub frames_processed: u64,
    /// Output callbacks that delivered no real audio.
    pub output_underruns: u64,
    /// Captured samples dropped before reaching the models.
    pub input_overruns: u64,
    /// Blocks the inference worker has processed.
    pub worker_blocks: u64,
    /// Worker blocks that exceeded the block budget.
    pub worker_blocks_over_budget: u64,
    /// Longest single worker block, in nanoseconds.
    pub worker_block_max_ns: u64,
    /// The worker's real-time promotion succeeded at start.
    pub worker_realtime: bool,
}

/// See docs/adr/0002-engine-diagnostics.md#level-meters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Levels {
    /// Peak level of the captured signal.
    pub input: f32,
    /// Peak level of the processed signal.
    pub output: f32,
    /// See docs/adr/0005-preview-monitor.md#preview-monitor-contract.
    pub monitor: MonitorState,
}

/// See docs/adr/0009-model-catalog-fetch.md#model-preparation-status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelState {
    /// Weights are not on disk; preparing the model downloads them.
    Missing,
    /// A download is in progress.
    Fetching,
    /// Weights are on disk, or the model needs none.
    Ready,
    /// The last preparation found a digest mismatch it could not repair.
    Corrupt,
}

/// See docs/adr/0009-model-catalog-fetch.md#model-preparation-status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelStatus {
    /// Where the model's weights stand.
    pub state: ModelState,
    /// Bytes of the current file received so far, while fetching.
    pub received_bytes: u64,
    /// Announced size of the current file while fetching; 0 when unknown.
    pub total_bytes: u64,
}

#[derive(Default)]
struct Preparation {
    turn: Mutex<()>,
    log: Mutex<PreparationLog>,
    received: AtomicU64,
    total: AtomicU64,
}

#[derive(Default)]
struct PreparationLog {
    fetching: Option<String>,
    repairing: bool,
    corrupt: BTreeSet<String>,
}

impl Preparation {
    fn log(&self) -> MutexGuard<'_, PreparationLog> {
        self.log.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn run(
        &self,
        model: &str,
        fetch: impl FnOnce(&mut dyn FnMut(FetchEvent<'_>)) -> Result<(), FetchError>,
    ) -> Result<(), FetchError> {
        let turn = self.turn.lock().unwrap_or_else(PoisonError::into_inner);
        let outcome = fetch(&mut |event| self.observe(model, event));
        self.conclude(model, &outcome);
        drop(turn);
        outcome
    }

    fn observe(&self, model: &str, event: FetchEvent<'_>) {
        match event.step {
            FetchStep::Started => {
                self.received.store(0, Ordering::Relaxed);
                self.total.store(0, Ordering::Relaxed);
                self.log().fetching = Some(model.to_owned());
            }
            FetchStep::Received(bytes, total) => {
                self.received.store(bytes, Ordering::Relaxed);
                self.total.store(total.unwrap_or(0), Ordering::Relaxed);
            }
            FetchStep::Repairing => self.log().repairing = true,
            FetchStep::Finished(_) => self.log().repairing = false,
            FetchStep::Present => {}
        }
    }

    fn conclude(&self, model: &str, outcome: &Result<(), FetchError>) {
        let mut log = self.log();
        if log.fetching.as_deref() == Some(model) {
            log.fetching = None;
        }
        let unrepaired = std::mem::take(&mut log.repairing);
        match outcome {
            Ok(()) => {
                log.corrupt.remove(model);
            }
            Err(FetchError::Checksum { .. }) => {
                log.corrupt.insert(model.to_owned());
            }
            Err(_) if unrepaired => {
                log.corrupt.insert(model.to_owned());
            }
            Err(_) => {}
        }
    }

    fn status(&self, model: &str, fetched: bool) -> ModelStatus {
        let log = self.log();
        let state = if log.fetching.as_deref() == Some(model) {
            ModelState::Fetching
        } else if log.corrupt.contains(model) {
            ModelState::Corrupt
        } else if fetched {
            ModelState::Ready
        } else {
            ModelState::Missing
        };
        drop(log);
        let fetching = state == ModelState::Fetching;
        ModelStatus {
            state,
            received_bytes: if fetching {
                self.received.load(Ordering::Relaxed)
            } else {
                0
            },
            total_bytes: if fetching {
                self.total.load(Ordering::Relaxed)
            } else {
                0
            },
        }
    }
}

struct ControlState {
    runtime: Option<Runtime>,
    publisher: Option<StagePublisher>,
    epoch: u64,
}

/// See docs/adr/0011-ffi-boundary.md#engine-crate.
pub struct Engine {
    models_dir: PathBuf,
    state: Mutex<ControlState>,
    levels: Arc<StreamLevels>,
    monitor_state: Arc<AtomicI32>,
    intensity: IntensityControl,
    preparation: Preparation,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("models_dir", &self.models_dir)
            .finish_non_exhaustive()
    }
}

/// The models directory used when the caller names none.
#[must_use]
pub fn default_models_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("NOICAN_MODELS_DIR") {
        return PathBuf::from(dir);
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::home_dir() {
        return home.join("Library/Application Support/noican/models");
    }
    PathBuf::from("models")
}

impl Engine {
    /// Creates a stopped engine that keeps model weights under `models_dir`.
    #[must_use]
    pub fn new(models_dir: PathBuf) -> Self {
        Self {
            models_dir,
            state: Mutex::new(ControlState {
                runtime: None,
                publisher: None,
                epoch: 0,
            }),
            levels: Arc::new(StreamLevels::new()),
            monitor_state: Arc::new(AtomicI32::new(MonitorState::Off.as_raw())),
            intensity: IntensityControl::default(),
            preparation: Preparation::default(),
        }
    }

    /// See docs/adr/0009-model-catalog-fetch.md#model-preparation-status.
    #[must_use]
    pub fn model_status(&self, model: &str) -> ModelStatus {
        let fetched = require_known_model(model).is_ok()
            && ModelSpec::find(model)
                .is_none_or(|spec| noican_models::fetch::is_fetched(&self.models_dir, spec));
        self.preparation.status(model, fetched)
    }

    /// See docs/adr/0009-model-catalog-fetch.md#model-preparation-status.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] for an unknown id or a failed download.
    pub fn prepare_model(&self, model: &str) -> Result<(), EngineError> {
        require_known_model(model)?;
        let Some(spec) = ModelSpec::find(model) else {
            return Ok(());
        };
        self.preparation
            .run(model, |report| {
                noican_models::fetch::fetch_model(&self.models_dir, spec, report)
            })
            .map_err(|error| EngineError::Fetch(model.to_owned(), error))
    }

    /// See docs/adr/0004-aggregate-output-routing.md#aggregate-channel-routing.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the model is unknown or cannot be
    /// prepared, the channel range is invalid, the transport refuses to
    /// start, or a newer control operation superseded this one.
    pub fn start_aggregate(
        &self,
        aggregate_device: u32,
        virtual_output_first_channel: u32,
        virtual_output_channel_count: u32,
        model: &str,
    ) -> Result<(), EngineError> {
        require_known_model(model)?;
        let virtual_output =
            VirtualOutputChannels::new(virtual_output_first_channel, virtual_output_channel_count)?;
        self.start_with(model, |engine, levels, monitor_state| {
            Runtime::start(
                aggregate_device,
                virtual_output,
                engine,
                levels,
                monitor_state,
            )
        })
    }

    /// See docs/adr/0003-coreaudio-transport.md#split-transport-native-capture.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the model is unknown or cannot be
    /// prepared, the capture rate is invalid, the transport refuses to
    /// start, or a newer control operation superseded this one.
    pub fn start_native(
        &self,
        input_device: u32,
        output_device: u32,
        capture_sample_rate: f64,
        model: &str,
    ) -> Result<(), EngineError> {
        require_known_model(model)?;
        let capture_rate = validate_capture_rate(capture_sample_rate)?;
        self.start_with(model, |engine, levels, monitor_state| {
            Runtime::start_native(
                input_device,
                output_device,
                capture_rate,
                engine,
                levels,
                monitor_state,
            )
        })
    }

    fn prepare_stage(&self, model: &str) -> Result<Box<dyn Stage>, EngineError> {
        self.prepare_model(model)?;
        noican_models::create_stage(model, &self.models_dir)
            .map_err(|error| EngineError::Load(model.to_owned(), error))
    }

    fn start_with(
        &self,
        model: &str,
        start_runtime: impl FnOnce(
            SwitchingEngine,
            Arc<StreamLevels>,
            Arc<AtomicI32>,
        ) -> Result<Runtime, CoreAudioError>,
    ) -> Result<(), EngineError> {
        let (epoch, old_runtime) = {
            let mut control = self.control()?;
            control.epoch += 1;
            control.publisher = None;
            (control.epoch, control.runtime.take())
        };
        if let Some(mut runtime) = old_runtime {
            runtime.stop();
        }

        let levels = Arc::clone(&self.levels);
        let monitor_state = Arc::clone(&self.monitor_state);
        let intensity = self.intensity.clone();
        let (publisher, mut runtime) = guard_panics(model, || {
            let stage = self.prepare_stage(model)?;
            let (publisher, engine) =
                SwitchingEngine::new(stage, SWITCH_FADE_SAMPLES, WORKER_BLOCK_SAMPLES, intensity)?;
            let runtime = start_runtime(engine, levels, monitor_state)?;
            Ok((publisher, runtime))
        })?;

        let Ok(mut control) = self.control() else {
            runtime.stop();
            return Err(EngineError::Poisoned);
        };
        if control.epoch != epoch {
            drop(control);
            runtime.stop();
            return Err(EngineError::Superseded);
        }
        control.runtime = Some(runtime);
        control.publisher = Some(publisher);
        Ok(())
    }

    /// See docs/adr/0011-ffi-boundary.md#ffi-handle-lifecycle.
    pub fn stop(&self) {
        let old_runtime = {
            let Ok(mut control) = self.control() else {
                return;
            };
            control.epoch += 1;
            control.publisher = None;
            control.runtime.take()
        };
        if let Some(mut runtime) = old_runtime {
            runtime.stop();
        }
    }

    /// See docs/adr/0011-ffi-boundary.md#ffi-control-locking-and-epoch.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the model is unknown or cannot be
    /// prepared, no session is running, or the session was stopped while
    /// the model loaded.
    pub fn set_model(&self, model: &str) -> Result<(), EngineError> {
        require_known_model(model)?;
        let (publisher, epoch) = {
            let control = self.control()?;
            let publisher = control.publisher.clone().ok_or(EngineError::NotRunning)?;
            (publisher, control.epoch)
        };
        let stage = guard_panics(model, || self.prepare_stage(model))?;
        let control = self.control()?;
        if control.epoch != epoch {
            return Err(EngineError::StoppedWhileLoading);
        }
        let _superseded = publisher.publish(stage);
        drop(control);
        Ok(())
    }

    /// See docs/adr/0005-preview-monitor.md#preview-monitor-contract.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::NotRunning`] when enabling without a session
    /// and [`EngineError::Transport`] when the monitor refuses to start.
    pub fn set_monitor(&self, enabled: bool) -> Result<(), EngineError> {
        let mut control = self.control()?;
        let Some(runtime) = control.runtime.as_mut() else {
            return if enabled {
                Err(EngineError::NotRunning)
            } else {
                Ok(())
            };
        };
        runtime.set_monitor(enabled)?;
        drop(control);
        Ok(())
    }

    /// See docs/adr/0002-engine-diagnostics.md#engine-diagnostics.
    #[must_use]
    pub fn health(&self) -> Health {
        let Ok(control) = self.control() else {
            return Health {
                faulted: true,
                ..Health::default()
            };
        };
        control
            .runtime
            .as_ref()
            .map_or_else(Health::default, |runtime| Health {
                running: runtime.is_running(),
                faulted: runtime.is_faulted(),
                monitor_device: runtime.monitor_device(),
                frames_processed: runtime.frames_processed(),
                output_underruns: runtime.output_underruns(),
                input_overruns: runtime.input_overruns(),
                worker_blocks: runtime.worker_blocks(),
                worker_blocks_over_budget: runtime.worker_blocks_over_budget(),
                worker_block_max_ns: runtime.worker_block_max_ns(),
                worker_realtime: runtime.worker_realtime(),
            })
    }

    /// See docs/adr/0002-engine-diagnostics.md#level-meters.
    #[must_use]
    pub fn levels(&self) -> Levels {
        Levels {
            input: self.levels.input(),
            output: self.levels.output(),
            monitor: MonitorState::from_raw(self.monitor_state.load(Ordering::Acquire)),
        }
    }

    /// See docs/adr/0004-aggregate-output-routing.md#transport-routing-diagnostics.
    #[must_use]
    pub fn routing_description(&self) -> String {
        self.control()
            .ok()
            .and_then(|control| {
                control
                    .runtime
                    .as_ref()
                    .map(|runtime| runtime.routing_description().to_owned())
            })
            .unwrap_or_default()
    }

    /// See docs/adr/0002-engine-diagnostics.md#engine-diagnostics.
    pub fn reset_debug_stats(&self) {
        if let Ok(control) = self.control()
            && let Some(runtime) = control.runtime.as_ref()
        {
            runtime.reset_debug_stats();
        }
    }

    /// See docs/adr/0007-dsp-stages.md#intensity-dry-wet-mix.
    pub fn set_intensity(&self, intensity: f32) {
        self.intensity.set(intensity);
    }

    /// The current dry/wet intensity, 0.0 to 1.0.
    #[must_use]
    pub fn intensity(&self) -> f32 {
        self.intensity.get()
    }

    fn control(&self) -> Result<MutexGuard<'_, ControlState>, EngineError> {
        self.state.lock().map_err(|_| EngineError::Poisoned)
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        if let Ok(control) = self.state.get_mut()
            && let Some(mut runtime) = control.runtime.take()
        {
            runtime.stop();
        }
    }
}

fn require_known_model(id: &str) -> Result<(), EngineError> {
    if noican_models::catalog().any(|entry| entry.id == id) {
        Ok(())
    } else {
        Err(EngineError::UnknownModel(id.to_owned()))
    }
}

fn validate_capture_rate(rate: f64) -> Result<u32, EngineError> {
    let rounded = rate.round();
    let min = f64::from(MIN_NATIVE_RATE);
    let max = f64::from(MAX_NATIVE_RATE);
    if !rate.is_finite() || (rate - rounded).abs() > 1e-6 || !(min..=max).contains(&rounded) {
        return Err(EngineError::InvalidCaptureRate(rate));
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "bounded to MIN_NATIVE_RATE..=MAX_NATIVE_RATE by the range check above"
    )]
    let hertz = rounded as u32;
    Ok(hertz)
}

fn guard_panics<T>(
    model: &str,
    work: impl FnOnce() -> Result<T, EngineError>,
) -> Result<T, EngineError> {
    catch_unwind(AssertUnwindSafe(work)).unwrap_or_else(|panic| {
        let message = panic
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic payload".to_owned());
        Err(EngineError::Panicked(model.to_owned(), message))
    })
}

#[cfg(test)]
mod tests {
    use noican_models::PASSTHROUGH_ID;

    use super::*;

    fn engine() -> Engine {
        Engine::new(std::env::temp_dir().join("noican-engine-test-no-weights"))
    }

    #[test]
    fn unknown_model_ids_are_rejected() {
        assert!(matches!(
            require_known_model("deepfilternet3"),
            Err(EngineError::UnknownModel(id)) if id == "deepfilternet3"
        ));
        assert!(require_known_model("dfn3").is_ok());
        assert!(require_known_model(PASSTHROUGH_ID).is_ok());
    }

    #[test]
    fn model_status_follows_the_files_on_disk() {
        let models_dir = std::env::temp_dir().join(format!(
            "noican-engine-status-test-{}-{}",
            std::process::id(),
            line!()
        ));
        let engine = Engine::new(models_dir.clone());
        let spec = ModelSpec::find("dfn3-ll").expect("dfn3-ll is in the registry");
        let status = |model| engine.model_status(model).state;

        assert_eq!(status(PASSTHROUGH_ID), ModelState::Ready);
        assert_eq!(status("dfn3-ll"), ModelState::Missing);
        assert_eq!(status("dfn3"), ModelState::Ready);
        assert_eq!(status("nope"), ModelState::Missing);
        assert!(matches!(
            engine.prepare_model("nope"),
            Err(EngineError::UnknownModel(id)) if id == "nope"
        ));
        assert!(engine.prepare_model(PASSTHROUGH_ID).is_ok());

        let dir = noican_models::fetch::model_dir(&models_dir, spec);
        std::fs::create_dir_all(&dir).expect("temp dir");
        for file in spec.files {
            std::fs::write(dir.join(file.name), b"placeholder").expect("write");
        }
        assert_eq!(status("dfn3-ll"), ModelState::Ready);
        std::fs::remove_dir_all(&models_dir).expect("cleanup");
    }

    #[test]
    fn a_download_is_reported_with_its_progress_until_it_concludes() {
        let preparation = Preparation::default();
        let event = |step| FetchEvent {
            model: "dependency",
            file: "weights.bin",
            step,
        };

        preparation.observe("model", event(FetchStep::Present));
        assert_eq!(preparation.status("model", true).state, ModelState::Ready);

        preparation.observe("model", event(FetchStep::Started));
        preparation.observe("model", event(FetchStep::Received(40, Some(100))));
        assert_eq!(
            preparation.status("model", false),
            ModelStatus {
                state: ModelState::Fetching,
                received_bytes: 40,
                total_bytes: 100,
            }
        );
        assert_eq!(
            preparation.status("other", false).state,
            ModelState::Missing
        );

        preparation.conclude("model", &Ok(()));
        assert_eq!(
            preparation.status("model", true),
            ModelStatus {
                state: ModelState::Ready,
                received_bytes: 0,
                total_bytes: 0,
            }
        );
    }

    #[test]
    fn an_unrepaired_digest_mismatch_marks_the_model_corrupt_until_a_fetch_succeeds() {
        let preparation = Preparation::default();
        let mismatch = Err(FetchError::Checksum {
            file: "weights.bin".to_owned(),
            expected: "aa".to_owned(),
            got: "bb".to_owned(),
        });
        let missing = Err(FetchError::Missing {
            file: "weights.bin".to_owned(),
        });

        preparation.conclude("model", &missing);
        assert_eq!(
            preparation.status("model", false).state,
            ModelState::Missing
        );

        preparation.conclude("model", &mismatch);
        assert_eq!(preparation.status("model", true).state, ModelState::Corrupt);
        assert_eq!(preparation.status("other", true).state, ModelState::Ready);

        preparation.conclude("model", &Ok(()));
        assert_eq!(preparation.status("model", true).state, ModelState::Ready);
    }

    #[test]
    fn a_failed_repair_leaves_the_model_corrupt_until_a_repair_finishes() {
        let preparation = Preparation::default();
        let event = |step| FetchEvent {
            model: "model",
            file: "weights.bin",
            step,
        };
        let offline = || {
            Err(FetchError::Io {
                path: PathBuf::from("weights.bin.part"),
                source: std::io::Error::other("offline"),
            })
        };

        let outcome = preparation.run("model", |report| {
            report(event(FetchStep::Repairing));
            report(event(FetchStep::Started));
            assert_eq!(
                preparation.status("model", true).state,
                ModelState::Fetching
            );
            offline()
        });
        assert!(outcome.is_err());
        assert_eq!(preparation.status("model", true).state, ModelState::Corrupt);

        let outcome = preparation.run("model", |report| {
            report(event(FetchStep::Repairing));
            report(event(FetchStep::Started));
            report(event(FetchStep::Finished(8)));
            report(event(FetchStep::Started));
            offline()
        });
        assert!(outcome.is_err());
        assert_eq!(preparation.status("model", true).state, ModelState::Corrupt);

        let outcome = preparation.run("model", |report| {
            report(event(FetchStep::Repairing));
            report(event(FetchStep::Started));
            report(event(FetchStep::Finished(8)));
            Ok(())
        });
        assert!(outcome.is_ok());
        assert_eq!(preparation.status("model", true).state, ModelState::Ready);

        let outcome = preparation.run("model", |report| {
            report(event(FetchStep::Repairing));
            report(event(FetchStep::Started));
            report(event(FetchStep::Finished(8)));
            report(event(FetchStep::Started));
            offline()
        });
        assert!(outcome.is_err());
        assert_eq!(preparation.status("model", true).state, ModelState::Ready);
    }

    #[test]
    fn overlapping_preparations_take_turns_and_stay_readable() {
        let preparation = Preparation::default();
        let active = AtomicU64::new(0);
        let overlaps = AtomicU64::new(0);
        std::thread::scope(|scope| {
            for model in ["hush", "hush-48k", "dfn3-ll", "dpdfnet2"] {
                let (preparation, active, overlaps) = (&preparation, &active, &overlaps);
                scope.spawn(move || {
                    let outcome = preparation.run(model, |report| {
                        if active.fetch_add(1, Ordering::SeqCst) > 0 {
                            overlaps.fetch_add(1, Ordering::SeqCst);
                        }
                        report(FetchEvent {
                            model,
                            file: "weights.bin",
                            step: FetchStep::Started,
                        });
                        for bytes in 1..=50 {
                            report(FetchEvent {
                                model,
                                file: "weights.bin",
                                step: FetchStep::Received(bytes, Some(50)),
                            });
                            std::thread::yield_now();
                            let status = preparation.status(model, false);
                            assert_eq!(status.state, ModelState::Fetching);
                            assert_eq!(status.received_bytes, bytes);
                        }
                        active.fetch_sub(1, Ordering::SeqCst);
                        Ok(())
                    });
                    assert!(outcome.is_ok());
                });
            }
        });
        assert_eq!(overlaps.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_stopped_engine_reports_default_health_and_silent_levels() {
        let engine = engine();
        assert_eq!(engine.health(), Health::default());
        let levels = engine.levels();
        assert!(levels.input.abs() < f32::EPSILON);
        assert!(levels.output.abs() < f32::EPSILON);
        assert_eq!(levels.monitor, MonitorState::Off);
        assert_eq!(engine.routing_description(), "");
        engine.reset_debug_stats();
        engine.stop();
        assert_eq!(engine.health(), Health::default());
    }

    #[test]
    fn the_monitor_state_follows_the_shared_atomic_and_rejects_unknown_values() {
        let engine = engine();
        for state in [
            MonitorState::Playing,
            MonitorState::Tripped,
            MonitorState::Off,
        ] {
            engine
                .monitor_state
                .store(state.as_raw(), Ordering::Release);
            assert_eq!(engine.levels().monitor, state);
        }
        engine.monitor_state.store(99, Ordering::Release);
        assert_eq!(engine.levels().monitor, MonitorState::Off);
    }

    #[test]
    fn the_monitor_can_be_disabled_but_not_enabled_while_stopped() {
        let engine = engine();
        assert!(engine.set_monitor(false).is_ok());
        let error = engine.set_monitor(true).expect_err("no session");
        assert!(matches!(error, EngineError::NotRunning));
        assert_eq!(error.to_string(), "engine is not running");
        assert_eq!(engine.levels().monitor, MonitorState::Off);
    }

    #[test]
    fn switching_models_needs_a_running_session() {
        let error = engine().set_model(PASSTHROUGH_ID).expect_err("stopped");
        assert!(matches!(error, EngineError::NotRunning));
    }

    #[test]
    fn intensity_defaults_to_full_clamps_and_survives_a_stop() {
        let engine = engine();
        assert!((engine.intensity() - 1.0).abs() < f32::EPSILON);
        engine.set_intensity(0.3);
        assert!((engine.intensity() - 0.3).abs() < f32::EPSILON);
        engine.set_intensity(5.0);
        assert!((engine.intensity() - 1.0).abs() < f32::EPSILON);
        engine.set_intensity(-1.0);
        assert!(engine.intensity().abs() < f32::EPSILON);
        engine.set_intensity(f32::NAN);
        assert!(engine.intensity().abs() < f32::EPSILON);
        engine.set_intensity(0.7);
        engine.stop();
        assert!((engine.intensity() - 0.7).abs() < f32::EPSILON);
    }

    #[test]
    fn capture_rate_validation_accepts_the_resampler_range() {
        for rate in [
            8_000.0, 11_025.0, 12_000.0, 16_000.0, 22_050.0, 24_000.0, 32_000.0, 44_100.0,
            48_000.0, 88_200.0, 96_000.0, 192_000.0,
        ] {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "test rates are small integers"
            )]
            let expected = rate as u32;
            assert_eq!(validate_capture_rate(rate).ok(), Some(expected));
        }
        for rate in [
            0.0,
            -16_000.0,
            7_999.0,
            192_001.0,
            16_000.7,
            44_100.01,
            f64::NAN,
            f64::INFINITY,
        ] {
            let error = validate_capture_rate(rate).expect_err("must be rejected");
            assert!(
                error.to_string().contains("Hz"),
                "unhelpful message for {rate}: {error}"
            );
        }
    }

    #[test]
    fn start_native_rejects_bad_rates_before_any_slow_work() {
        let engine = engine();
        let error = engine
            .start_native(0, 0, 4_000.0, PASSTHROUGH_ID)
            .expect_err("4 kHz is out of range");
        let message = error.to_string();
        assert!(
            message.contains("4000") && message.contains("invalid capture sample rate"),
            "unhelpful message: {message}"
        );
        for rate in [16_000.0, 44_100.0] {
            let error = engine
                .start_native(0, 0, rate, PASSTHROUGH_ID)
                .expect_err("no device 0");
            assert!(
                !matches!(error, EngineError::InvalidCaptureRate(_)),
                "a valid rate must not be blamed: {error}"
            );
            #[cfg(not(target_os = "macos"))]
            assert!(
                error.to_string().contains("only on macOS"),
                "portable builds refuse the transport: {error}"
            );
        }
    }

    #[test]
    fn start_aggregate_rejects_an_empty_virtual_output_range_before_any_slow_work() {
        let engine = engine();
        let error = engine
            .start_aggregate(0, 2, 0, PASSTHROUGH_ID)
            .expect_err("empty range");
        assert!(
            error
                .to_string()
                .starts_with("virtual output routing failed"),
            "unhelpful message: {error}"
        );
        for first in [0, 2] {
            let error = engine
                .start_aggregate(0, first, 2, PASSTHROUGH_ID)
                .expect_err("no device 0");
            assert!(
                !error
                    .to_string()
                    .starts_with("virtual output routing failed: the virtual output reports"),
                "a valid layout must not be blamed: {error}"
            );
            #[cfg(not(target_os = "macos"))]
            assert!(
                error.to_string().contains("only on macOS"),
                "portable builds refuse the transport: {error}"
            );
        }
    }

    #[test]
    fn panics_become_errors_not_aborts() {
        let result: Result<(), EngineError> = guard_panics("test-model", || panic!("boom"));
        let error = result.expect_err("panic must map to an error").to_string();
        assert!(error.contains("test-model"), "unhelpful message: {error}");
        assert!(error.contains("boom"), "payload lost: {error}");
    }
}
