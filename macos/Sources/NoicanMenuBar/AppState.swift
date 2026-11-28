import Combine
import CoreAudio
import Foundation
import NoicanState
import ServiceManagement
import os

@MainActor
final class AppState: ObservableObject {
    static let log = Logger(
        subsystem: "com.lightsound.noican", category: "engine-diagnostics"
    )

    @Published private(set) var model: AppModel
    @Published private(set) var inputLevel: Float = 0
    @Published private(set) var outputLevel: Float = 0

    let models = RustEngine.models()

    nonisolated static let defaultModelID = "dfn3"

    private enum PreferenceKey {
        static let modelID = "selectedModelID"
        static let inputUID = "selectedInputUID"
        static let intensity = "intensity"
    }

    private(set) var allDevices: [AudioDeviceInfo] = []
    private let aggregate = AggregateDevice()
    var engine: RustEngine?
    private var faultPollTask: Task<Void, Never>?
    private var lastFrameCount: UInt64 = 0
    private var stalledTicks = 0
    let diagnostics = EngineDiagnostics()
    var monitorSafetyListener: (device: AudioObjectID, block: AudioObjectPropertyListenerBlock)?
    var activeCaptureRate: Double?
    var inputRateListener: (device: AudioObjectID, block: AudioObjectPropertyListenerBlock)?

    init() {
        var initialPhase = EnginePhase.off
        do {
            engine = try RustEngine()
        } catch {
            initialPhase = .failed(error.localizedDescription, session: nil)
        }
        var modelID = Self.defaultModelID
        if !models.contains(where: { $0.id == modelID }) {
            modelID = models.first?.id ?? ""
        }
        model = AppModel(
            machine: .settled(initialPhase),
            selectedModelID: modelID,
            isEngineAvailable: engine != nil
        )
        refreshDevices()
        restorePreferences()
        dispatch(.launchAtLoginStatusRead(
            isEnabled: Self.isLoginItemRegistered(SMAppService.mainApp.status)
        ))
        registerDeviceListener()
        registerDefaultOutputListener()
    }

    func setMode(_ newMode: EngineMode) {
        guard !model.isBusy, let engine else {
            return
        }
        dispatch(.modeSelected(
            newMode,
            monitorTargetError: newMode == .preview ? RustEngine.monitorTargetError : nil,
            isEngineRunning: engine.isRunning
        ))
    }

    func selectModel(_ id: String) {
        dispatch(.modelSelected(id))
    }

    func selectMicrophone(_ uid: String) {
        dispatch(.microphoneSelected(uid))
    }

    func setIntensity(_ value: Double) {
        dispatch(.intensityChanged(value))
    }

    func setLaunchAtLogin(_ enabled: Bool) {
        dispatch(.launchAtLoginToggled(enabled))
    }

    func setProcessingAllowed(_ isAllowed: Bool) {
        dispatch(.processingAllowanceChanged(isAllowed))
    }

    func dispatch(_ event: AppEvent) {
        let previous = model
        let (newModel, effects) = AppReducer.reduce(model, event)
        model = newModel
        persistPreferences(from: previous, event: event)
        for effect in effects {
            perform(effect)
        }
        syncFaultPolling()
        syncMonitorSafetyObservation()
        syncInputRateObservation()
    }

    private func perform(_ effect: AppEffect) {
        switch effect {
        case .stopEngine:
            engine?.stop()
            aggregate.destroy()
        case let .startEngine(attempt):
            startEngine(attempt)
        case let .setMonitor(enabled):
            setMonitor(enabled)
        case let .switchModel(id):
            switchModel(id)
        case let .setIntensity(value):
            engine?.setIntensity(Float(value))
        case let .setLaunchAtLogin(enabled):
            applyLaunchAtLogin(enabled)
        }
    }

