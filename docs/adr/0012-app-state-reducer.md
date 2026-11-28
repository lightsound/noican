# 0012. App state and reducer contracts

Status: Accepted

Moved from code comments on 2026-11-21 (the delete-comment pass).
The original wording is preserved under each topic; the file and
symbol each comment was attached to are noted above its text and
checked by `scripts/adr-links.py`.

## app-effect-completion-contracts

Source: `macos/NoicanState/Sources/NoicanState/AppEffect.swift`, `AppEffect.stopEngine`.

Tear the transport down (engine stop + aggregate destroy).
Synchronous and idempotent; has no completion event.

Source: `macos/NoicanState/Sources/NoicanState/AppEffect.swift`, `AppEffect.startEngine`.

Build the private aggregate around the attempt's microphone and
start the engine (asynchronous: aggregate creation polls the
device until alive and the engine start may download weights).
Completes with `AppEvent.startCompleted`.

Source: `macos/NoicanState/Sources/NoicanState/AppEffect.swift`, `AppEffect.setMonitor`.

Toggle the preview self-monitor on the live transport
(asynchronous: starting an output device can take a moment).
Completes with `AppEvent.monitorChangeCompleted`.

Source: `macos/NoicanState/Sources/NoicanState/AppEffect.swift`, `AppEffect.switchModel`.

Prepare and lock-free publish a replacement model (asynchronous:
weight download and model construction). Completes with
`AppEvent.modelSwitchCompleted`.

Source: `macos/NoicanState/Sources/NoicanState/AppEffect.swift`, `AppEffect.setIntensity`.

Publish the dry/wet strength to the engine handle's shared atomic.
Synchronous and lock-free (one atomic store, applied mid-stream
without rebuilding anything; a value set while stopped seeds the
next start); has no completion event.

Source: `macos/NoicanState/Sources/NoicanState/AppEffect.swift`, `AppEffect.setLaunchAtLogin`.

Register or unregister the app as an `SMAppService` login item
(asynchronous: the attempt can fail depending on the app's
location and signature). Completes with
`AppEvent.launchAtLoginChangeCompleted`, which carries the
re-read real status.

## app-reducer-invariants

Source: `macos/NoicanState/Sources/NoicanState/AppReducer.swift`, `AppReducer`.

The pure transition function of the menu bar app.

`reduce` maps one (state, event) pair to the next state plus the
effects the runtime shell must perform. Every transition — including
the ones that used to be buried in detached-task completion callbacks
(start results, monitor toggles, the one-shot microphone fallback) —
is an explicit event → state change here, so the whole machine is
unit-testable without Core Audio or the Rust engine.

Invariants encoded by construction rather than by discipline:
- `AppModel.mode` is the user's intent; no event other than
  `modeSelected` writes it.
- At most one `EngineTransition` is in flight (`EngineMachine.busy`),
  and every event that would start another one is ignored until it
  settles — the old `isBusy` serialization.
- While busy the UI renders the `rendering` snapshot; state the UI
  projects changes only when a transition settles.
- Message-slot clearing is declared per event, in one place each.

## bluetooth-telephony-capture

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `AppModel.microphoneNotice`.

