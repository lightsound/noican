@testable import NoicanState

// MARK: - Shared fixtures and drivers for the reducer test suites

let builtInMic = InputDevice(uid: "builtin", name: "MacBook Pro Microphone", supports48kHz: true)
let usbMic = InputDevice(uid: "usb", name: "USB Microphone", supports48kHz: true)
let bluetoothMic = InputDevice(uid: "bt", name: "AirPods", supports48kHz: false)

/// A ready-to-start model: engine available, devices present, defaults
/// selected — the state right after launch on a healthy machine.
func readyModel(
    devices: [InputDevice] = [builtInMic, usbMic, bluetoothMic],
    selectedInputUID: String = builtInMic.uid,
    selectedModelID: String = "fastenhancer-b"
) -> AppModel {
    AppModel(
        selectedModelID: selectedModelID,
        selectedInputUID: selectedInputUID,
        inputDevices: devices,
        isVirtualOutputPresent: true
    )
}

/// Drives `model` through `events`, discarding intermediate effects.
func drive(_ model: AppModel, _ events: [AppEvent]) -> AppModel {
    events.reduce(model) { state, event in
        AppReducer.reduce(state, event).state
    }
}

/// One reduce step, returning both halves for assertions.
func step(_ model: AppModel, _ event: AppEvent) -> (state: AppModel, effects: [AppEffect]) {
    AppReducer.reduce(model, event)
}

/// A user tap on the mode control under a healthy environment (safe
/// preview target; live engine liveness mirrors the machine).
func tap(_ mode: EngineMode, monitorTargetError: String? = nil, isEngineRunning: Bool? = nil) -> AppEvent {
    .modeSelected(
        mode,
        monitorTargetError: monitorTargetError,
        isEngineRunning: isEngineRunning ?? false
    )
}

/// A running state reached through the real start transition.
func runningModel(mode: EngineMode = .on) -> AppModel {
    let monitorEvents: [AppEvent] = mode == .preview ? [.monitorChangeCompleted(error: nil)] : []
    return drive(
        readyModel(),
        [tap(mode), .startCompleted(error: nil)] + monitorEvents
    )
}
