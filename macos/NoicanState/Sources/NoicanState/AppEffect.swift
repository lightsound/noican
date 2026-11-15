public enum AppEffect: Hashable, Sendable {
    case stopEngine
    case startEngine(StartAttempt)
    case setMonitor(enabled: Bool)
    case switchModel(to: String)
    case setIntensity(Double)
    case setLaunchAtLogin(enabled: Bool)
}