Informational caption for a native-rate selection, shown under
the microphone list (secondary style — a property of the device,
not an error). Telephony-profile rates: Bluetooth headset
microphones are captured at their narrow-band native rate and
resampled (issue #7), which cannot restore full-band quality, and
using the headset's microphone drops the whole headset into the
phone profile, so its *playback* quality degrades too while the
engine runs. Full-band rates (44.1 kHz and up) only note the
conversion — nothing about the audio is narrow-band. A pure
projection of the selection — no state, no clearing rules.

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.startEngine`.

The effective capture rate settles when capture
starts (Bluetooth renegotiates profiles), so read
it as late as possible; the Rust side validates it.

## busy-guard-before-engine-access

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.setMode`.

Environment reads the pure reducer cannot perform, sampled at
dispatch time. Guarded on busy first so a tap during a
transition never touches the engine (the monitor-target check
is lock-free, but `isRunning` takes the control mutex).

## completion-event-error-shape

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `Result.errorMessage`.

The failure as a user-facing message, or nil on success — the
shape completion events carry (`Result` itself is not `Equatable`).

## effect-completion-inflight-state

Source: `macos/NoicanState/Sources/NoicanState/AppEvent.swift`, `AppEvent.monitorChangeCompleted`.

`AppEffect.setMonitor` finished. Which direction was requested is
already recorded in the in-flight `EngineTransition.settingMonitor`.

Source: `macos/NoicanState/Sources/NoicanState/AppEvent.swift`, `AppEvent.modelSwitchCompleted`.

`AppEffect.switchModel` finished. The target model is recorded in
the in-flight `EngineTransition.switchingModel`.

## effect-completion-reentrancy

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.startEngine`.

The reducer pre-flighted all three against the same
snapshot, so this cannot happen; fail the attempt cleanly
rather than trap. Deferred: completions never re-enter
`dispatch` while an effect list is still being performed.

## engine-availability-gating

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `AppModel.isEngineAvailable`.

Whether the Rust engine handle was created. When false, mode and
selection changes still update the pickers but never claim engine
transitions.

## engine-fault-handling

Source: `macos/NoicanState/Sources/NoicanState/AppEvent.swift`, `AppEvent.engineFaulted`.

The health poll found the engine faulted (audio callback,
workgroup, or inference fault). The transport stays up; the next
user action tears it down.

## engine-fault-keeps-transport

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `EnginePhase.failed`.

The last attempt failed. `session` is non-nil when the transport
is still up despite the failure (an engine fault reports failure
without tearing the transport down; the next user action does).

## engine-fault-semantics

Source: `crates/noican-ffi/src/lib.rs`, `NoicanHealth::faulted`.

Set after an audio callback, workgroup, or inference fault.

## engine-fault-vs-runtime-stop

Source: `macos/NoicanState/Sources/NoicanState/AppReducer.swift`, `AppReducer.engineFaulted`.

An engine fault reports failure without tearing the transport
down (the session stays in the failed phase); the next user
action rebuilds it.

Source: `macos/NoicanState/Sources/NoicanState/AppReducer.swift`, `AppReducer.runtimeStopObserved`.

Runtime stops (device loss, stalls) tear the engine down but keep
the mode: the pill shows what the user asked for, the red tint and
the message say it is not running, and selecting another
microphone (or re-tapping the segment) restarts into that mode.

## engine-mode-user-intent

Source: `macos/NoicanState/Sources/NoicanState/EngineMode.swift`, `EngineMode`.

The single top-level control: Off, Preview (engine + self-monitor on
the default output), or On (engine only, for meetings). Preview and On
both feed the virtual microphone; the only difference is the monitor
tee, so switching between them is instant and click-free.

The mode is the *user's intent* and only the user changes it: the
system never moves the selection. When the selected mode is not
actually delivering, the failure renders as the warning tint plus the
reason under the control, and re-tapping the same segment retries.

## engine-sample-rate-contract

Source: `crates/noican-core/src/stage.rs`, `ENGINE_SAMPLE_RATE`.

The fixed sample rate (Hz) at which the engine exchanges audio with
stages. Models running at other rates are wrapped by
[`crate::framed::FramedStage`], which resamples internally.

Source: `crates/noican-core/src/stage.rs`, `FrameProcessor::sample_rate`.

Native sample rate (Hz). Must divide [`ENGINE_SAMPLE_RATE`].

## engine-state-machine

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `EngineSession`.

The live transport as the state machine knows it: which model and
microphone the engine was built around, and whether the preview
monitor tee is armed. Carried by every state that has (or renders) a
running engine, so nothing about the transport is tracked in loose
fields.

## engine-transition-serialization

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `EngineTransition`.

The one transition that may be in flight. Exactly one exists at a
time (`EngineMachine.busy`), which is what serialized every engine
operation behind the old `isBusy` flag — now by construction.

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `EngineMachine.busy`.

One transition in flight. Events that would start another one are
ignored until this settles (the old `isBusy` serialization).

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.setMonitor`.

Sends the monitor toggle to the engine off the main actor,
serialized like every other engine transition: the busy machine
state blocks concurrent mode changes (and the pollers' engine
calls) until the outcome lands, so rapid taps cannot interleave.

## header-status-projection

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `AppModel.statusText`.

One-line status for the header. The whole header is a projection
of the settled snapshot (`phase`) — including whether the preview
monitor is actually playing — so transitions do not churn this
text (the spinner is the busy feedback). Deliberately never
multi-line: text above the mode control must not change the
control's vertical position (the sliding pill would move
mid-animation). Full failure text lives in `engineErrorMessage`,
shown below the control instead.

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `AppModel.statusText`.

No model name here: the Model picker below already shows it
(and reverts on failed switches, so it never lies).
"Previewing" only while the monitor settled as playing;
after a trip or a monitor failure the engine still runs but
the preview does not.

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `AppModel.engineErrorMessage`.

Full engine failure text, displayed under the mode control (with
the preview messages) so the header height stays constant. It
stays visible while a retry is in flight and clears only when the
retry actually succeeds.

## launch-at-login-optimistic-toggle

Source: `macos/NoicanState/Sources/NoicanState/AppEvent.swift`, `AppEvent.launchAtLoginToggled`.

The login-item toggle was flipped. Optimistic: the toggle moves
immediately and `launchAtLoginChangeCompleted` snaps it back to
the real status when registration fails. Serialized: at most one
registration attempt is in flight, and a flip while one is
pending is ignored (concurrent attempts would race).

Source: `macos/NoicanState/Sources/NoicanState/AppEvent.swift`, `AppEvent.launchAtLoginChangeCompleted`.

