/// The live transport as the state machine knows it: which model and
/// microphone the engine was built around, and whether the preview
/// monitor tee is armed. Carried by every state that has (or renders) a
/// running engine, so nothing about the transport is tracked in loose
/// fields.
public struct EngineSession: Hashable, Sendable {
    /// Model the running transport was started/switched to.
    public var modelID: String
    /// Microphone the running transport was built around (the aggregate
    /// is composed at start time, so a live change means a rebuild).
    public var inputUID: String
    /// Whether the preview self-monitor is armed and playing. Updated
    /// only when a monitor transition settles — never optimistically —
    /// so every projection reading it renders settled state.
    public var isMonitorArmed: Bool

    public init(modelID: String, inputUID: String, isMonitorArmed: Bool = false) {
        self.modelID = modelID
        self.inputUID = inputUID
        self.isMonitorArmed = isMonitorArmed
    }
}

/// Settled engine lifecycle: the last known outcome, never a transition.
/// In-flight transitions are expressed solely by `EngineMachine.busy`
/// (the spinner), so every surface rendered from this moves exactly once
/// per transition, settled state to settled state.
public enum EnginePhase: Hashable, Sendable {
    /// Engine torn down by the user's choice.
    case off
    /// Engine running; `session` describes the live transport.
    case running(EngineSession)
    /// The last attempt failed. `session` is non-nil when the transport
    /// is still up despite the failure (an engine fault reports failure
    /// without tearing the transport down; the next user action does).
    case failed(String, session: EngineSession?)
}

/// One engine start as claimed by the reducer: everything the runtime
/// shell needs to build the transport, plus the one-shot fallback target
/// for failed live microphone switches.
public struct StartAttempt: Hashable, Sendable {
    /// Model to start with.
    public var modelID: String
    /// Microphone to compose the aggregate around. Captured at claim
    /// time: the *selection* can be reassigned by device hot-plug during
    /// the busy window, but the transport is bound to this device.
    public var inputUID: String
    /// Whether to arm the preview monitor once the transport is up.
    public var monitor: Bool
    /// Microphone to fall back to when this start fails (the device that
    /// was working a moment ago, for live switches). One attempt only:
    /// the fallback start carries no further revert target.
    public var revertInputUID: String?

    public init(modelID: String, inputUID: String, monitor: Bool, revertInputUID: String? = nil) {
        self.modelID = modelID
        self.inputUID = inputUID
        self.monitor = monitor
        self.revertInputUID = revertInputUID
    }
}

/// The one transition that may be in flight. Exactly one exists at a
/// time (`EngineMachine.busy`), which is what serialized every engine
/// operation behind the old `isBusy` flag — now by construction.
public enum EngineTransition: Hashable, Sendable {
    /// Aggregate + engine start (possibly followed by a monitor arm).
    case starting(StartAttempt)
    /// Preview monitor toggle on a live transport.
    case settingMonitor(enabled: Bool, session: EngineSession)
    /// Model switch. `session` is nil when no live transport is known
    /// (the switch then fails fast in the engine with a clear reason,
    /// which renders under the Model picker like any other switch
    /// failure).
    case switchingModel(to: String, session: EngineSession?)
}

/// The single engine state: either settled, or one serialized transition
/// in flight. While busy, the UI keeps rendering `rendering` — the last
/// settled phase — and the spinner is the only transitional feedback, so
/// a transition that fails quickly never flashes optimistic UI.
public enum EngineMachine: Hashable, Sendable {
    /// No transition in flight; `EnginePhase` is the settled outcome.
    case settled(EnginePhase)
    /// One transition in flight. Events that would start another one are
    /// ignored until this settles (the old `isBusy` serialization).
    case busy(EngineTransition, rendering: EnginePhase)
}

