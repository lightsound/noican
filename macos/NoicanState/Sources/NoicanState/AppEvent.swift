public enum AppEvent: Hashable, Sendable {

    case modeSelected(EngineMode, monitorTargetError: String?, isEngineRunning: Bool)
    case modelSelected(String)
    case microphoneSelected(String)
    case intensityChanged(Double)
    case launchAtLoginToggled(Bool)

    case startCompleted(error: String?)
    case monitorChangeCompleted(error: String?)
    case modelSwitchCompleted(error: String?)
    case launchAtLoginChangeCompleted(isEnabled: Bool, error: String?)

    case preferencesRestored(modelID: String?, inputUID: String?, intensity: Double?)
    case launchAtLoginStatusRead(isEnabled: Bool)
    case devicesChanged(inputs: [InputDevice], allInputUIDs: Set<String>, isVirtualOutputPresent: Bool)
    case monitorTargetErrorChanged(String?)
    case inputSampleRateChanged
    case monitorTripped
    case monitorTargetBecameUnsafe(reason: String)
    case engineFaulted
    case engineStoppedUnexpectedly
    case audioStalled
    case virtualOutputLevelObserved(VirtualOutputLevel)
    case deviceQueryFailed(String)
    case processingAllowanceChanged(Bool)
}