`AppEffect.setLaunchAtLogin` finished. `isEnabled` is the real
`SMAppService` status re-read after the attempt (registration
depends on the app's location and signature, so the outcome —
not the request — is what the toggle must show; registered but
pending the user's approval counts as on), and `error` renders
under the toggle when something needs the user's attention — a
failure reason or the pending-approval notice.

Source: `macos/NoicanState/Sources/NoicanState/AppEvent.swift`, `AppEvent.launchAtLoginStatusRead`.

The `SMAppService` login-item status read at launch (the service
itself is the source of truth; nothing about it is persisted by
the app).

Source: `macos/Sources/NoicanMenuBar/MenuView.swift`, `MenuView.footer`.

The toggle renders the settled/optimistic reducer state;
a failed registration snaps it back with the reason
shown below (registration depends on the app's location
and signature — see docs/macos-hardware-test.md).
A checkbox, not a switch: this is a secondary setting
in a utility footer, and macOS reserves the prominent
switch style for primary states. Deliberately not
disabled while a registration attempt is in flight —
the reducer's serialization already ignores such
clicks, and the disabled dimming made the label
flicker gray on every click.

## launch-at-login-serialization

Source: `macos/NoicanState/Sources/NoicanState/AppReducer.swift`, `AppReducer.launchAtLoginToggled`.

The login-item toggle moves optimistically (the registration
attempt follows as an effect); `launchAtLoginChangeCompleted`
snaps it back to the re-read real status when the attempt fails,
so the toggle never lies for longer than the attempt takes.
Exactly one attempt may be in flight (`isLaunchAtLoginBusy`):
a re-toggle before the outcome lands is ignored, because two
concurrent register/unregister calls would race and the toggle
would settle on whichever completion arrived last.

Source: `macos/NoicanState/Sources/NoicanState/AppReducer.swift`, `AppReducer.launchAtLoginChangeCompleted`.

The registration attempt settled: show the *re-read* real status
(an optimistic toggle that failed snaps back here), surface the
failure reason under the toggle, and release the serialization
gate. Registration is inherently environment-dependent (app
location, signature), so the outcome is authoritative and the
request is not. A completion without a claimed attempt is stale
by construction and ignored.

Source: `macos/NoicanState/Sources/NoicanState/AppReducer.swift`, `AppReducer.launchAtLoginStatusRead`.

Seeds the login-item toggle from the `SMAppService` status read
at launch (the service is the source of truth; the app persists
nothing about it). Ignored while an attempt is in flight — its
completion re-reads the status and is the fresher answer.

## login-item-status-mirroring

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `AppModel.isLaunchAtLoginEnabled`.

Whether the app is registered as a login item, mirroring the
`SMAppService` status: seeded by `AppEvent.launchAtLoginStatusRead`
at launch, moved optimistically by a toggle, and snapped back to
the re-read real status when the registration attempt completes.

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `AppModel.isLaunchAtLoginBusy`.

Whether a login-item registration attempt is in flight. Serializes
toggles exactly like `EngineMachine.busy` serializes engine
transitions: a toggle while one is pending is ignored, so two
concurrent attempts can never race and settle the toggle on
whichever completion happens to arrive last.

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.setLaunchAtLogin`.

Login-item toggle binding setter. The reducer moves the toggle
optimistically; the completion event snaps it back to the re-read
`SMAppService` status when registration fails.

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.isLoginItemRegistered`.

Whether the login item counts as on for the toggle. Registered
but pending the user's consent (`requiresApproval` — common for
development builds and previously denied items) reads as on: the
registration itself succeeded and the remaining step lives in
System Settings, so snapping the toggle off would misreport a
successful attempt (and invite a pointless re-register loop).

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.applyLaunchAtLogin`.

Registers or unregisters the login item off the main actor
(registration can touch the service database). The completion
carries the *re-read* status: registration depends on the app's
location and signature (development builds under dist/ commonly
fail), so the outcome — not the request — is what the reducer
must show, together with the reason under the toggle: a failure,
or the pending-approval notice when macOS wants the user's
consent in System Settings.

## login-item-toggle-serialization

Source: `macos/NoicanState/Tests/NoicanStateTests/PreferencesReducerTests.swift`, `LaunchAtLoginTests.rapidTogglesAreSerialized`.

Flip on, then flip off before the registration settles: the
second flip must be ignored (two concurrent register/unregister
calls would race and the toggle would settle on whichever
completion arrived last).

Source: `macos/NoicanState/Tests/NoicanStateTests/PreferencesReducerTests.swift`, `LaunchAtLoginTests.rapidTogglesAreSerialized`.

A status read racing the attempt is stale and ignored too.

## loopback-output-preference

Source: `macos/Sources/NoicanMenuBar/CoreAudioDevices.swift`, `AudioDeviceCatalog.virtualOutput`.

The loopback output the engine feeds. When the Noican driver and
stock BlackHole 2ch are both installed, the Noican driver wins:
it is the device this project brands, signs, and tests against,
while stock BlackHole may be shared with (and reconfigured by)
other software. Previously the pick fell to device-enumeration
order, which is undefined (docs/driver.md, "Coexistence").

## main-actor-delivery-assumptions

Source: `macos/Sources/NoicanMenuBar/LicenseModel.swift`, `LicenseModel.init`.

Delivered on the main queue (see `start`), which is the
main actor.

Source: `macos/Sources/NoicanMenuBar/StatusBarController.swift`, `StatusBarController.init`.

@Published emits on the main actor (AppState is bound
to it); the annotation gap is bridged here.

## main-actor-detached-effects

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.startEngine`.

