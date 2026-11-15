@testable import NoicanState

let builtInMic = InputDevice(uid: "builtin", name: "MacBook Pro Microphone", capture: .engineRate)
let usbMic = InputDevice(uid: "usb", name: "USB Microphone", capture: .engineRate)
let bluetoothMic = InputDevice(uid: "bt", name: "AirPods", capture: .nativeRate(hertz: 16_000))
let fortyFourMic = InputDevice(
    uid: "cd", name: "CD-rate Headset", capture: .nativeRate(hertz: 44_100)
)
let unsupportedMic = InputDevice(
    uid: "odd", name: "Broken Interface", capture: .unsupported(hertz: 0)
)

func readyModel(
    devices: [InputDevice] = [builtInMic, usbMic, bluetoothMic, fortyFourMic, unsupportedMic],
    selectedInputUID: String = builtInMic.uid,
    selectedModelID: String = "dfn3"
) -> AppModel {
    AppModel(
        selectedModelID: selectedModelID,
        selectedInputUID: selectedInputUID,
        inputDevices: devices,
        isVirtualOutputPresent: true
    )
}

func drive(_ model: AppModel, _ events: [AppEvent]) -> AppModel {
    events.reduce(model) { state, event in
        AppReducer.reduce(state, event).state
    }
}

func step(_ model: AppModel, _ event: AppEvent) -> (state: AppModel, effects: [AppEffect]) {
    AppReducer.reduce(model, event)
}

func tap(_ mode: EngineMode, monitorTargetError: String? = nil, isEngineRunning: Bool? = nil) -> AppEvent {
    .modeSelected(
        mode,
        monitorTargetError: monitorTargetError,
        isEngineRunning: isEngineRunning ?? false
    )
}

func runningModel(mode: EngineMode = .on) -> AppModel {
    let monitorEvents: [AppEvent] = mode == .preview ? [.monitorChangeCompleted(error: nil)] : []
    return drive(
        readyModel(),
        [tap(mode), .startCompleted(error: nil)] + monitorEvents
    )
}
