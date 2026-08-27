/// Side effects the reducer requests from the runtime shell. The reducer
/// decides *what* happens; the shell performs it against Core Audio and
/// the Rust engine and reports back with the matching completion event.
public enum AppEffect: Hashable, Sendable {
    /// Tear the transport down (engine stop + aggregate destroy).
    /// Synchronous and idempotent; has no completion event.
    case stopEngine
    /// Build the private aggregate around the attempt's microphone and
    /// start the engine (asynchronous: aggregate creation polls the
    /// device until alive and the engine start may download weights).
    /// Completes with `AppEvent.startCompleted`.
    case startEngine(StartAttempt)
    /// Toggle the preview self-monitor on the live transport
    /// (asynchronous: starting an output device can take a moment).
    /// Completes with `AppEvent.monitorChangeCompleted`.
    case setMonitor(enabled: Bool)
    /// Prepare and lock-free publish a replacement model (asynchronous:
    /// weight download and model construction). Completes with
    /// `AppEvent.modelSwitchCompleted`.
    case switchModel(to: String)
}
