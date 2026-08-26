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

    // MARK: Effect completions

    /// `AppEffect.startEngine` finished (nil error = the transport is up).
    case startCompleted(error: String?)
    /// `AppEffect.setMonitor` finished. Which direction was requested is
    /// already recorded in the in-flight `EngineTransition.settingMonitor`.
    case monitorChangeCompleted(error: String?)
    /// `AppEffect.switchModel` finished. The target model is recorded in
    /// the in-flight `EngineTransition.switchingModel`.
    case modelSwitchCompleted(error: String?)

    // MARK: Environment observations

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
    /// The engine-side feedback killswitch tripped: the worker already
    /// silenced the preview; the monitor device must be released and the
    /// user told why.
    case monitorTripped
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