Detached: aggregate creation polls the device until it is alive
(up to ~1.5 s) and engine start may download weights — neither
may block the main actor. The busy machine state keeps this the
only operation touching `aggregate`/`engine` until it finishes.

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.switchModel`.

Detached: weight download and model construction must not run
on (or inherit) the main actor.

## main-queue-listener-delivery

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.registerDeviceListener`.

Delivered on the main queue, which is the main actor.

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.registerDefaultOutputListener`.

Delivered on the main queue, which is the main actor.

Source: `macos/Sources/NoicanMenuBar/ListenerObservation.swift`, `AppState.syncMonitorSafetyObservation`.

Delivered on the main queue, which is the main actor.

Source: `macos/Sources/NoicanMenuBar/ListenerObservation.swift`, `AppState.syncInputRateObservation`.

Delivered on the main queue, which is the main actor.

## message-clearing-on-teardown

Source: `macos/NoicanState/Sources/NoicanState/AppReducer.swift`, `AppReducer.startTransition`.

The teardown clears any live session; a model-switch message
would describe the torn-down engine, so it clears with it, and
the virtual-output level reading is re-taken once the new
transport is up (its poll stops with the old one).

## message-slot-clearing-rules

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `MessageSlots`.

The user-facing message slots, together so the clearing rules live
in one place: the reducer states, per event, which slots clear —
instead of scattering assignments across transition callbacks.

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `MessageSlots.previewError`.

Last preview failure (start failure, feedback trip, …), shown
under the mode control. Preview failures never affect the engine
phase: the meeting-facing path keeps running. Kept through a
Preview retry (settled-state rendering) and cleared only when an
enable actually succeeds or the user leaves Preview.

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `MessageSlots.previewUnavailableReason`.

Why the last Preview attempt was refused (unsafe default output),
shown under the mode control until the user fixes the output —
`monitorTargetErrorChanged` clears it live — or moves on. Nil
until Preview is actually pressed: an unavailable Preview stays
pressable and explains itself on press, which confuses less than
a segment that cannot be pressed.

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `MessageSlots.microphoneError`.

Why the last microphone selection was refused in place (the
device's rate lies outside what the capture resampler converts)
while the engine kept the previous one. Shown under the
microphone list; cleared on the next selection — including a
re-click of the already-selected microphone, which acknowledges
the message without touching the engine.

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `MessageSlots.modelError`.

Why the last model switch failed while the engine kept running the
previous model. Shown under the Model picker (this is not an
engine failure, so the phase — and the pill — stay green).
Cleared by the next pick and by any engine teardown (the message
would describe a torn-down engine).

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `MessageSlots.launchAtLoginError`.

Login-item outcome that needs the user's attention, shown under
the toggle: why the last registration attempt failed (development
builds outside /Applications commonly cannot register), or the
pending-approval notice when macOS wants consent in System
Settings. The completion event that carries it has already moved
the toggle to the re-read real status. Cleared by the next toggle
attempt and by a later clean completion.

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `MessageSlots.licenseRequired`.

Why the last Preview/On tap was refused: no valid license. Shown
under the mode control; cleared by the next tap and as soon as a
license allows processing.

## mode-control-feedback-placement

Source: `macos/Sources/NoicanMenuBar/MenuView.swift`, `MenuView.modePicker`.

The single top-level control. Preview = engine + self-monitor;
On = engine only. Both feed the virtual microphone. All prose
feedback — engine failures, refused Preview presses (cleared live
once the output is safe), preview failures and feedback trips, and
the virtual output's turned-down/muted notice — renders below the
control, where changing height cannot move it.

## mode-intent-and-retry

Source: `macos/NoicanState/Sources/NoicanState/AppReducer.swift`, `AppReducer.modeSelected`.

The mode control is intent: the system never changes it, only the
user does. Tapping the already-selected segment retries when the
intent is unfulfilled (e.g. the last start failed).

Source: `macos/NoicanState/Sources/NoicanState/AppReducer.swift`, `AppReducer.startCompleted`.

The mode keeps the user's intent; the red pill tint and the
error below the control say it is not running. Retry by
tapping the segment again or picking another microphone.

## mode-is-user-intent

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `AppModel.mode`.

The user's intent. Changed only by `AppEvent.modeSelected`.

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `AppModel.isModeUnfulfilled`.

True while the selected mode is not actually delivering (start
failure, runtime stop, or a preview whose monitor is not playing).
The mode control is the user's intent and never moves on its own;
this drives the warning tint that says "you asked for this, but it
is not running".

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.setMode`.

The mode control is intent: the reducer never moves it on its own
and re-tapping the selected segment retries an unfulfilled mode.

## model-pick-without-session

Decided 2026-11-27; recorded here because code comments are banned.

