/// Everything that can happen to the app, as one input stream to the
/// reducer: user intents, completions of the effects the reducer itself
/// requested, and environment observations (device hot-plug, health-poll
/// findings, the feedback killswitch).
///
/// Events carry the environment facts the pure reducer cannot read
/// (Core Audio pre-flights, live engine liveness): the runtime shell
/// samples them at dispatch time, which keeps `AppReducer.reduce` a pure
/// function without hiding those reads from the transition logic.
public enum AppEvent: Hashable, Sendable {
    // MARK: User intents

    /// A mode-control segment was tapped. `monitorTargetError` is the
    /// preview-target pre-flight (nil when the default output may receive
    /// the preview; only sampled for Preview taps), and `isEngineRunning`
    /// is the engine's live answer — the machine's belief can lag it by
    /// up to one health-poll tick, and a tap in that window must rebuild
    /// instead of toggling the monitor on a dead transport.
    case modeSelected(EngineMode, monitorTargetError: String?, isEngineRunning: Bool)
    /// A model was picked in the Model picker.
    case modelSelected(String)
    /// A microphone was clicked in the list.
    case microphoneSelected(String)
    /// The strength slider moved (0 = raw microphone, 1 = fully
    /// processed). Applying it is a lock-free atomic write on the engine
    /// side, so this claims no transition and is accepted even while
    /// busy — gating it would make the slider feel dead during a model
    /// download.
    case intensityChanged(Double)
    /// The login-item toggle was flipped. Optimistic: the toggle moves
    /// immediately and `launchAtLoginChangeCompleted` snaps it back to
    /// the real status when registration fails. Serialized: at most one
    /// registration attempt is in flight, and a flip while one is
    /// pending is ignored (concurrent attempts would race).
    case launchAtLoginToggled(Bool)

    // MARK: Effect completions

    /// `AppEffect.startEngine` finished (nil error = the transport is up).
    case startCompleted(error: String?)
    /// `AppEffect.setMonitor` finished. Which direction was requested is
    /// already recorded in the in-flight `EngineTransition.settingMonitor`.
    case monitorChangeCompleted(error: String?)
    /// `AppEffect.switchModel` finished. The target model is recorded in
    /// the in-flight `EngineTransition.switchingModel`.
    case modelSwitchCompleted(error: String?)
    /// `AppEffect.setLaunchAtLogin` finished. `isEnabled` is the real
    /// `SMAppService` status re-read after the attempt (registration
    /// depends on the app's location and signature, so the outcome —
    /// not the request — is what the toggle must show; registered but
    /// pending the user's approval counts as on), and `error` renders
    /// under the toggle when something needs the user's attention — a
    /// failure reason or the pending-approval notice.
    case launchAtLoginChangeCompleted(isEnabled: Bool, error: String?)

    // MARK: Environment observations

    /// Persisted preferences read back at launch. The shell validates
    /// each value before dispatching (a stored model id must exist in
    /// the registry and be startable), nil skips a field, and the
    /// reducer additionally requires a stored microphone to be present
    /// in the current device list. The mode is deliberately *not*
    /// restored: the app always starts Off, so launching it never
    /// captures the microphone (TCC prompts and live capture must
    /// follow a user action, not app startup).
    case preferencesRestored(modelID: String?, inputUID: String?, intensity: Double?)
    /// The `SMAppService` login-item status read at launch (the service
    /// itself is the source of truth; nothing about it is persisted by
    /// the app).
    case launchAtLoginStatusRead(isEnabled: Bool)
    /// The device list changed (hot-plug) or was first read. `inputs` is
    /// the selectable-microphone snapshot; `allInputUIDs` additionally
    /// contains every device with input channels (the transport-loss
    /// check must not depend on the selectability filter); the virtual
    /// output presence is a start pre-flight.
    case devicesChanged(inputs: [InputDevice], allInputUIDs: Set<String>, isVirtualOutputPresent: Bool)
    /// Re-sample of the preview-target pre-flight while a refusal message
    /// is showing, so the message clears (or updates) live as the user
    /// fixes the default output.
    case monitorTargetErrorChanged(String?)
    /// The running microphone renegotiated its nominal sample rate
    /// after the transport was built around the old one (Bluetooth
    /// headsets flip between A2DP and HFP profiles). The shell only
    /// dispatches this after confirming the rate actually differs from
    /// the rate the transport captures at, so the reducer's job is to
    /// rebuild the transport into the current mode.
    case inputSampleRateChanged
    /// The engine-side feedback killswitch tripped: the worker already
    /// silenced the preview; the monitor device must be released and the
    /// user told why.
    case monitorTripped
    /// The device the playing monitor targets lost its safety after
    /// enable time (the headphone jack flipped onto the internal
    /// speakers the user did not choose, or the device disappeared):
    /// the preview must stop itself before the unvetted output keeps
    /// playing. A preview deliberately started on the speakers never
    /// produces this — the shell's flip check compares against the
    /// enable-time choice. `reason` explains what happened, in the same
    /// voice as the enable-time refusals.
    case monitorTargetBecameUnsafe(reason: String)
    /// The health poll found the engine faulted (audio callback,
    /// workgroup, or inference fault). The transport stays up; the next
    /// user action tears it down.
    case engineFaulted
    /// The health poll found the engine no longer running.
    case engineStoppedUnexpectedly
    /// The health poll saw no frames for three consecutive seconds while
    /// running (unplugged mic, coreaudiod restart, post-sleep stall).
    case audioStalled
    /// Reading the device list itself failed.
    case deviceQueryFailed(String)
}
