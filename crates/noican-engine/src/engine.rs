//! The engine: owns the inference thread and mediates between the three
//! threads that touch audio.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use noican_core::{Stage, StageRunner};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::bridge::AudioBridge;
use crate::error::{Error, Result};
use crate::status::{Snapshot, Status};
use crate::switch::SwitchRamp;

/// How the engine is sized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineConfig {
    /// Host sample rate, in hertz.
    pub sample_rate: u32,
    /// Largest device buffer the audio callback will present.
    pub max_device_block: usize,
    /// Samples the inference thread processes per iteration.
    pub inference_block: usize,
    /// Samples each half of a switch ramp lasts.
    pub fade_samples: usize,
    /// Capacity of each hand-off queue, in samples.
    ///
    /// Has to absorb the difference between the device's period and the
    /// inference thread's polling period without ever filling, because a full
    /// input queue means dropped microphone audio.
    pub queue_capacity: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            sample_rate: noican_core::HOST_SAMPLE_RATE,
            max_device_block: 1_024,
            inference_block: 128,
            // 30 ms at 48 kHz: long enough that no step in the envelope is
            // audible, short enough that a deliberate model change feels
            // immediate.
            fade_samples: 1_440,
            // A quarter of a second, far more than the threads can drift apart
            // by, and under 50 kB per direction.
            queue_capacity: 12_000,
        }
    }
}

impl EngineConfig {
    /// Largest block a runner built by this engine has to accept.
    const fn runner_block(&self) -> usize {
        if self.inference_block > self.max_device_block {
            self.inference_block
        } else {
            self.max_device_block
        }
    }

    fn validate(&self) -> Result<()> {
        if self.sample_rate == 0 {
            return Err(Error::InvalidConfiguration(
                "sample_rate must be non-zero".to_owned(),
            ));
        }
        if self.inference_block == 0 || self.max_device_block == 0 {
            return Err(Error::InvalidConfiguration(
                "block sizes must be non-zero".to_owned(),
            ));
        }
        if self.queue_capacity < self.max_device_block * 2 {
            return Err(Error::InvalidConfiguration(format!(
                "queue_capacity ({}) must be at least twice max_device_block ({})",
                self.queue_capacity, self.max_device_block
            )));
        }
        Ok(())
    }
}

/// Depth of the stage hand-off queues.
const STAGE_QUEUE_DEPTH: usize = 4;

/// The engine.
///
/// Construct it, [`Engine::start`] it to obtain the [`AudioBridge`] the audio
/// callback needs, then change models with [`Engine::set_stage`] while it runs.
///
/// Runners are built here, on the control thread, and retired runners come back
/// here to be dropped. The inference thread only ever moves them, so neither
/// allocation nor deallocation happens anywhere near the audio path.
pub struct Engine {
    config: EngineConfig,
    status: Arc<Status>,
    shutdown: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    pending_runners: Option<Producer<StageRunner>>,
    retired_runners: Option<Consumer<StageRunner>>,
}

// `rtrb`'s queue ends are not `Debug`.
impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("config", &self.config)
            .field("running", &self.worker.is_some())
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