Picking another model while the mode is On or Preview and no
session is live (the last start failed, or a runtime stop tore the
engine down) starts the engine with that model, as picking another
microphone does. `EngineTransition.switchingModel` therefore always
carries the session it switches; a switch without one is not
representable. Before this the pick sent `AppEffect.switchModel` to
a stopped engine, which could only answer "engine is not running"
under the Model picker.

## no-persistent-headphone-warning

Source: `macos/Sources/NoicanMenuBar/MenuView.swift`, `MenuView.monitoring`.

Live observation of the running stream: input (pre-model) and
output (post-model) peak bars on a shared dB scale — speech moves
both; noise-only passages leave the output bar clearly below the
input bar, showing the suppression at a glance.

No persistent headphone warning: its job is done by code — unsafe
outputs are refused on press with the reason shown, and actual
feedback through an unclassifiable output trips the killswitch,
which also explains itself.

## preference-persistence-rules

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.PreferenceKey`.

`UserDefaults` keys for the persisted preferences. Only picker
state is persisted (microphone UID, model id, strength) — never
the mode: the app always launches Off so it never captures the
microphone without a user action.

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.init`.

Restore after the first device read so the reducer can vet the
stored microphone against the live list.

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.restorePreferences`.

Feeds the persisted preferences back through the reducer (the
restore is an event like everything else, so it stays testable).
The stored model id is validated against the live registry here —
the reducer has no model catalog.

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.persistPreferences`.

Persists preference fields the reduce step changed. Skipped for
`devicesChanged` (hot-plug reassignment of a vanished selection is
not a user choice — the stored microphone must survive being
temporarily unplugged across a relaunch) and for the restore
event itself. Reverts (a failed model switch or microphone
fallback) do persist: they record what actually runs.

## preferences-restore-gating

Source: `macos/NoicanState/Sources/NoicanState/AppReducer.swift`, `AppReducer.preferencesRestored`.

Applies persisted preferences at launch. Accepted only while the
user's intent is Off with no transition in flight — the state the
app launches in — so a late or replayed restore can never disturb
a session the user has since started. Selections flow through the
same fields the pickers use; the mode is never restored (the app
always starts Off, so launch never captures the microphone).

## preferences-restore-no-mode

Source: `macos/NoicanState/Sources/NoicanState/AppEvent.swift`, `AppEvent.preferencesRestored`.

Persisted preferences read back at launch. The shell validates
each value before dispatching (a stored model id must exist in
the registry and be startable), nil skips a field, and the
reducer additionally requires a stored microphone to be present
in the current device list. The mode is deliberately *not*
restored: the app always starts Off, so launching it never
captures the microphone (TCC prompts and live capture must
follow a user action, not app startup).

## reducer-effect-architecture

Source: `macos/NoicanState/Sources/NoicanState/AppEffect.swift`, `AppEffect`.

Side effects the reducer requests from the runtime shell. The reducer
decides *what* happens; the shell performs it against Core Audio and
the Rust engine and reports back with the matching completion event.

## reducer-event-sampling

Source: `macos/NoicanState/Sources/NoicanState/AppEvent.swift`, `AppEvent`.

Everything that can happen to the app, as one input stream to the
reducer: user intents, completions of the effects the reducer itself
requested, and environment observations (device hot-plug, health-poll
findings, the feedback killswitch).

Events carry the environment facts the pure reducer cannot read
(Core Audio pre-flights, live engine liveness): the runtime shell
samples them at dispatch time, which keeps `AppReducer.reduce` a pure
function without hiding those reads from the transition logic.

## reducer-shell-boundary

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState`.

The runtime shell around the pure state machine in the `NoicanState`
package: it samples the environment (Core Audio device lists,
pre-flight checks, engine health), feeds everything into
`AppReducer.reduce` as events, performs the effects the reducer
requests against the aggregate and the Rust engine, and publishes the
resulting `AppModel` for SwiftUI. All transition *decisions* live in
the reducer; nothing here changes `model` except `dispatch(_:)`.

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.model`.

The reducer state, replaced wholesale by `dispatch(_:)`. The UI
renders its projections (`statusText`, `phase`, message slots, …),
which are all derived from settled snapshots.

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.allDevices`.

Full Core Audio device snapshot (the reducer sees the pure
`InputDevice` projection; effects need the identifiers here).
Internal for the extensions in sibling files (the virtual-output
level check); written only by `refreshDevices()`.

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.dispatch`.

The single writer of `model`: reduce, publish, persist changed
preferences, perform effects, then re-derive the pollers from the
new state. Internal (not private) so the extensions in sibling
files can dispatch their observations through the same path.

## reducer-state-architecture

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `AppModel`.

The whole reducer state. The engine lifecycle is the single
`EngineMachine` enum; the fields around it are the pieces that are
orthogonal to it by design: `mode` is the user's intent (the system
never moves it, so it cannot live inside the machine), the selections
are picker state that persists across engine lifecycles, and
`messages` are display slots with their own declarative clearing
rules.

## reducer-test-support-sharing

Source: `macos/NoicanState/Tests/NoicanStateTests/AppReducerTests.swift`, `StartStopTests`.