/// The user-facing message slots, together so the clearing rules live
/// in one place: the reducer states, per event, which slots clear —
/// instead of scattering assignments across transition callbacks.
public struct MessageSlots: Hashable, Sendable {
    /// Last preview failure (start failure, feedback trip, …), shown
    /// under the mode control. Preview failures never affect the engine
    /// phase: the meeting-facing path keeps running. Kept through a
    /// Preview retry (settled-state rendering) and cleared only when an
    /// enable actually succeeds or the user leaves Preview.
    public var previewError: String?
    /// Why the last Preview attempt was refused (unsafe default output),
    /// shown under the mode control until the user fixes the output —
    /// `monitorTargetErrorChanged` clears it live — or moves on. Nil
    /// until Preview is actually pressed: an unavailable Preview stays
    /// pressable and explains itself on press, which confuses less than
    /// a segment that cannot be pressed.
    public var previewUnavailableReason: String?
    /// Why the last microphone selection was refused in place (the
    /// device's rate lies outside what the capture resampler converts)
    /// while the engine kept the previous one. Shown under the
    /// microphone list; cleared on the next selection — including a
    /// re-click of the already-selected microphone, which acknowledges
    /// the message without touching the engine.
    public var microphoneError: String?
    /// Why the last model switch failed while the engine kept running the
    /// previous model. Shown under the Model picker (this is not an
    /// engine failure, so the phase — and the pill — stay green).
    /// Cleared by the next pick and by any engine teardown (the message
    /// would describe a torn-down engine).
    public var modelError: String?
    /// Login-item outcome that needs the user's attention, shown under
    /// the toggle: why the last registration attempt failed (development
    /// builds outside /Applications commonly cannot register), or the
    /// pending-approval notice when macOS wants consent in System
    /// Settings. The completion event that carries it has already moved
    /// the toggle to the re-read real status. Cleared by the next toggle
    /// attempt and by a later clean completion.
    public var launchAtLoginError: String?
    /// The Noican virtual output device's own volume control is turned
    /// down, or its mute is on, so consumers hear the processed voice
    /// quietly or not at all (`VirtualOutputLevel.notice`). Shown under
    /// the mode control while a transport is live; maintained by
    /// `virtualOutputLevelObserved` (set while the condition holds,
    /// cleared the moment a reading is nominal) and cleared by any
    /// engine teardown — the poll that keeps it truthful only runs while
    /// a transport is live. Never acted on automatically: the level is
    /// the user's (or another app's) setting to change.
    public var virtualOutputLevelNotice: String?

    public init(
        previewError: String? = nil,
        previewUnavailableReason: String? = nil,
        microphoneError: String? = nil,
        modelError: String? = nil,
        launchAtLoginError: String? = nil,
        virtualOutputLevelNotice: String? = nil
    ) {
        self.previewError = previewError
        self.previewUnavailableReason = previewUnavailableReason
        self.microphoneError = microphoneError
        self.modelError = modelError
        self.launchAtLoginError = launchAtLoginError
        self.virtualOutputLevelNotice = virtualOutputLevelNotice
    }
}

