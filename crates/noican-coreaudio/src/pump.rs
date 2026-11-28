//! See docs/adr/0003-coreaudio-transport.md#worker-pump.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use noican_core::{DriftServo, InputResampler, SwitchingEngine};
use rtrb::{Consumer, Producer};

use crate::WORKER_BLOCK_SAMPLES;
use crate::callback::shed_capture_backlog;
use crate::monitor::MonitorTee;
use crate::observe::{StreamLevels, WorkerBlockStats};

pub(crate) const NATIVE_CHUNK_SAMPLES: usize = 480;

pub(crate) const OUTPUT_STALL_BLOCKS: usize = 300;

pub(crate) struct PumpLinks {
    pub(crate) engine: SwitchingEngine,
    pub(crate) input: Consumer<f32>,
    pub(crate) output: Producer<f32>,
    pub(crate) tee: MonitorTee,
    pub(crate) levels: Arc<StreamLevels>,
    pub(crate) faulted: Arc<AtomicBool>,
    pub(crate) input_overruns: Arc<AtomicU64>,
    pub(crate) block_stats: Arc<WorkerBlockStats>,
}

pub(crate) struct ClockBridge {
    resampler: InputResampler,
    servo: DriftServo,
    output_pulses: Arc<AtomicU64>,
    converted: Vec<f32>,
    last_output_pulses: u64,
    output_idle_blocks: usize,
}

impl ClockBridge {
    pub(crate) fn new(
        resampler: InputResampler,
        servo: DriftServo,
        output_pulses: Arc<AtomicU64>,
    ) -> Self {
        let converted = Vec::with_capacity(resampler.max_output_len(NATIVE_CHUNK_SAMPLES));
        Self {
            resampler,
            servo,
            output_pulses,
            converted,
            last_output_pulses: 0,
            output_idle_blocks: 0,
        }
    }

    fn follow(&mut self, links: &PumpLinks, pending: usize) {
        let buffered = (links.output.buffer().capacity() - links.output.slots())
            + pending
            + self.resampler.to_engine_samples(links.input.slots());
        self.resampler.set_drift_ppm(self.servo.update(buffered));
        let pulses = self.output_pulses.load(Ordering::Relaxed);
        if pulses == self.last_output_pulses {
            self.output_idle_blocks = self.output_idle_blocks.saturating_add(1);
            if self.output_idle_blocks >= OUTPUT_STALL_BLOCKS {
                links.faulted.store(true, Ordering::Release);
            }
        } else {
            self.last_output_pulses = pulses;
            self.output_idle_blocks = 0;
        }
    }
}

pub(crate) struct Pump {
    links: PumpLinks,
    bridge: Option<ClockBridge>,
    chunk: [f32; NATIVE_CHUNK_SAMPLES],
    pending: VecDeque<f32>,
    input_block: [f32; WORKER_BLOCK_SAMPLES],
    output_block: [f32; WORKER_BLOCK_SAMPLES],
}

impl Pump {
    pub(crate) fn new(links: PumpLinks, bridge: Option<ClockBridge>) -> Self {
        let converted = bridge
            .as_ref()
            .map_or(0, |bridge| bridge.converted.capacity());
        links.levels.reset();
        links.block_stats.reset();
        Self {
            links,
            bridge,
            chunk: [0.0; NATIVE_CHUNK_SAMPLES],
            pending: VecDeque::with_capacity(converted + WORKER_BLOCK_SAMPLES),
            input_block: [0.0; WORKER_BLOCK_SAMPLES],
            output_block: [0.0; WORKER_BLOCK_SAMPLES],
        }
    }

    pub(crate) fn step(&mut self) -> bool {
        let room = if self.bridge.is_some() {
            NATIVE_CHUNK_SAMPLES
        } else {
            if self.pending.is_empty() {
                shed_capture_backlog(
                    &mut self.links.input,
                    WORKER_BLOCK_SAMPLES,
                    &self.links.input_overruns,
                );
            }
            WORKER_BLOCK_SAMPLES - self.pending.len()
        };
        let mut popped = 0;
        while popped < room {
            let Ok(sample) = self.links.input.pop() else {
                break;
            };
            self.chunk[popped] = sample;
            popped += 1;
        }
        if popped == 0 {
            return false;
        }
        let fresh = &self.chunk[..popped];
        if let Some(bridge) = &mut self.bridge {
            bridge.converted.clear();
            bridge.resampler.process(fresh, &mut bridge.converted);
            self.pending.extend(bridge.converted.iter().copied());
        } else {
            self.pending.extend(fresh.iter().copied());
        }
        while self.pending.len() >= WORKER_BLOCK_SAMPLES {
            for slot in &mut self.input_block {
                *slot = self.pending.pop_front().unwrap_or(0.0);
            }
            self.run_block();
            if let Some(bridge) = &mut self.bridge {
                bridge.follow(&self.links, self.pending.len());
            }
        }
        true
    }

