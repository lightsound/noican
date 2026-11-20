public struct EngineSession: Hashable, Sendable {
    public var modelID: String
    public var inputUID: String
    public var isMonitorArmed: Bool

    public init(modelID: String, inputUID: String, isMonitorArmed: Bool = false) {
        self.modelID = modelID
        self.inputUID = inputUID
        self.isMonitorArmed = isMonitorArmed
    }
}

public enum EnginePhase: Hashable, Sendable {
    case off
    case running(EngineSession)
    case failed(String, session: EngineSession?)
}

public struct StartAttempt: Hashable, Sendable {
    public var modelID: String
    public var inputUID: String
    public var monitor: Bool
    public var revertInputUID: String?

    public init(modelID: String, inputUID: String, monitor: Bool, revertInputUID: String? = nil) {
        self.modelID = modelID
        self.inputUID = inputUID
        self.monitor = monitor
        self.revertInputUID = revertInputUID
    }
}

public enum EngineTransition: Hashable, Sendable {
    case starting(StartAttempt)
    case settingMonitor(enabled: Bool, session: EngineSession)
    case switchingModel(to: String, session: EngineSession?)
}

public enum EngineMachine: Hashable, Sendable {
    case settled(EnginePhase)
    case busy(EngineTransition, rendering: EnginePhase)
}

public struct MessageSlots: Hashable, Sendable {
    public var previewError: String?
    public var previewUnavailableReason: String?
    public var microphoneError: String?
    public var modelError: String?
    public var launchAtLoginError: String?
    public var licenseRequired: String?

    public init(
        previewError: String? = nil,
        previewUnavailableReason: String? = nil,
        microphoneError: String? = nil,
        modelError: String? = nil,
        launchAtLoginError: String? = nil,
        licenseRequired: String? = nil
    ) {
        self.previewError = previewError
        self.previewUnavailableReason = previewUnavailableReason
        self.microphoneError = microphoneError
        self.modelError = modelError
        self.launchAtLoginError = launchAtLoginError
        self.licenseRequired = licenseRequired
    }
}

public struct AppModel: Hashable, Sendable {
    public var mode: EngineMode
    public var machine: EngineMachine
    public var selectedModelID: String
    public var selectedInputUID: String
    public var inputDevices: [InputDevice]
    public var isVirtualOutputPresent: Bool
    public var intensity: Double
    public var isLaunchAtLoginEnabled: Bool
    public var isLaunchAtLoginBusy: Bool
    public var messages: MessageSlots
    public var virtualOutputLevel: VirtualOutputLevel
    public var isEngineAvailable: Bool
    public var isProcessingAllowed: Bool

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
        virtualOutputLevel: VirtualOutputLevel = .nominal,
        isEngineAvailable: Bool = true,
        isProcessingAllowed: Bool = true
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
        self.virtualOutputLevel = virtualOutputLevel
        self.isEngineAvailable = isEngineAvailable
        self.isProcessingAllowed = isProcessingAllowed
    }
}

extension AppModel {
    public var isBusy: Bool {
        if case .busy = machine {
            return true
        }
        return false
    }

    public var phase: EnginePhase {
        switch machine {
        case let .settled(phase):
            phase
        case let .busy(_, rendering):
            rendering
        }
    }

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

    public var liveRunningSession: EngineSession? {
        if case let .settled(.running(session)) = machine {
            return session
        }
        return nil
    }

    public var hasLiveTransport: Bool {
        transportSession != nil
    }

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

extension AppModel {
    public var statusText: String {
        switch phase {
        case .off:
            inputDevices.isEmpty ? "No input device" : "Off"
        case let .running(session):
            session.isMonitorArmed ? "Previewing" : "Running"
        case .failed:
            "Error"
        }
    }

    public var engineErrorMessage: String? {
        if case let .failed(message, _) = phase {
            return message
        }
        return nil
    }

    public var showsMonitoring: Bool {
        if case .running = phase {
            return mode != .off
        }
        return false
    }

    public var isModeUnfulfilled: Bool {
        engineErrorMessage != nil || (mode == .preview && messages.previewError != nil)
    }

    public var virtualOutputLevelNotice: String? {
        virtualOutputLevel.notice
    }

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
