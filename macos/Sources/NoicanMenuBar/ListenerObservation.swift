import CoreAudio
import Foundation

extension AppState {

    func checkMonitorSafety() {
        guard
            let session = model.liveSession, session.isMonitorArmed,
            !model.isBusy, let engine
        else {
            return
        }
        let device = engine.monitorDeviceID
        guard device != AudioObjectID(kAudioObjectUnknown) else {
            return
        }
        let reason: String?
        if !allDevices.contains(where: { $0.id == device }) {
            reason = "the monitor output device was disconnected; select Preview again to play on the new output"
        } else {
            reason = RustEngine.monitorDeviceError(device)
        }
        if let reason {
            dispatch(.monitorTargetBecameUnsafe(reason: reason))
        }
    }

    func syncMonitorSafetyObservation() {
        let isArmed = model.liveSession?.isMonitorArmed == true
        if isArmed {
            guard monitorSafetyListener == nil, let engine else {
                return
            }
            let device = engine.monitorDeviceID
            guard device != AudioObjectID(kAudioObjectUnknown) else {
                return
            }
            let block: AudioObjectPropertyListenerBlock = { [weak self] _, _ in
                MainActor.assumeIsolated {
                    self?.checkMonitorSafety()
                }
            }
            var address = Self.monitorDataSourceAddress
            let status = AudioObjectAddPropertyListenerBlock(
                device, &address, DispatchQueue.main, block
            )
            if status == noErr {
                monitorSafetyListener = (device, block)
            } else {
                Self.log.warning(
                    "Monitor-safety listener registration failed (status \(status))"
                )
            }
        } else if let listener = monitorSafetyListener {
            var address = Self.monitorDataSourceAddress
            _ = AudioObjectRemovePropertyListenerBlock(
                listener.device,
                &address,
                DispatchQueue.main,
                listener.block
            )
            monitorSafetyListener = nil
        }
    }

    static var monitorDataSourceAddress: AudioObjectPropertyAddress {
        AudioObjectPropertyAddress(
            mSelector: kAudioDevicePropertyDataSource,
            mScope: kAudioObjectPropertyScopeOutput,
            mElement: kAudioObjectPropertyElementMain
        )
    }

    func syncInputRateObservation() {
        if let session = model.transportSession,
           activeCaptureRate != nil,
           let device = allDevices.first(where: { $0.uid == session.inputUID }) {
            guard inputRateListener?.device != device.id else {
                return
            }
            removeInputRateListener()
            let block: AudioObjectPropertyListenerBlock = { [weak self] _, _ in
                MainActor.assumeIsolated {
                    self?.checkInputRate()
                }
            }
            var address = Self.nominalSampleRateAddress
            let status = AudioObjectAddPropertyListenerBlock(
                device.id, &address, DispatchQueue.main, block
            )
            if status == noErr {
                inputRateListener = (device.id, block)
            } else {
                Self.log.warning(
                    "Input-rate listener registration failed (status \(status))"
                )
            }
            checkInputRate()
        } else {
            removeInputRateListener()
        }
    }

    func removeInputRateListener() {
        guard let listener = inputRateListener else {
            return
        }
        var address = Self.nominalSampleRateAddress
        _ = AudioObjectRemovePropertyListenerBlock(
            listener.device,
            &address,
            DispatchQueue.main,
            listener.block
        )
        inputRateListener = nil
    }

    func checkInputRate() {
        guard
            let expected = activeCaptureRate,
            !model.isBusy, let session = model.transportSession,
            let device = allDevices.first(where: { $0.uid == session.inputUID }),
            let rate = AudioDeviceCatalog.nominalSampleRate(device.id),
            abs(rate - expected) > 0.5
        else {
            return
        }
        dispatch(.inputSampleRateChanged)
    }

    static var nominalSampleRateAddress: AudioObjectPropertyAddress {
        AudioObjectPropertyAddress(
            mSelector: kAudioDevicePropertyNominalSampleRate,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
    }
}