    fn run_block(&mut self) {
        let started = std::time::Instant::now();
        let links = &mut self.links;
        if links
            .engine
            .process_block(&self.input_block, &mut self.output_block)
            .is_err()
        {
            self.output_block.fill(0.0);
            links.faulted.store(true, Ordering::Release);
        }
        links.levels.update(&self.input_block, &self.output_block);
        let _teed = links.tee.feed(&self.output_block);
        for sample in self.output_block.iter().copied() {
            let _ignored = links.output.push(sample);
        }
        links
            .block_stats
            .record(u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX));
    }
}

impl Drop for Pump {
    fn drop(&mut self) {
        self.links.levels.reset();
        self.links.block_stats.reset();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicI32;

    use noican_core::{IntensityControl, Passthrough, Stage, StageError};
    use rtrb::RingBuffer;

    use super::*;
    use crate::callback::{CaptureFeed, OutputFeed};
    use crate::monitor::MonitorState;

    const RING_CAPACITY: usize = 48_000;
    const PRIME_SAMPLES: usize = 2_400;
    const CALLBACK_FRAMES: usize = 512;
    const TICKS_PER_SECOND: usize = 94;

    struct Failing;

    impl Stage for Failing {
        fn id(&self) -> &'static str {
            "failing"
        }

        fn process_block(&mut self, _input: &[f32], _output: &mut [f32]) -> Result<(), StageError> {
            Err(StageError::Inference("simulated".to_owned()))
        }

        fn latency_samples(&self) -> usize {
            0
        }

        fn reset(&mut self) {}
    }

    struct FormerLoops {
        links: PumpLinks,
        bridge: Option<ClockBridge>,
        position: usize,
        input_block: [f32; WORKER_BLOCK_SAMPLES],
        pending: VecDeque<f32>,
    }

    impl FormerLoops {
        fn run_block(&mut self, output_block: &mut [f32]) {
            let links = &mut self.links;
            if links
                .engine
                .process_block(&self.input_block, output_block)
                .is_err()
            {
                output_block.fill(0.0);
                links.faulted.store(true, Ordering::Release);
            }
            links.levels.update(&self.input_block, output_block);
            let _teed = links.tee.feed(output_block);
            for sample in output_block.iter().copied() {
                let _ignored = links.output.push(sample);
            }
        }

        fn drain_shared_clock(&mut self) {
            let mut output_block = [0.0_f32; WORKER_BLOCK_SAMPLES];
            loop {
                if self.position == 0 {
                    shed_capture_backlog(
                        &mut self.links.input,
                        WORKER_BLOCK_SAMPLES,
                        &self.links.input_overruns,
                    );
                }
                let Ok(sample) = self.links.input.pop() else {
                    return;
                };
                self.input_block[self.position] = sample;
                self.position += 1;
                if self.position == WORKER_BLOCK_SAMPLES {
                    self.run_block(&mut output_block);
                    self.position = 0;
                }
            }
        }

        fn drain_own_clock(&mut self) {
            let output_capacity = self.links.output.buffer().capacity();
            let mut native_chunk = [0.0_f32; NATIVE_CHUNK_SAMPLES];
            let mut output_block = [0.0_f32; WORKER_BLOCK_SAMPLES];
            loop {
                let mut popped = 0;
                while popped < NATIVE_CHUNK_SAMPLES {
                    let Ok(sample) = self.links.input.pop() else {
                        break;
                    };
                    native_chunk[popped] = sample;
                    popped += 1;
                }
                if popped == 0 {
                    return;
                }
                let Some(bridge) = self.bridge.as_mut() else {
                    return;
                };
                bridge.converted.clear();
                bridge
                    .resampler
                    .process(&native_chunk[..popped], &mut bridge.converted);
                self.pending.extend(bridge.converted.iter().copied());
                while self.pending.len() >= WORKER_BLOCK_SAMPLES {
                    for slot in &mut self.input_block {
                        *slot = self.pending.pop_front().unwrap_or(0.0);
                    }
                    self.run_block(&mut output_block);
                    let Some(bridge) = self.bridge.as_mut() else {
                        return;
                    };
                    let buffered = (output_capacity - self.links.output.slots())
                        + self.pending.len()
                        + bridge.resampler.to_engine_samples(self.links.input.slots());
                    bridge
                        .resampler
                        .set_drift_ppm(bridge.servo.update(buffered));
                    let pulses = bridge.output_pulses.load(Ordering::Relaxed);
                    if pulses == bridge.last_output_pulses {
                        bridge.output_idle_blocks = bridge.output_idle_blocks.saturating_add(1);
                        if bridge.output_idle_blocks >= OUTPUT_STALL_BLOCKS {
                            self.links.faulted.store(true, Ordering::Release);
                        }
                    } else {
                        bridge.last_output_pulses = pulses;
                        bridge.output_idle_blocks = 0;
                    }
                }
            }
        }
    }

