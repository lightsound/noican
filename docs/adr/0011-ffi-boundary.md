# 0011. FFI boundary

Status: Accepted

Moved from code comments on 2026-11-21 (the delete-comment pass).
The original wording is preserved under each topic; the file and
symbol each comment was attached to are noted above its text and
checked by `scripts/adr-links.py`.

## engine-crate

Decided 2026-11-28; recorded here because code comments are banned.

The live session (model preparation, the control state and its
epoch, the transport lifecycle, and the status readers) is the
`noican-engine` crate, a safe Rust API (`Engine`). `noican-ffi`
only translates: C strings in, a `Result` out as a status code plus
the `last_error` string. Before this the session logic lived inside
the `extern "C"` functions, so it could be driven only through raw
pointers: no Rust caller (the CLI, a test with a simulated
transport) could start a session, and every test of the control
flow had to go through the C ABI.

Errors are a typed `EngineError`. Its `Display` text is exactly the
string the C ABI recorded before, so what the app shows is
unchanged. `last_error` is a C idiom and stays in the FFI handle
behind its own lock. One consequence: a poisoned control lock now
yields `EngineError::Poisoned` with a message, where before the
call failed without recording one because the message lived behind
the poisoned lock.

`Engine::health` reads every status and counter of the running
transport under one acquisition of the control lock, and
`Engine::levels` reads the meters and the monitor state from
atomics without it. They match the app's two polling rates: health
at 1 Hz, meters at 20 Hz.

`ControlState::active_model` was written on every start and model
switch and never read; it is gone.

## engine-ffi-threading-contract

Source: `macos/Sources/NoicanMenuBar/RustEngine.swift`, `RustEngine`.

Thin wrapper over the Rust C ABI. The handle is internally synchronized
(a mutex-guarded control state on the Rust side), so calls may come from
any thread; long operations (weight download, model construction) are
expected to run off the main actor.

Source: `macos/Sources/NoicanMenuBar/RustEngine.swift`, `RustEngine.monitorState`.

Preview monitor state, one lock-free read — never waits on the
engine's control lock, so it is safe to poll at 20 Hz even while
a monitor start is in progress. The tripped protocol (AUHAL still
up but silenced; cleared by the next toggle) is defined on the
enum itself.

Source: `macos/Sources/NoicanMenuBar/RustEngine.swift`, `RustEngine.monitorDeviceID`.

Device the running preview monitor plays on (resolved on the Rust
side at enable time), or 0 while no monitor is up. Reads the
control mutex — meant for event-driven and 1 Hz callers, not the
20 Hz poll path.

## ffi-control-locking-and-epoch

Source: `crates/noican-ffi/src/lib.rs`.

C ABI used by the `SwiftUI` menu bar control plane.

Everything here runs on the control plane (Swift side): model download,
stage construction, and engine lifecycle. Prepared stages reach the
inference thread through [`noican_core::StagePublisher`]'s lock-free
queue, so no call in this module ever blocks the audio path.

Locking discipline: the control mutex is held only for short state
transitions, never across weight downloads or model construction, so
status queries (`is_running`, `is_faulted`, `last_error`) always return
promptly — regardless of what the UI does. Slow work runs unlocked and
commits its result only when the operation epoch is unchanged (a
concurrent `stop`/`start` supersedes it).

The model catalog is projected from `noican-models`'
[`noican_models::catalog`] at call time — neither this crate nor the UI
hardcodes model identifiers or names.

Since 2026-11-28 the control state, its lock and the epoch live in
`noican-engine` (see `engine-crate`); the discipline above is
unchanged and is now `Engine`'s.

Source: `crates/noican-engine/src/lib.rs`, `ControlState::epoch`.

Bumped by every `start`/`stop`; slow operations only commit when the
epoch they claimed is still current.

Source: `crates/noican-engine/src/lib.rs`, `Engine::state`.

Locked only for short transitions (see the crate docs). If a panic
poisons it, control calls fail with `EngineError::Poisoned` and the
readers return their fallback value (`Engine::health` reports a
fault).

Source: `crates/noican-engine/src/lib.rs`, `Engine::start_with`.

Shared start flow behind `Engine::start_aggregate` and
`Engine::start_native`: claims an epoch (tearing down any
running transport), prepares the stage and engine unlocked, starts
the runtime through `start_runtime`, and commits unless a newer
control operation superseded the epoch.

Source: `crates/noican-ffi/src/lib.rs`, `noican_engine_set_model`.

Prepares and lock-free publishes a replacement model.

Fails fast when the engine is not running; missing model weights are
downloaded first (on this control thread, without holding the control
lock).

# Safety

`handle` must be a live engine handle and `model_id` must point to a
valid NUL-terminated string for the duration of this call.