/// The whole reducer state. The engine lifecycle is the single
/// `EngineMachine` enum; the fields around it are the pieces that are
/// orthogonal to it by design: `mode` is the user's intent (the system
/// never moves it, so it cannot live inside the machine), the selections
/// are picker state that persists across engine lifecycles, and
/// `messages` are display slots with their own declarative clearing
/// rules.
public struct AppModel: Hashable, Sendable {
    /// The user's intent. Changed only by `AppEvent.modeSelected`.
    public var mode: EngineMode
    /// The engine lifecycle state machine.
    public var machine: EngineMachine
    /// Model chosen in the picker (reverted by the reducer when a switch
    /// fails, so the picker never lies about what is running).
    public var selectedModelID: String
    /// Microphone chosen in the list (reassigned when the device
    /// disappears, restored when a refused/failed switch keeps the
    /// previous one).
    public var selectedInputUID: String
    /// Selectable microphones, snapshotted by `AppEvent.devicesChanged`.
    public var inputDevices: [InputDevice]
    /// Whether the Noican/BlackHole loopback output exists, snapshotted
    /// by `AppEvent.devicesChanged` (a start pre-flight).
    public var isVirtualOutputPresent: Bool
    /// Dry/wet strength (0 = raw microphone, 1 = fully processed;
    /// default 1). Slider state like the pickers — it persists across
    /// engine lifecycles and applying it is a lock-free atomic write,
    /// so changes never claim an engine transition.
    public var intensity: Double
    /// Whether the app is registered as a login item, mirroring the
    /// `SMAppService` status: seeded by `AppEvent.launchAtLoginStatusRead`
    /// at launch, moved optimistically by a toggle, and snapped back to
    /// the re-read real status when the registration attempt completes.
    public var isLaunchAtLoginEnabled: Bool
    /// Whether a login-item registration attempt is in flight. Serializes
    /// toggles exactly like `EngineMachine.busy` serializes engine
    /// transitions: a toggle while one is pending is ignored, so two
    /// concurrent attempts can never race and settle the toggle on
    /// whichever completion happens to arrive last.
    public var isLaunchAtLoginBusy: Bool
    /// The user-facing message slots.
    public var messages: MessageSlots
    /// Whether the Rust engine handle was created. When false, mode and
    /// selection changes still update the pickers but never claim engine
    /// transitions.
    public var isEngineAvailable: Bool

    public init(
        mode: EngineMode = .off,
        machine: EngineMachine = .settled(.off),
        selectedModelID: String = "",
        selectedInputUID: String = "",
        inputDevices: [InputDevice] = [],
        isVirtualOutputPresent: Bool = false,
        intensity: Double = 1.0,
        isLaunchAtLoginEnabled: Bool = false,
        isLaunchAtLoginBusy: Bool = false,
        messages: MessageSlots = MessageSlots(),
        isEngineAvailable: Bool = true
    ) {
        self.mode = mode
        self.machine = machine
        self.selectedModelID = selectedModelID
        self.selectedInputUID = selectedInputUID
        self.inputDevices = inputDevices
        self.isVirtualOutputPresent = isVirtualOutputPresent
        self.intensity = intensity
        self.isLaunchAtLoginEnabled = isLaunchAtLoginEnabled
        self.isLaunchAtLoginBusy = isLaunchAtLoginBusy
        self.messages = messages
        self.isEngineAvailable = isEngineAvailable
    }
}

// MARK: - State accessors

extension AppModel {
    /// Whether a transition is in flight (drives the spinner and gates
    /// every new transition).
    public var isBusy: Bool {
        if case .busy = machine {
            return true
        }
        return false
    }

    /// The settled phase the UI renders: the machine's settled outcome,
    /// or — while busy — the snapshot taken when the transition was
    /// claimed. Everything rendered from this (sections, colors, error
    /// text) changes once per transition, when the outcome is known.
    public var phase: EnginePhase {
        switch machine {
        case let .settled(phase):
            phase
        case let .busy(_, rendering):
            rendering
        }
    }

    /// The live transport in a settled state: running, or failed with the
    /// transport still up (an engine fault). Nil while busy — the busy
    /// transition owns its session and no new transition may start.
    public var liveSession: EngineSession? {
        switch machine {
        case let .settled(.running(session)):
            session
        case let .settled(.failed(_, session)):
            session
        case .settled(.off), .busy:
            nil
        }
    }

    /// The settled, healthy transport (used by mode switches that only
    /// need to toggle the monitor half).
    public var liveRunningSession: EngineSession? {
        if case let .settled(.running(session)) = machine {
            return session
        }
        return nil
    }

    /// Whether an engine transport exists that the health poll must
    /// watch. True through busy monitor/model transitions (the transport
    /// stays up) and through a fault (failed with a live session); false
    /// while off, torn down, or rebuilding (`starting` claims a fresh
    /// transport whose watch begins on success).
    public var hasLiveTransport: Bool {
        transportSession != nil
    }