    enum Worker {
        Pump(Box<Pump>),
        Former(Box<FormerLoops>),
    }

    struct Rig {
        worker: Worker,
        capture: CaptureFeed,
        render: OutputFeed,
        pulses: Arc<AtomicU64>,
        faulted: Arc<AtomicBool>,
        overruns: Arc<AtomicU64>,
        underruns: Arc<AtomicU64>,
        block_stats: Arc<WorkerBlockStats>,
        next: u32,
        owed: u64,
        rendered: Vec<f32>,
    }

    impl Rig {
        fn new(stage: Box<dyn Stage>, capture_rate: Option<u32>) -> Self {
            Self::build(stage, capture_rate, false)
        }

        fn build(stage: Box<dyn Stage>, capture_rate: Option<u32>, former: bool) -> Self {
            let (input_producer, input) = RingBuffer::new(RING_CAPACITY);
            let (mut output, output_consumer) = RingBuffer::new(RING_CAPACITY);
            let (monitor_producer, _monitor_consumer) = RingBuffer::new(RING_CAPACITY);
            let (_publisher, engine) = SwitchingEngine::new(
                stage,
                240,
                WORKER_BLOCK_SAMPLES,
                IntensityControl::default(),
            )
            .expect("engine");
            let faulted = Arc::new(AtomicBool::new(false));
            let overruns = Arc::new(AtomicU64::new(0));
            let underruns = Arc::new(AtomicU64::new(0));
            let block_stats = Arc::new(WorkerBlockStats::new());
            let pulses = Arc::new(AtomicU64::new(0));
            let bridge = capture_rate.map(|rate| {
                for _ in 0..PRIME_SAMPLES {
                    output.push(0.0).expect("room to prime");
                }
                ClockBridge::new(
                    InputResampler::new(rate, NATIVE_CHUNK_SAMPLES).expect("resampler"),
                    DriftServo::new(PRIME_SAMPLES),
                    Arc::clone(&pulses),
                )
            });
            let links = PumpLinks {
                engine,
                input,
                output,
                tee: MonitorTee::new(
                    monitor_producer,
                    Arc::new(AtomicI32::new(MonitorState::Off.as_raw())),
                ),
                levels: Arc::new(StreamLevels::new()),
                faulted: Arc::clone(&faulted),
                input_overruns: Arc::clone(&overruns),
                block_stats: Arc::clone(&block_stats),
            };
            let worker = if former {
                let converted = bridge
                    .as_ref()
                    .map_or(0, |bridge| bridge.converted.capacity());
                Worker::Former(Box::new(FormerLoops {
                    links,
                    bridge,
                    position: 0,
                    input_block: [0.0; WORKER_BLOCK_SAMPLES],
                    pending: VecDeque::with_capacity(converted + WORKER_BLOCK_SAMPLES),
                }))
            } else {
                Worker::Pump(Box::new(Pump::new(links, bridge)))
            };
            Self {
                worker,
                capture: CaptureFeed::new(input_producer, Arc::clone(&overruns)),
                render: OutputFeed::new(output_consumer, Arc::clone(&underruns)),
                pulses,
                faulted,
                overruns,
                underruns,
                block_stats,
                next: 0,
                owed: 0,
                rendered: Vec::new(),
            }
        }

        fn capture(&mut self, frames: usize) {
            let mut block = Vec::with_capacity(frames);
            for _ in 0..frames {
                self.next += 1;
                block.push(signal(self.next));
            }
            self.capture.push(&block);
        }

        fn work(&mut self) {
            match &mut self.worker {
                Worker::Pump(pump) => while pump.step() {},
                Worker::Former(former) if former.bridge.is_some() => former.drain_own_clock(),
                Worker::Former(former) => former.drain_shared_clock(),
            }
        }