Fixtures and drivers are shared with the other reducer suites; see
ReducerTestSupport.swift.

## sample-rate-change-rebuild

Source: `macos/NoicanState/Sources/NoicanState/AppReducer.swift`, `AppReducer.inputSampleRateChanged`.

The running microphone's nominal rate changed under the transport
(Bluetooth A2DP ↔ HFP renegotiation): the transport captures at
the rate fixed at start time, so rebuild it into the current mode
around the new rate — the same automatic recovery a live
microphone switch performs. The shell pre-filters spurious
notifications (same rate) and the busy machine serializes
overlapping rebuilds, so this cannot storm.

## sample-rate-request-async

Source: `macos/Sources/NoicanMenuBar/CoreAudioDevices.swift`, `AudioDeviceCatalog.requestSampleRate`.

Requests a nominal sample rate; the change lands asynchronously, so
callers must poll [`nominalSampleRate`] for the result.

## settled-state-rendering

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `EngineSession.isMonitorArmed`.

Whether the preview self-monitor is armed and playing. Updated
only when a monitor transition settles — never optimistically —
so every projection reading it renders settled state.

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `EnginePhase`.

Settled engine lifecycle: the last known outcome, never a transition.
In-flight transitions are expressed solely by `EngineMachine.busy`
(the spinner), so every surface rendered from this moves exactly once
per transition, settled state to settled state.

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `EngineMachine`.

The single engine state: either settled, or one serialized transition
in flight. While busy, the UI keeps rendering `rendering` — the last
settled phase — and the spinner is the only transitional feedback, so
a transition that fails quickly never flashes optimistic UI.

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `AppModel.phase`.

The settled phase the UI renders: the machine's settled outcome,
or — while busy — the snapshot taken when the transition was
claimed. Everything rendered from this (sections, colors, error
text) changes once per transition, when the outcome is known.

Source: `macos/NoicanState/Sources/NoicanState/AppReducer.swift`, `AppReducer.modeSelected`.

Keep a preview failure visible while a Preview retry is in
flight (settled-state rendering: only a settled enable success
clears it, in `monitorChangeCompleted`); leaving Preview clears
it now.

Source: `macos/NoicanState/Sources/NoicanState/AppReducer.swift`, `AppReducer.monitorChangeCompleted`.