    private func startEngine(_ attempt: StartAttempt) {
        guard
            let engine,
            let input = allDevices.first(where: { $0.uid == attempt.inputUID }),
            let virtualOutput = AudioDeviceCatalog.virtualOutput(in: allDevices)
        else {
            Task { @MainActor in
                self.dispatch(.startCompleted(error: "The selected device disappeared"))
            }
            return
        }
        let aggregate = self.aggregate
        let modelID = attempt.modelID
        let snapshotCapture = model.inputDevices.first { $0.uid == attempt.inputUID }?.capture
        Task.detached {
            guard await MicrophoneAccess.isGranted() else {
                await self.finishStart(captureRate: nil, error: MicrophoneAccess.deniedMessage)
                return
            }
            var captureRate: Double?
            let result = Result {
                if Self.shouldUseAggregate(for: input.id, snapshot: snapshotCapture) {
                    let composition = try aggregate.create(
                        input: input, virtualOutput: virtualOutput
                    )
                    try engine.start(composition, model: modelID)
                } else {
                    try AggregateDevice.ensure48k(
                        virtualOutput, role: "virtual output \"\(virtualOutput.name)\""
                    )
                    let rate = AudioDeviceCatalog.nominalSampleRate(input.id) ?? 0
                    captureRate = rate
                    try engine.startNative(
                        inputDevice: input.id,
                        virtualOutput: virtualOutput.id,
                        captureRate: rate,
                        model: modelID
                    )
                }
            }
            await self.finishStart(captureRate: captureRate, error: result.errorMessage)
        }
    }

    private func setMonitor(_ enabled: Bool) {
        guard let engine else {
            return
        }
        Task.detached {
            let result = Result { try engine.setMonitor(enabled) }
            await self.finish(.monitorChangeCompleted(error: result.errorMessage))
        }
    }

    private func switchModel(_ id: String) {
        guard let engine else {
            return
        }
        Task.detached {
            let result = Result { try engine.setModel(id) }
            await self.finish(.modelSwitchCompleted(error: result.errorMessage))
        }
    }

    private static func isLoginItemRegistered(_ status: SMAppService.Status) -> Bool {
        status == .enabled || status == .requiresApproval
    }

    private func applyLaunchAtLogin(_ enabled: Bool) {
        Task.detached {
            var message: String?
            do {
                if enabled {
                    try SMAppService.mainApp.register()
                } else {
                    try await SMAppService.mainApp.unregister()
                }
            } catch {
                message = "Could not update the login item: \(error.localizedDescription)"
            }
            let status = SMAppService.mainApp.status
            if message == nil, enabled, status == .requiresApproval {
                message = "Registered — allow Noican in System Settings › General › Login Items to finish enabling it."
            }
            await self.finish(.launchAtLoginChangeCompleted(
                isEnabled: Self.isLoginItemRegistered(status),
                error: message
            ))
        }
    }

    private func finish(_ event: AppEvent) {
        dispatch(event)
    }

    private func restorePreferences() {
        let defaults = UserDefaults.standard
        var modelID = defaults.string(forKey: PreferenceKey.modelID)
        if !models.contains(where: { $0.id == modelID }) {
            modelID = nil
        }
        dispatch(.preferencesRestored(
            modelID: modelID,
            inputUID: defaults.string(forKey: PreferenceKey.inputUID),
            intensity: defaults.object(forKey: PreferenceKey.intensity) as? Double
        ))
    }

    private func persistPreferences(from previous: AppModel, event: AppEvent) {
        switch event {
        case .devicesChanged, .preferencesRestored:
            return
        default:
            break
        }
        let defaults = UserDefaults.standard
        if model.selectedModelID != previous.selectedModelID {
            defaults.set(model.selectedModelID, forKey: PreferenceKey.modelID)
        }
        if model.selectedInputUID != previous.selectedInputUID {
            defaults.set(model.selectedInputUID, forKey: PreferenceKey.inputUID)
        }
        if model.intensity != previous.intensity {
            defaults.set(model.intensity, forKey: PreferenceKey.intensity)
        }
    }

    func refreshDevices() {
        do {
            allDevices = try AudioDeviceCatalog.devices()
            let inputs = allDevices
                .filter(AudioDeviceCatalog.isSelectableInput)
                .sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
                .map { device in
                    InputDevice(
                        uid: device.uid,
                        name: device.name,
                        capture: CaptureSupport.classify(
                            supports48kHz: AudioDeviceCatalog.supportsSampleRate(device.id, 48_000),
                            nominalRate: AudioDeviceCatalog.nominalSampleRate(device.id) ?? 0
                        )
                    )
                }
            dispatch(.devicesChanged(
                inputs: inputs,
                allInputUIDs: Set(allDevices.filter { $0.inputChannels > 0 }.map(\.uid)),
                isVirtualOutputPresent: AudioDeviceCatalog.virtualOutput(in: allDevices) != nil
            ))
        } catch {
            dispatch(.deviceQueryFailed(error.localizedDescription))
        }
    }