        fn render(&mut self, frames: usize) {
            let start = self.rendered.len();
            self.rendered.resize(start + frames, 0.0);
            self.render.render(&mut self.rendered[start..], 1);
            self.pulses.fetch_add(1, Ordering::Relaxed);
        }

        fn own_clock_tick(&mut self, rate: u32, drift_ppm: i32, capturing: bool) {
            const PERIOD: u64 = 48_000 * 1_000_000;
            self.owed +=
                u64::from(rate) * u64::from(1_000_000_u32.saturating_add_signed(drift_ppm));
            while self.owed >= PERIOD {
                self.owed -= PERIOD;
                if capturing {
                    self.capture(CALLBACK_FRAMES);
                    self.work();
                }
            }
            self.render(CALLBACK_FRAMES);
        }

        fn occupancy(&self) -> usize {
            let Worker::Pump(pump) = &self.worker else {
                return 0;
            };
            let queued = pump.links.input.slots();
            (pump.links.output.buffer().capacity() - pump.links.output.slots())
                + pump.pending.len()
                + pump
                    .bridge
                    .as_ref()
                    .map_or(queued, |bridge| bridge.resampler.to_engine_samples(queued))
        }

        fn audible(&self) -> Vec<f32> {
            self.rendered
                .iter()
                .copied()
                .filter(|sample| sample.to_bits() != 0)
                .collect()
        }

        fn faulted(&self) -> bool {
            self.faulted.load(Ordering::Acquire)
        }

        fn overruns(&self) -> u64 {
            self.overruns.load(Ordering::Relaxed)
        }

        fn underruns(&self) -> u64 {
            self.underruns.load(Ordering::Relaxed)
        }
    }

    #[expect(clippy::cast_precision_loss, reason = "the values stay far below 2^24")]
    fn signal(index: u32) -> f32 {
        ((index % 1_000) + 1) as f32 / 1_000.0
    }

    fn next_choice(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        *seed >> 16
    }

    #[test]
    fn a_shared_clock_passes_every_sample_through_in_order() {
        let mut rig = Rig::new(Box::new(Passthrough), None);
        for _ in 0..200 {
            rig.capture(CALLBACK_FRAMES);
            rig.work();
            rig.render(CALLBACK_FRAMES);
        }

        let audible = rig.audible();
        assert!(audible.len() > 190 * CALLBACK_FRAMES, "{}", audible.len());
        let mut expected = 0;
        for sample in audible {
            expected += 1;
            assert_eq!(sample.to_bits(), signal(expected).to_bits(), "{expected}");
        }
        let settled = &rig.rendered[100 * CALLBACK_FRAMES..];
        assert!(settled.iter().all(|sample| sample.to_bits() != 0));
        assert_eq!((rig.overruns(), rig.underruns()), (0, 0));
        assert_eq!(rig.block_stats.blocks(), 213);
        assert!(!rig.faulted());

        drop(rig.worker);
        assert_eq!(rig.block_stats.blocks(), 0);
    }

    #[test]
    fn a_stalled_shared_clock_worker_sheds_all_but_the_newest_block() {
        let mut rig = Rig::new(Box::new(Passthrough), None);
        for _ in 0..20 {
            rig.capture(CALLBACK_FRAMES);
        }
        rig.work();
        rig.render(CALLBACK_FRAMES);

        let queued = 20 * CALLBACK_FRAMES;
        let stale = queued - WORKER_BLOCK_SAMPLES;
        assert_eq!(rig.overruns(), u64::try_from(stale).expect("fits"));
        let first = u32::try_from(stale + 1).expect("fits");
        assert_eq!(rig.rendered[0].to_bits(), signal(first).to_bits());
    }

    #[test]
    fn an_own_clock_capture_is_held_near_the_primed_depth_despite_drift() {
        for (rate, drift_ppm) in [(48_000, -300), (44_100, 300)] {
            let mut rig = Rig::new(Box::new(Passthrough), Some(rate));
            let (mut low, mut high) = (usize::MAX, 0);
            for _ in 0..40 * TICKS_PER_SECOND {
                rig.own_clock_tick(rate, drift_ppm, true);
                low = low.min(rig.occupancy());
                high = high.max(rig.occupancy());
            }

            let case = format!("{rate} Hz at {drift_ppm} ppm: {low}..{high}");
            assert!(
                low >= PRIME_SAMPLES - 2 * CALLBACK_FRAMES - WORKER_BLOCK_SAMPLES,
                "{case}"
            );
            assert!(high <= PRIME_SAMPLES + CALLBACK_FRAMES, "{case}");
            assert_eq!((rig.overruns(), rig.underruns()), (0, 0), "{case}");
            assert!(!rig.faulted(), "{case}");
        }
    }