    /// The session whose engine transport is currently up, *including*
    /// through busy monitor/model transitions (which keep the transport
    /// running) and through a fault that left it up. This is what
    /// per-device observers must key on — the input-rate listener keyed
    /// on `liveSession` would detach for the length of every Preview
    /// toggle or model switch and miss a profile flip in that window.
    /// Nil while off, torn down, or rebuilding (`starting` claims a
    /// fresh transport).
    public var transportSession: EngineSession? {
        switch machine {
        case let .settled(.running(session)), let .busy(.settingMonitor(_, session), _):
            return session
        case let .settled(.failed(_, session)):
            return session
        case let .busy(.switchingModel(_, session), _):
            return session
        case .settled(.off), .busy(.starting, _):
            return nil
        }
    }
}

// MARK: - Projections for the menu UI

extension AppModel {
    /// One-line status for the header. The whole header is a projection
    /// of the settled snapshot (`phase`) — including whether the preview
    /// monitor is actually playing — so transitions do not churn this
    /// text (the spinner is the busy feedback). Deliberately never
    /// multi-line: text above the mode control must not change the
    /// control's vertical position (the sliding pill would move
    /// mid-animation). Full failure text lives in `engineErrorMessage`,
    /// shown below the control instead.
    public var statusText: String {
        switch phase {
        case .off:
            inputDevices.isEmpty ? "No input device" : "Off"
        case let .running(session):
            // No model name here: the Model picker below already shows it
            // (and reverts on failed switches, so it never lies).
            // "Previewing" only while the monitor settled as playing;
            // after a trip or a monitor failure the engine still runs but
            // the preview does not.
            session.isMonitorArmed ? "Previewing" : "Running"
        case .failed:
            "Error"
        }
    }

    /// Full engine failure text, displayed under the mode control (with
    /// the preview messages) so the header height stays constant. It
    /// stays visible while a retry is in flight and clears only when the
    /// retry actually succeeds.
    public var engineErrorMessage: String? {
        if case let .failed(message, _) = phase {
            return message
        }
        return nil
    }

    /// Whether the monitoring section (level meters) is shown: only for
    /// a settled, healthy run, so the section neither slides in during a
    /// start that may still fail nor disappears during a model switch.
    public var showsMonitoring: Bool {
        if case .running = phase {
            return mode != .off
        }
        return false
    }

    /// True while the selected mode is not actually delivering (start
    /// failure, runtime stop, or a preview whose monitor is not playing).
    /// The mode control is the user's intent and never moves on its own;
    /// this drives the warning tint that says "you asked for this, but it
    /// is not running".
    public var isModeUnfulfilled: Bool {
        engineErrorMessage != nil || (mode == .preview && messages.previewError != nil)
    }

    /// Informational caption for a native-rate selection, shown under
    /// the microphone list (secondary style — a property of the device,
    /// not an error). Telephony-profile rates: Bluetooth headset
    /// microphones are captured at their narrow-band native rate and
    /// resampled (issue #7), which cannot restore full-band quality, and
    /// using the headset's microphone drops the whole headset into the
    /// phone profile, so its *playback* quality degrades too while the
    /// engine runs. Full-band rates (44.1 kHz and up) only note the
    /// conversion — nothing about the audio is narrow-band. A pure
    /// projection of the selection — no state, no clearing rules.
    public var microphoneNotice: String? {
        guard
            let device = inputDevices.first(where: { $0.uid == selectedInputUID }),
            case let .nativeRate(hertz) = device.capture
        else {
            return nil
        }
        let rate = device.rateLabel ?? "\(hertz) Hz"
        guard CaptureSupport.isTelephonyRate(hertz) else {
            return "\(device.name) captures at \(rate) and is resampled to the 48 kHz "
                + "engine rate inside Noican (exact ratio, drift-compensated)."
        }
        return "\(device.name) captures at \(rate) (Bluetooth phone profile): audio is "
            + "narrow-band — resampling can't restore full quality — and headset "
            + "playback quality also drops while the microphone is in use."
    }
}