impl Engine {
    /// Creates a stopped engine.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidConfiguration`] if `config` is inconsistent.
    pub fn new(config: EngineConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            config,
            status: Arc::new(Status::new()),
            shutdown: Arc::new(AtomicBool::new(false)),
            worker: None,
            pending_runners: None,
            retired_runners: None,
        })
    }

    /// The configuration this engine was built with.
    #[must_use]
    pub const fn config(&self) -> EngineConfig {
        self.config
    }

    /// A consistent-enough view of what the engine is doing.
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        self.status.snapshot()
    }

    /// Whether the inference thread is running.
    #[must_use]
    pub const fn is_running(&self) -> bool {
        self.worker.is_some()
    }

    /// Starts the inference thread with `stage` active, and returns the bridge
    /// the audio callback should use.
    ///
    /// # Errors
    ///
    /// Returns [`Error::AlreadyRunning`] if the engine is already started, or a
    /// core error if `stage` cannot be adapted to the host format.
    pub fn start(&mut self, stage: Box<dyn Stage>) -> Result<AudioBridge> {
        if self.worker.is_some() {
            return Err(Error::AlreadyRunning);
        }

        let runner = StageRunner::new(stage, self.config.sample_rate, self.config.runner_block())?;
        self.status.set_latency_ms(runner.latency_ms());

        let (to_inference, inference_input) = RingBuffer::new(self.config.queue_capacity);
        let (inference_output, from_inference) = RingBuffer::new(self.config.queue_capacity);
        let (pending_producer, pending_consumer) = RingBuffer::new(STAGE_QUEUE_DEPTH);
        let (retired_producer, retired_consumer) = RingBuffer::new(STAGE_QUEUE_DEPTH);

        self.shutdown.store(false, Ordering::Relaxed);
        let worker = Worker {
            runner,
            incoming: None,
            input: inference_input,
            output: inference_output,
            pending: pending_consumer,
            retired: retired_producer,
            ramp: SwitchRamp::new(self.config.fade_samples),
            status: Arc::clone(&self.status),
            shutdown: Arc::clone(&self.shutdown),
            config: self.config,
            input_block: vec![0.0; self.config.inference_block],
            output_block: vec![0.0; self.config.inference_block],
            discard_block: vec![0.0; self.config.inference_block],
        };

        self.pending_runners = Some(pending_producer);
        self.retired_runners = Some(retired_consumer);
        self.status.set_running(true);
        self.worker = Some(
            std::thread::Builder::new()
                .name("noican-inference".to_owned())
                .spawn(move || worker.run())
                .map_err(|error| {
                    Error::InvalidConfiguration(format!("cannot spawn inference thread: {error}"))
                })?,
        );

        Ok(AudioBridge::new(
            to_inference,
            from_inference,
            Arc::clone(&self.status),
        ))
    }

    /// Stops the inference thread and returns once it has exited.
    pub fn stop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            drop(worker.join());
        }
        self.status.set_running(false);
        self.status.set_switching(false);
        self.pending_runners = None;
        self.drain_retired();
        self.retired_runners = None;
    }

    /// Switches to `stage`, ramping so the change does not click.
    ///
    /// The runner is built here and handed over ready to use; the inference
    /// thread performs the ramp and hands the retired one back. Returns as soon
    /// as the hand-off is queued.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotRunning`] if the engine is stopped,
    /// [`Error::SwitchInFlight`] if switches are being requested faster than
    /// the inference thread can consume them, or a core error if `stage` cannot
    /// be adapted to the host format.
    pub fn set_stage(&mut self, stage: Box<dyn Stage>) -> Result<()> {
        self.drain_retired();
        if self.pending_runners.is_none() {
            return Err(Error::NotRunning);
        }
        let runner = StageRunner::new(stage, self.config.sample_rate, self.config.runner_block())?;
        let pending = self.pending_runners.as_mut().ok_or(Error::NotRunning)?;
        pending.push(runner).map_err(|_| Error::SwitchInFlight)
    }

    /// Bypasses or re-enables the active model.
    ///
    /// The model keeps running while bypassed so its recurrent state stays
    /// current; only its output is discarded. Re-enabling therefore does not
    /// restart from silence.
    pub fn set_bypass(&self, bypassed: bool) {
        self.status.set_bypassed(bypassed);
    }

    /// Frees any runner the inference thread has handed back.
    ///
    /// Called automatically by [`Self::set_stage`] and [`Self::stop`]; exposed
    /// so a UI can also call it from its refresh timer.
    pub fn drain_retired(&mut self) {
        if let Some(retired) = self.retired_runners.as_mut() {
            while let Ok(runner) = retired.pop() {
                drop(runner);
            }
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The inference thread's state.
struct Worker {
    runner: StageRunner,
    /// A runner taken from the queue, waiting for the fade-out to finish.
    incoming: Option<StageRunner>,
    input: Consumer<f32>,
    output: Producer<f32>,
    pending: Consumer<StageRunner>,
    retired: Producer<StageRunner>,
    ramp: SwitchRamp,
    status: Arc<Status>,
    shutdown: Arc<AtomicBool>,
    config: EngineConfig,
    input_block: Vec<f32>,
    output_block: Vec<f32>,
    /// Receives the model's output while bypassed, so the model still runs.
    discard_block: Vec<f32>,
}

impl Worker {
    fn run(mut self) {
        // Poll rather than wait on a condition variable: signalling one from
        // the audio callback would mean taking a lock there, which
        // `docs/tech-research.md` §9 forbids. A quarter of the block period
        // keeps the added jitter well inside the latency budget.
        let block = u32::try_from(self.config.inference_block).unwrap_or(u32::MAX);
        let period =
            Duration::from_secs_f64(f64::from(block) / f64::from(self.config.sample_rate) / 4.0);

        while !self.shutdown.load(Ordering::Relaxed) {
            if self.step() {
                continue;
            }
            self.status.add_idle_poll();
            std::thread::sleep(period);
        }

        self.retire_everything();
    }

    /// Processes one block if one is available. Returns whether it did.
    fn step(&mut self) -> bool {
        let block = self.config.inference_block;
        if self.input.slots() < block || self.output.slots() < block {
            return false;
        }

        let Ok(chunk) = self.input.read_chunk(block) else {
            return false;
        };
        let (first, second) = chunk.as_slices();
        self.input_block[..first.len()].copy_from_slice(first);
        self.input_block[first.len()..block].copy_from_slice(second);
        chunk.commit_all();

        self.run_model(block);
        self.apply_ramp(block);

        if let Ok(chunk) = self.output.write_chunk_uninit(block) {
            chunk.fill_from_iter(self.output_block[..block].iter().copied());
        }
        true
    }

    /// Fills `output_block` with the model's output, or the input if bypassed.
    fn run_model(&mut self, block: usize) {
        let destination = if self.status.is_bypassed() {
            &mut self.discard_block
        } else {
            &mut self.output_block
        };

        if let Err(error) = self
            .runner
            .process(&self.input_block[..block], &mut destination[..block])
        {
            // A failing model must not take the microphone down with it: pass
            // the input through and let the log say why it sounds unprocessed.
            tracing::error!(%error, "stage failed; passing audio through");
            self.output_block[..block].copy_from_slice(&self.input_block[..block]);
            return;
        }

        if self.status.is_bypassed() {
            self.output_block[..block].copy_from_slice(&self.input_block[..block]);
        }
    }

    /// Applies the switch envelope, swapping runners when the fade-out ends.
    fn apply_ramp(&mut self, block: usize) {
        self.take_pending_runner();
        if !self.ramp.is_switching() {
            return;
        }

        for index in 0..block {
            if self.ramp.wants_swap() {
                self.perform_swap();
            }
            self.output_block[index] *= self.ramp.next_gain();
        }
        self.status.set_switching(self.ramp.is_switching());
    }

    /// Starts a ramp if the control thread has queued a runner.
    fn take_pending_runner(&mut self) {
        if self.ramp.is_switching() || self.incoming.is_some() {
            return;
        }
        let Ok(runner) = self.pending.pop() else {
            return;
        };
        // Hold silence until the incoming model is certainly producing audio,
        // so the ramp fades in on signal rather than on a gap. The swap lands
        // partway through a block but the new runner is not fed until the next
        // one, so the worst case is its own latency plus one whole block; be
        // silent for that long. Overshooting only lengthens the dip slightly,
        // whereas undershooting fades in on silence and then steps to full
        // level when the model finally speaks — exactly the click the ramp
        // exists to prevent.
        let priming = runner.latency_samples() + self.config.inference_block;
        self.incoming = Some(runner);
        self.status.set_switching(true);
        self.ramp.begin(priming);
    }

    /// Installs the queued runner, retiring the current one.
    fn perform_swap(&mut self) {
        let Some(runner) = self.incoming.take() else {
            return;
        };
        let previous = core::mem::replace(&mut self.runner, runner);
        self.status.set_latency_ms(self.runner.latency_ms());
        // If the control thread has not drained the queue, the retired runner
        // is dropped here rather than leaked. That deallocates on the inference
        // thread, which only happens if the UI has stopped polling entirely.
        drop(self.retired.push(previous));
    }

    /// Hands everything back to the control thread on shutdown.
    fn retire_everything(self) {
        let Self {
            runner,
            incoming,
            mut retired,
            mut pending,
            ..
        } = self;
        drop(retired.push(runner));
        if let Some(incoming) = incoming {
            drop(retired.push(incoming));
        }
        while let Ok(runner) = pending.pop() {
            drop(retired.push(runner));
        }
    }
}

// `rtrb`'s queue ends are not `Debug`.
impl std::fmt::Debug for Worker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Worker")
            .field("config", &self.config)
            .field("runner", &self.runner)
            .field("switching", &self.ramp.is_switching())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::{Engine, EngineConfig};
    use noican_core::stage::{Passthrough, Stage, StageSpec};
    use std::time::{Duration, Instant};

    /// A stage that writes a constant, so a test can tell which one is active.
    #[derive(Debug)]
    struct Constant {
        spec: StageSpec,
        value: f32,
    }

    impl Constant {
        fn boxed(value: f32, block: usize) -> Box<dyn Stage> {
            Box::new(Self {
                spec: StageSpec::streaming(48_000, block),
                value,
            })
        }
    }

    impl Stage for Constant {
        fn spec(&self) -> StageSpec {
            self.spec
        }

        fn process(&mut self, _input: &[f32], output: &mut [f32]) -> noican_core::Result<()> {
            output.fill(self.value);
            Ok(())
        }

        fn reset(&mut self) {}
    }

    fn config() -> EngineConfig {
        EngineConfig {
            max_device_block: 256,
            inference_block: 128,
            fade_samples: 64,
            queue_capacity: 4_096,
            ..EngineConfig::default()
        }
    }

    /// Drives the bridge for `blocks` device buffers, paced at the real device
    /// rate so that the inference thread has the time it would really have.
    ///
    /// Returns the output split into runs of consecutive complete blocks. A
    /// dropout ends a run, so callers examining sample-to-sample continuity are
    /// not fooled by the silence the bridge substitutes — that is a starved
    /// queue, not a click.
    fn pump(bridge: &mut crate::AudioBridge, level: f32, blocks: usize) -> Vec<Vec<f32>> {
        const BLOCK: usize = 128;
        #[expect(clippy::cast_precision_loss, reason = "test fixture")]
        let period = Duration::from_secs_f64(BLOCK as f64 / 48_000.0);
        let input = vec![level; BLOCK];
        let mut output = vec![0.0; BLOCK];
        let mut runs: Vec<Vec<f32>> = vec![Vec::new()];

        let mut next = Instant::now();
        for _ in 0..blocks {
            next += period;
            if bridge.process(&input, &mut output) {
                runs.last_mut()
                    .expect("at least one run")
                    .extend_from_slice(&output);
            } else if !runs.last().expect("at least one run").is_empty() {
                runs.push(Vec::new());
            }
            let now = Instant::now();
            if next > now {
                std::thread::sleep(next - now);
            }
        }
        runs.retain(|run| !run.is_empty());
        runs
    }

    /// Largest sample-to-sample step anywhere inside a run.
    fn largest_step(runs: &[Vec<f32>]) -> f32 {
        runs.iter()
            .flat_map(|run| run.windows(2))
            .fold(0.0f32, |largest, pair| {
                largest.max((pair[1] - pair[0]).abs())
            })
    }

    #[test]
    fn rejects_an_inconsistent_configuration() {
        assert!(
            Engine::new(EngineConfig {
                sample_rate: 0,
                ..config()
            })
            .is_err()
        );
        assert!(
            Engine::new(EngineConfig {
                inference_block: 0,
                ..config()
            })
            .is_err()
        );
        assert!(
            Engine::new(EngineConfig {
                queue_capacity: 8,
                ..config()
            })
            .is_err()
        );
    }

    #[test]
    fn starting_twice_is_an_error() {
        let mut engine = Engine::new(config()).unwrap();
        let _bridge = engine
            .start(Box::new(Passthrough::new(48_000, 128)))
            .unwrap();
        assert!(engine.is_running());
        assert!(
            engine
                .start(Box::new(Passthrough::new(48_000, 128)))
                .is_err()
        );
    }

    #[test]
    fn switching_a_stopped_engine_is_an_error() {
        let mut engine = Engine::new(config()).unwrap();
        assert!(
            engine
                .set_stage(Box::new(Passthrough::new(48_000, 128)))
                .is_err()
        );
    }

    #[test]
    fn audio_flows_through_the_active_stage() {
        let mut engine = Engine::new(config()).unwrap();
        let mut bridge = engine.start(Constant::boxed(0.5, 128)).unwrap();

        let runs = pump(&mut bridge, 0.0, 200);
        assert!(
            runs.iter()
                .flatten()
                .any(|sample| (sample - 0.5).abs() < 1e-6),
            "the constant stage never reached the output"
        );
        assert!(engine.snapshot().running);
    }

    #[test]
    fn a_switch_reaches_the_output_without_a_click() {
        let mut engine = Engine::new(config()).unwrap();
        let mut bridge = engine.start(Constant::boxed(0.5, 128)).unwrap();

        // Let the first stage settle before measuring.
        pump(&mut bridge, 0.0, 100);

        engine.set_stage(Constant::boxed(-0.5, 128)).unwrap();
        let runs = pump(&mut bridge, 0.0, 400);

        assert!(
            runs.iter()
                .flatten()
                .any(|sample| (sample + 0.5).abs() < 1e-6),
            "the second stage never reached the output"
        );

        // The two stages differ by 1.0, so an unramped swap would show a step
        // of exactly that. The ramp has to keep every step far below it.
        let largest = largest_step(&runs);
        assert!(
            largest < 0.1,
            "output stepped by {largest} during the switch"
        );
    }

    #[test]
    fn bypass_passes_the_input_through() {
        let mut engine = Engine::new(config()).unwrap();
        let mut bridge = engine.start(Constant::boxed(0.5, 128)).unwrap();
        engine.set_bypass(true);

        let runs = pump(&mut bridge, 0.25, 200);
        assert!(
            runs.iter()
                .flatten()
                .any(|sample| (sample - 0.25).abs() < 1e-6),
            "bypass did not pass the input through"
        );
        assert!(engine.snapshot().bypassed);
    }

    #[test]
    fn stopping_returns_the_active_runner_for_disposal() {
        let mut engine = Engine::new(config()).unwrap();
        let mut bridge = engine.start(Constant::boxed(0.5, 128)).unwrap();
        pump(&mut bridge, 0.0, 50);
        engine.stop();
        assert!(!engine.is_running());
        // A second stop is harmless.
        engine.stop();
    }
}
