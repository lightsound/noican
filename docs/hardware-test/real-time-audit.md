# Hardware test: Real-time audit

Part of the [macOS Build and Hardware Test Plan](../macos-hardware-test.md).

## Real-time audit

Run Instruments with Allocations, System Trace, and Thread Sanitizer in
separate runs:

1. Identify the AUHAL render callback.
2. Confirm no heap allocation, Objective-C/Swift ARC traffic, mutex wait,
   file I/O, logging, or blocking syscall occurs in that callback.
3. Confirm the callback only calls `AudioUnitRender`, moves `f32` samples
   through the preallocated SPSC rings, and signals the worker's dispatch
   semaphore (a non-blocking call).
4. Confirm `noican-inference` joins the AUHAL `os_workgroup`. A failed join
   sets the engine fault flag and fails this test.
5. Confirm the `noican-inference` worker blocks between device callbacks:
   over a minute of engine-on idle time its CPU use must be far below one
   core (it waits on the semaphore; a busy-spinning worker fails this test).
6. Induce inference overload with the heaviest model. The callback must emit
   silence on output-ring underrun rather than block.