The engine itself keeps running either way; the mode keeps the
user's intent. On failure the pill's warning tint plus the
message below the control say the preview is not playing, and
re-tapping Preview retries. Settled-state rendering: a kept
preview failure clears only when an enable actually succeeds
(a successful disable after a feedback trip must not erase the
trip's explanation).

Source: `macos/NoicanState/Sources/NoicanState/AppReducer.swift`, `AppReducer.modelSwitchCompleted`.

Settle back to exactly the rendered snapshot: on a healthy
engine the switch failure is not an engine failure (previous
model keeps running, the phase stays running), and on a
stopped one (the switch fails fast) painting running would be
a green lie. The reason renders under the Model picker, with
the picker reverted to stay truthful.

## stale-failure-clearing

Source: `macos/NoicanState/Sources/NoicanState/AppReducer.swift`, `AppReducer.modelSelected`.

Changing the model while stopped starts nothing; clear a stale
failure message so the menu does not keep blaming the last
attempt.

Source: `macos/NoicanState/Sources/NoicanState/AppReducer.swift`, `AppReducer.microphoneSelected`.

Changing the microphone while Off only updates the selection;
clear a stale failure so the menu stops blaming the previous
device the moment another one is chosen.

## start-attempt-fallback

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `StartAttempt`.

One engine start as claimed by the reducer: everything the runtime
shell needs to build the transport, plus the one-shot fallback target
for failed live microphone switches.

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `StartAttempt.revertInputUID`.

Microphone to fall back to when this start fails (the device that
was working a moment ago, for live switches). One attempt only:
the fallback start carries no further revert target.

## start-completion-ordering

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.finishStart`.

Completion entry point for start effects: records the capture
rate the split transport was built around (nil on the aggregate
path or on failure) as the baseline for the nominal-rate
listener, then dispatches the outcome and reads the virtual
output's level controls once right away (the health poll's first
tick is a second out).

## start-preflight-ordering

Source: `macos/NoicanState/Sources/NoicanState/AppReducer.swift`, `AppReducer.startTransition`.

Claims a start: tears any previous transport down, pre-flights
everything that can be checked synchronously (an incapable
microphone fails here, before any busy round-trip, so the reason
appears without the UI flashing transitional state), then goes
busy with the attempt.

## stop-is-immediate-and-infallible

Source: `macos/NoicanState/Sources/NoicanState/AppReducer.swift`, `AppReducer.stopTransition`.

Off: tear the transport down synchronously. No busy state — a
stop is immediate and cannot fail.

## string-write-infallible

Source: `crates/noican-cli/src/eval_run.rs`, `render_csv`.

Writing to a String cannot fail.

## telephony-profile-notice-style

Source: `macos/Sources/NoicanMenuBar/MenuView.swift`, `MenuView.settings`.

Telephony-profile (Bluetooth headset) selections are a
supported path with an inherent quality trade-off; the
secondary style says "property of the device", not
"error".

## telephony-rate-classification

Source: `macos/NoicanState/Sources/NoicanState/InputDevice.swift`, `CaptureSupport.isTelephonyRate`.

Whether a native rate is a Bluetooth telephony profile — the
4 kHz-multiple speech rates up to 32 kHz (CVSD 8 kHz, mSBC
16 kHz, LC3 24/32 kHz, and the 12 kHz divisor the split path has
always listed) — which means narrow-band capture with the
headset-wide playback trade-off. The 44.1 kHz family (44.1,
22.05, 11.025 kHz) is not telephony even where it lies below
24 kHz: those are CD-derived rates, and the notice must describe
only the conversion. Drives the selection notice.

Source: `macos/NoicanState/Tests/NoicanStateTests/NativeRateReducerTests.swift`, `NativeRateSelectionTests.telephonyNotice`.

A 44.1 kHz device is full-band: the notice explains the
conversion and must not call the audio narrow-band.

## test-cleanup-best-effort

Source: `crates/noican-cli/src/eval_run.rs`, `tests::TempDir::drop`.

Best effort: a leftover temp dir is not a test failure.

## test-double-scripting-semantics

Source: `macos/NoicanLicensing/Tests/NoicanLicensingTests/TestSupport.swift`, `MockBackend`.

Scripted backend: each operation pops its next result (or repeats
the last one) and records the call.

## virtual-output-48k

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.startEngine`.

The virtual output feeds consumers and must stay at
48 kHz; the aggregate path switches it inside
`create`, the split path must do it explicitly.

## virtual-output-level-checking

Source: `macos/NoicanState/Tests/NoicanStateTests/VirtualOutputLevelTests.swift`, `VirtualOutputLevelReducerTests.scalarMovesAreRecorded`.

The shell dedupes on the whole reading so every move reaches
the log; the model must therefore keep the scalar, not just
the (identical) notice text.

Source: `macos/NoicanState/Tests/NoicanStateTests/VirtualOutputLevelTests.swift`, `VirtualOutputLevelReducerTests.acceptedWhileTransportBusy`.

Preview start: the transport is up while the monitor half is
still settling, and the start-completion check lands here.

Source: `macos/NoicanState/Tests/NoicanStateTests/VirtualOutputLevelTests.swift`, `VirtualOutputLevelReducerTests.keptThroughFault`.

The poll keeps running on the faulted-but-live transport, so a
nominal reading still clears it.

Source: `macos/Sources/NoicanMenuBar/VirtualOutputLevelCheck.swift`, `AppState.checkVirtualOutputLevel`.

Reads the virtual output's level controls, hands the reading to the
diagnostics log (which writes one line per distinct reading, with
the scalar, across transport rebuilds), and forwards a reading
that differs from the model's to the reducer. The two change keys
are separate on purpose: the model resets to nominal on every
teardown so the notice is re-established for a new transport,
while the log must only record slider moves that actually
happened. Called when an engine start settles and by the 1 Hz
health poll (ahead of its busy guard, since this is a device
property read, not an engine call); the reducer accepts it only
while a transport is up, so a reading while Off is dropped before
it can churn the model. The device read is the Noican virtual
output itself (the same pick the aggregate is composed around),
never the private aggregate.

## virtual-output-level-detection

Source: `macos/Sources/NoicanMenuBar/EngineDiagnostics.swift`, `EngineDiagnostics.recordVirtualOutputLevel`.

Records a reading of the virtual output device's own volume/mute
controls: one line per *distinct* reading (a turned-down slider
logs its scalar again whenever it moves; the return to unity logs
once), nothing while the reading is unchanged. Detection only —
the level is never written (see `VirtualOutputLevel`).

Source: `macos/Sources/NoicanMenuBar/EngineDiagnostics.swift`, `EngineDiagnostics.recordVirtualOutputLevel`.

Percent first (the slider position as the user sees it),
scalar second for exact comparison between runs. Truncated,
not rounded: a reading just under the unity threshold must
not print "100%" next to "below unity".

## virtual-output-level-notice

Source: `macos/NoicanState/Sources/NoicanState/AppEvent.swift`, `AppEvent.virtualOutputLevelObserved`.

The shell read the Noican virtual output device's own volume and
mute controls (at start completion and on every health-poll tick)
and classified them. The reducer only records the reading (the
notice under the mode control is its projection); it never asks
for the level to be changed (see `VirtualOutputLevel`).

Source: `macos/NoicanState/Sources/NoicanState/AppReducer.swift`, `AppReducer.virtualOutputLevelObserved`.

The shell's reading of the Noican virtual output device's own
volume/mute controls: record it — the notice under the mode
control is its projection, shown while the device is turned down
or muted and gone as soon as a reading is nominal — and nothing
else. Deliberately no effect: restoring the level would fight
whichever app or user set it and remove the user's own adjustment
(`VirtualOutputLevel`). Accepted only while a transport is up
(including through busy monitor/model transitions — the device is
live regardless), because the poll that keeps the reading
truthful only runs then; teardown paths reset it themselves.

Source: `macos/Sources/NoicanMenuBar/MenuView.swift`, `MenuView.modePicker`.

The virtual output device's own volume/mute (System
Settings › Sound › Input): the engine runs fine, consumers
just hear it quietly — a warning, not an engine failure, so
orange rather than red, and never acted on automatically.