Source: `crates/noican-engine/src/lib.rs`, `Engine::health`.

Reads the running runtime under the control lock, or returns the
stopped values (`Health::default`) when there is none to read: a
stopped engine or, with `faulted` set, a poisoned lock. Holds the
control mutex, so it serves event-driven and 1 Hz callers, not the
20 Hz meter path.

Source: `crates/noican-ffi/src/lib.rs`, `health`.

The shared shape of the status and diagnostic getters of the C ABI:
`Engine::health` for a live handle, the stopped values for a null
one.

# Safety

`handle` must be null or a live engine handle.

## ffi-fields-outside-mutex

Source: `crates/noican-engine/src/lib.rs`, `Engine::levels`.

Peak meters shared with each runtime's inference worker. Kept
outside the control mutex so level polling reads plain atomics and
never waits on slow control work (weight downloads, model loads).
The worker resets it to silence on start and exit, so readers see
0 whenever the engine is stopped.

Source: `crates/noican-engine/src/lib.rs`, `Engine::monitor_state`.

The [`MonitorState`] cell shared with each runtime's monitor
control and inference worker. Kept outside the control mutex for
the same reason as `levels`: the UI polls it at 20 Hz and must
never wait on a slow monitor start. Monitor toggles move it
between off and playing, the worker's feedback guard moves it to
tripped, and it reads off whenever no runtime is up.

Source: `crates/noican-engine/src/lib.rs`, `Engine::intensity`.

The dry/wet ("strength") control shared with each runtime's
[`SwitchingEngine`]. One atomic, kept outside the control mutex:
slider moves apply instantly without waiting on slow control work
and never rebuild the engine. Owned by the handle (not the
runtime) so the value set while stopped carries into the next
start.

## ffi-handle-lifecycle

Source: `crates/noican-ffi/src/lib.rs`, `noican_engine_create`.

Creates an engine control handle.

A null `models_directory` selects `NOICAN_MODELS_DIR` or the platform
default (`~/Library/Application Support/noican/models` on macOS).

# Safety

A non-null `models_directory` must point to a valid NUL-terminated
string for the duration of this call.

Source: `crates/noican-ffi/src/lib.rs`, `noican_engine_destroy`.

Stops and destroys an engine handle.

# Safety

`handle` must be null or a live pointer returned by
[`noican_engine_create`], and it must be destroyed at most once.

Source: `crates/noican-ffi/src/lib.rs`, `noican_engine_stop`.

Stops AUHAL while preserving the reusable control handle.

# Safety

`handle` must be null or a live engine handle.

Source: `crates/noican-ffi/src/lib.rs`, `engine_handle`.

Borrows the engine behind a C handle, or `None` for a null one.

# Safety

`handle` must be null or a live engine handle that is not destroyed
while the borrow lasts (no caller keeps it past its own call).

## ffi-panic-guard

Source: `crates/noican-engine/src/lib.rs`, `guard_panics`.

Runs slow, panic-capable work (ONNX session construction, tar
extraction, ...) behind a panic guard: a Rust panic crossing the C ABI
would abort the whole app, so it is converted into an error string
instead.

Source: `crates/noican-engine/src/lib.rs`, `guard_panics`.

Nothing observes the closure's state after a panic; the result is
discarded wholesale, so broken invariants cannot leak.

## ffi-return-value-contracts

Source: `macos/Sources/NoicanMenuBar/RustEngine.swift`, `RustEngine.rating`.

One registry rating, clamped into the displayable 0–5 range (the
FFI returns -1 only for arguments this caller never passes).

## ffi-string-copy

Source: `crates/noican-ffi/src/lib.rs`, `copy_string`.

A truncated copy must end on a UTF-8 char boundary — a multibyte
sequence cut in the middle yields invalid UTF-8 for the caller
even though it is NUL-terminated.

## split-transport-ffi-contract

Source: `macos/Sources/NoicanMenuBar/RustEngine.swift`, `RustEngine.startNative`.

Starts the split transport for a microphone that cannot run at
the 48 kHz engine rate (issue #7): the microphone is captured
natively at `captureRate` (its current nominal rate; the standard
rate families from 8 to 192 kHz — Bluetooth telephony profiles,
the 44.1 kHz family, 88.2/96 kHz interfaces; the Rust side refuses
exotic rates whose ratio to 48 kHz does not reduce to small
terms) and resampled to 48 kHz inside
the transport by the exact rational ratio, with clock drift
between the two devices compensated there. No Aggregate Device
is involved. The virtual output's channel count is not passed:
the Rust side reads it from the device once its output AUHAL is
bound to it and sizes the render format from that (one client
channel per device channel; a device reporting none refuses the
start).