    private func registerDeviceListener() {
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioHardwarePropertyDevices,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        let status = AudioObjectAddPropertyListenerBlock(
            AudioObjectID(kAudioObjectSystemObject),
            &address,
            DispatchQueue.main
        ) { [weak self] _, _ in
            MainActor.assumeIsolated {
                self?.refreshDevices()
                self?.refreshPreviewAvailability()
                self?.checkMonitorSafety()
            }
        }
        if status != noErr {
            Self.log.warning("Device-list listener registration failed (status \(status))")
        }
    }
}

extension AppState {
    private func syncFaultPolling() {
        if model.hasLiveTransport {
            guard faultPollTask == nil else {
                return
            }
            lastFrameCount = 0
            stalledTicks = 0
            diagnostics.reset()
            faultPollTask = Task { [weak self] in
                while !Task.isCancelled {
                    try? await Task.sleep(for: .seconds(1))
                    self?.checkEngineHealth()
                }
            }
        } else {
            faultPollTask?.cancel()
            faultPollTask = nil
        }
    }

    private func checkEngineHealth() {
        checkVirtualOutputLevel()
        guard model.mode != .off, !model.isBusy, let engine else {
            return
        }
        checkMonitorTrip()
        checkMonitorSafety()
        checkInputRate()
        if engine.isFaulted {
            dispatch(.engineFaulted)
            return
        }
        if !engine.isRunning {
            dispatch(.engineStoppedUnexpectedly)
            return
        }
        let frames = engine.framesProcessed
        if frames == lastFrameCount {
            stalledTicks += 1
            if stalledTicks >= 3 {
                dispatch(.audioStalled)
            }
        } else {
            lastFrameCount = frames
            stalledTicks = 0
        }
        diagnostics.sample(engine, activeModelID: model.selectedModelID, splitTransport: activeCaptureRate != nil)
    }
}

extension AppState {
    func pollLevels() async {
        refreshPreviewAvailability()
        var ticks = 0
        while !Task.isCancelled {
            refreshLevels()
            ticks += 1
            if ticks.isMultiple(of: 20) {
                refreshPreviewAvailability()
            }
            try? await Task.sleep(for: .milliseconds(50))
        }
    }

    private func refreshLevels() {
        guard model.mode != .off, let engine else {
            if inputLevel != 0 { inputLevel = 0 }
            if outputLevel != 0 { outputLevel = 0 }
            return
        }
        inputLevel = engine.inputLevel
        outputLevel = engine.outputLevel
        checkMonitorTrip()
    }

    private func checkMonitorTrip() {
        guard
            model.mode != .off, !model.isBusy, let engine,
            engine.monitorState == .tripped
        else {
            return
        }
        dispatch(.monitorTripped)
    }

    private func refreshPreviewAvailability() {
        guard model.messages.previewUnavailableReason != nil else {
            return
        }
        dispatch(.monitorTargetErrorChanged(RustEngine.monitorTargetError))
    }

    private nonisolated static func shouldUseAggregate(
        for device: AudioObjectID,
        snapshot: CaptureSupport?
    ) -> Bool {
        if let advertises48k = AudioDeviceCatalog.advertisesSampleRate(device, 48_000) {
            return advertises48k
        }
        if let nominal = AudioDeviceCatalog.nominalSampleRate(device),
           case .unsupported = CaptureSupport.classify(
               supports48kHz: false, nominalRate: nominal
           ) {
            return true
        }
        if case .nativeRate = snapshot {
            return false
        }
        return true
    }

    private func finishStart(captureRate: Double?, error: String?) {
        activeCaptureRate = error == nil ? captureRate : nil
        dispatch(.startCompleted(error: error))
        checkVirtualOutputLevel()
    }

    private func registerDefaultOutputListener() {
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioHardwarePropertyDefaultOutputDevice,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        let status = AudioObjectAddPropertyListenerBlock(
            AudioObjectID(kAudioObjectSystemObject),
            &address,
            DispatchQueue.main
        ) { [weak self] _, _ in
            MainActor.assumeIsolated {
                self?.refreshPreviewAvailability()
            }
        }
        if status != noErr {
            Self.log.warning("Default-output listener registration failed (status \(status))")
        }
    }
}

extension Result where Success == Void {
    fileprivate var errorMessage: String? {
        if case let .failure(error) = self {
            return error.localizedDescription
        }
        return nil
    }
}