## virtual-output-level-observation

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `AppModel.virtualOutputLevel`.

The last classified reading of the Noican virtual output device's
own volume/mute controls (`VirtualOutputLevel`), as observed by
the shell while a transport is live. Kept as the full reading —
not just its notice text — so the shell can tell one slider
position from the next and log every distinct move with its
scalar, while the popover renders the projection
`virtualOutputLevelNotice`. Maintained by
`AppEvent.virtualOutputLevelObserved`; reset to `.nominal` by
every engine teardown, because the poll that keeps it truthful
only runs while a transport is live. Never acted on
automatically: the level is the user's (or another app's) setting
to change.

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `AppModel.virtualOutputLevelNotice`.

One-line notice under the mode control when the Noican virtual
output device's own volume is turned down or its mute is on
(consumers hear the processed voice quietly or not at all), or nil.
A projection of `virtualOutputLevel` in the message slots' voice —
a warning about the environment, not an engine failure, so it
leaves the phase, the pill tint, and `isModeUnfulfilled` alone.

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.checkEngineHealth`.

Ahead of the busy guard: the virtual output's level controls are
a device property read, not an engine call, and the reducer
accepts the reading through monitor/model transitions.

## virtual-output-level-report-only

Source: `macos/NoicanState/Sources/NoicanState/VirtualOutputLevel.swift`, `VirtualOutputLevel`.

What the Noican virtual output device's own volume and mute controls
say about the level consumers will receive, classified from the raw
Core Audio readings by the shell.

The BlackHole-derived driver registers a volume control (one master
value, −64…0 dB, applied to every sample it loops) and a mute control,
both exposed in System Settings › Sound › Input as the "Noican
Microphone" slider. A 2026-09-05 hardware investigation traced "Noican
sounds quiet in meetings" to that slider sitting at about −35 dB —
nothing in the engine or the transport was attenuating. Who moved it
is unknown (the user, or a meeting app's "automatically adjust
microphone volume" writing the selected input device's system volume),
which is why the reducer only *reports* the condition and never
restores the level: writing it back would fight such an app and take
away the user's own adjustment.

Devices without these controls (or whose controls cannot be read) are
classified as `.nominal` — fail open: no message is better than a
message about a control that does not exist.

Source: `macos/NoicanState/Sources/NoicanState/VirtualOutputLevel.swift`, `VirtualOutputLevel.turnedDown`.

The volume control is below unity; `scalar` is the Core Audio
scalar reading (0…1), kept for the log rather than the UI.

Source: `macos/NoicanState/Sources/NoicanState/VirtualOutputLevel.swift`, `VirtualOutputLevel.muted`.

The mute control is on. Takes precedence over a low volume: muted
is the more actionable finding.

Source: `macos/NoicanState/Sources/NoicanState/VirtualOutputLevel.swift`, `VirtualOutputLevel.classify`.

Classifies the raw readings. `volumeScalar` and `isMuted` are nil
when the corresponding control is absent or unreadable, which
never produces a finding on its own.

Source: `macos/NoicanState/Sources/NoicanState/VirtualOutputLevel.swift`, `VirtualOutputLevel.notice`.

The one-line notice for the popover, or nil when nothing needs
saying. Always a single sentence naming the place to fix it, in
the voice of the other message slots.

Source: `macos/Sources/NoicanMenuBar/VirtualOutputLevelCheck.swift`, `VirtualOutputLevelProbe`.

Reads the Noican virtual output device's own volume and mute controls.

The BlackHole-derived driver applies one master volume (−64…0 dB) and
one mute to every sample it loops, and macOS shows them as the
"Noican Microphone" input slider in System Settings › Sound › Input. A
slider left low there makes every consumer hear the processed voice
quietly, with nothing in Noican's own meters or diagnostics showing
it (2026-09-05 hardware investigation: about −35 dB). This probe is
read-only by design — see `VirtualOutputLevel` for why the level is
reported, never restored.

The controls are read on the input scope first (the slider the user
sees for a microphone), falling back to the output scope: BlackHole
registers the same single value on both, and a future output-only
virtual device would have only the latter. A control that is absent
on both scopes reads as nil, which classifies as nominal (fail open).

## virtual-output-unity-threshold

Source: `macos/NoicanState/Sources/NoicanState/VirtualOutputLevel.swift`, `VirtualOutputLevel.unityThreshold`.

Scalar readings at or above this count as unity. Core Audio
reports the slider position as a `Float32`, and a slider parked
at the top can read a hair below 1.0 depending on the driver's
dB → scalar conversion; anything closer than this is not a level
the user (or an app) turned down.