    #[test]
    fn a_capture_dropout_underruns_then_recovers_without_a_fault() {
        let mut rig = Rig::new(Box::new(Passthrough), Some(48_000));
        let dropout = 5 * TICKS_PER_SECOND..5 * TICKS_PER_SECOND + 28;
        let mut after_dropout = 0;
        let mut shallowest = usize::MAX;
        for tick in 0..40 * TICKS_PER_SECOND {
            rig.own_clock_tick(48_000, 100, !dropout.contains(&tick));
            if tick == dropout.end {
                after_dropout = rig.underruns();
            }
            if tick >= dropout.end {
                shallowest = shallowest.min(rig.occupancy());
            }
        }

        assert!(after_dropout > 20, "{after_dropout}");
        assert_eq!(rig.underruns(), after_dropout);
        assert!(shallowest < CALLBACK_FRAMES, "{shallowest}");
        assert!(
            rig.occupancy() > PRIME_SAMPLES - 2 * CALLBACK_FRAMES,
            "{}",
            rig.occupancy()
        );
        assert!(!rig.faulted());
    }

    #[test]
    fn a_silent_output_device_faults_only_an_own_clock_pump() {
        let mut own = Rig::new(Box::new(Passthrough), Some(48_000));
        let mut shared = Rig::new(Box::new(Passthrough), None);
        for _ in 0..OUTPUT_STALL_BLOCKS - 10 {
            own.capture(NATIVE_CHUNK_SAMPLES);
            own.work();
        }
        assert!(!own.faulted());

        own.render(CALLBACK_FRAMES);
        for _ in 0..OUTPUT_STALL_BLOCKS - 10 {
            own.capture(NATIVE_CHUNK_SAMPLES);
            own.work();
        }
        assert!(!own.faulted());

        for _ in 0..20 {
            own.capture(NATIVE_CHUNK_SAMPLES);
            own.work();
        }
        assert!(own.faulted());

        for _ in 0..2 * OUTPUT_STALL_BLOCKS {
            shared.capture(NATIVE_CHUNK_SAMPLES);
            shared.work();
        }
        assert!(!shared.faulted());
    }

    #[test]
    fn a_failing_stage_renders_silence_and_faults() {
        let mut rig = Rig::new(Box::new(Failing), None);
        rig.capture(CALLBACK_FRAMES);
        rig.work();
        rig.render(WORKER_BLOCK_SAMPLES);

        assert!(rig.faulted());
        assert!(rig.rendered.iter().all(|sample| sample.to_bits() == 0));
        assert_eq!(rig.block_stats.blocks(), 1);
    }

    #[test]
    fn the_pump_matches_the_loops_it_replaced() {
        for capture_rate in [None, Some(44_100), Some(48_000)] {
            let mut pump = Rig::build(Box::new(Passthrough), capture_rate, false);
            let mut former = Rig::build(Box::new(Passthrough), capture_rate, true);
            let mut seed = 0x2026_1128;
            for step in 0..2_400 {
                let choice = next_choice(&mut seed) % 64;
                let starved = (step / 300) % 2 == 1;
                for rig in [&mut pump, &mut former] {
                    match choice {
                        0 if !starved => {
                            for _ in 0..120 {
                                rig.capture(CALLBACK_FRAMES);
                            }
                        }
                        1..=20 if !starved || choice <= 6 => rig.capture(CALLBACK_FRAMES),
                        21..=24 => rig.capture(64),
                        25..=42 => rig.work(),
                        _ => rig.render(CALLBACK_FRAMES),
                    }
                }
            }

            let case = format!("{capture_rate:?}");
            assert_eq!(pump.rendered.len(), former.rendered.len(), "{case}");
            for (index, (ours, theirs)) in pump.rendered.iter().zip(&former.rendered).enumerate() {
                assert_eq!(ours.to_bits(), theirs.to_bits(), "{case} sample {index}");
            }
            assert!(pump.audible().len() > 200_000, "{case}");
            assert_eq!(pump.overruns(), former.overruns(), "{case}");
            assert_eq!(pump.underruns(), former.underruns(), "{case}");
            assert_eq!(pump.faulted(), former.faulted(), "{case}");
            assert!(pump.overruns() > 0 && pump.underruns() > 0, "{case}");
        }
    }
}
