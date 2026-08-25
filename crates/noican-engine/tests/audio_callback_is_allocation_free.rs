//! Proves the audio callback allocates nothing.
//!
//! `docs/tech-research.md` §9 forbids allocation, locks, and I/O on the audio
//! thread, and until now that was a claim in a comment. A comment cannot fail.
//! This installs a global allocator that counts allocations made while a flag is
//! set, and asserts the count stays at zero across thousands of callbacks —
//! including while a model switch is in flight, which is when the engine is
//! doing the most work behind the callback's back.
//!
//! Only the *callback* is covered. The inference thread is allowed to allocate:
//! it is not real-time, and ONNX Runtime's pooled allocations are outside our
//! control either way.

// A global allocator cannot be written without `unsafe`, and the workspace
// denies it everywhere by default.
#![expect(
    unsafe_code,
    reason = "implementing GlobalAlloc requires it; every method forwards to the system allocator \
              unchanged"
)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

use noican_core::stage::Passthrough;
use noican_core::{Stage, StageSpec};
use noican_engine::{Engine, EngineConfig};

/// Counts allocations, but only on threads that asked to be watched.
struct Counting;

thread_local! {
    /// Const-initialised so that reading it cannot itself allocate, and holding
    /// no destructor so no TLS teardown is registered.
    static WATCHING: Cell<bool> = const { Cell::new(false) };
}

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

impl Counting {
    fn note() {
        if WATCHING.get() {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
    }
}

// SAFETY: every method forwards to the system allocator unchanged; the only
// addition is a counter that touches no heap memory.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        Self::note();
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        Self::note();
        unsafe { System.realloc(pointer, layout, new_size) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        Self::note();
        unsafe { System.alloc_zeroed(layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Runs `body` with allocation counting on, returning how many it made.
fn count_allocations(body: impl FnOnce()) -> usize {
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    WATCHING.set(true);
    body();
    WATCHING.set(false);
    ALLOCATIONS.load(Ordering::Relaxed) - before
}

/// A stage that delays by a block, so switching to it has to prime.
#[derive(Debug)]
struct Delaying {
    spec: StageSpec,
    held: Vec<f32>,
}

impl Delaying {
    fn new(sample_rate: u32, block: usize) -> Self {
        Self {
            spec: StageSpec::streaming(sample_rate, block).with_latency(block),
            held: vec![0.0; block],
        }
    }
}

impl Stage for Delaying {
    fn spec(&self) -> StageSpec {
        self.spec
    }

    fn process(&mut self, input: &[f32], output: &mut [f32]) -> noican_core::Result<()> {
        output.copy_from_slice(&self.held);
        self.held.copy_from_slice(input);
        Ok(())
    }

    fn reset(&mut self) {
        self.held.fill(0.0);
    }
}

const RATE: u32 = 48_000;
const DEVICE_BLOCK: usize = 512;

fn engine() -> Engine {
    Engine::new(EngineConfig {
        sample_rate: RATE,
        max_device_block: DEVICE_BLOCK,
        ..EngineConfig::default()
    })
    .expect("the engine should build at a normal device block size")
}

#[test]
fn the_callback_allocates_nothing_in_steady_state() {
    let mut engine = engine();
    let mut bridge = engine
        .start(Box::new(Passthrough::new(RATE, 128)))
        .expect("the engine should start");

    let input = vec![0.25f32; DEVICE_BLOCK];
    let mut output = vec![0.0f32; DEVICE_BLOCK];

    // Warm up outside the watch: the first calls touch lazily-initialised
    // machinery in the standard library that a steady-state callback never
    // reaches again.
    for _ in 0..64 {
        bridge.process(&input, &mut output);
    }

    let allocations = count_allocations(|| {
        for _ in 0..10_000 {
            bridge.process(&input, &mut output);
        }
    });
    assert_eq!(
        allocations, 0,
        "the audio callback made {allocations} allocations across 10000 blocks"
    );

    engine.stop();
}

/// A switch is the interesting case: the control plane is swapping stages and
/// the callback keeps running through it. Nothing in that handover may allocate
/// on the callback's side.
#[test]
fn the_callback_allocates_nothing_across_a_model_switch() {
    let mut engine = engine();
    let mut bridge = engine
        .start(Box::new(Passthrough::new(RATE, 128)))
        .expect("the engine should start");

    let input = vec![0.1f32; DEVICE_BLOCK];
    let mut output = vec![0.0f32; DEVICE_BLOCK];
    for _ in 0..64 {
        bridge.process(&input, &mut output);
    }

    // The switch itself runs on this thread, and it is allowed to allocate, so
    // it stays outside the watched region.
    let mut allocations = 0;
    for round in 0..6 {
        let stage: Box<dyn Stage> = if round % 2 == 0 {
            Box::new(Delaying::new(RATE, 256))
        } else {
            Box::new(Passthrough::new(RATE, 128))
        };
        engine
            .set_stage(stage)
            .expect("the switch should be accepted");

        allocations += count_allocations(|| {
            for _ in 0..2_000 {
                bridge.process(&input, &mut output);
            }
        });
        engine.drain_retired();
    }

    assert_eq!(
        allocations, 0,
        "the audio callback made {allocations} allocations while models were switching"
    );

    engine.stop();
}

/// The counter has to be able to see an allocation, or the tests above would
/// pass with a broken allocator and prove nothing.
#[test]
fn the_counter_detects_an_allocation() {
    let observed = count_allocations(|| {
        let victim: Vec<u8> = Vec::with_capacity(4_096);
        std::hint::black_box(&victim);
    });
    assert!(
        observed > 0,
        "the allocation counter saw nothing while a Vec was being allocated, so the other tests \
         in this file prove nothing"
    );
}
