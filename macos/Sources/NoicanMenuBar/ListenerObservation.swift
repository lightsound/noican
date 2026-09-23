import CoreAudio
import Foundation

/// Core Audio property listeners the menu-bar app keeps in lockstep with
/// engine state: the preview monitor's per-device data-source watch (a
/// headphone-jack flip fires no device-list or default-output
/// notification) and the running microphone's nominal sample-rate watch
/// (Bluetooth headsets renegotiate A2DP ↔ HFP underneath a split
/// transport that fixed its rate at start). Both live here, off the main
/// `AppState.swift`, so that file stays under the lint length cap; the
/// shared state they touch is `internal` for the same reason (see
/// `allDevices`).
extension AppState {

    /// Re-vets the device the running monitor actually plays on and
    /// auto-stops the preview (via the reducer) when its safety is gone.
    /// Two loss shapes exist, machine-dependent: the same built-in device
    /// flips its data source from the headphone jack to the internal
    /// speakers (caught by the per-device listener and the health poll),
    /// or the jack is a separate device that disappears (caught by the
    /// device-list listener). `noican_monitor_target_error` cannot serve
    /// here: it judges the *current default output*, which may have moved
    /// on while the monitor stayed on the old device.
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

    /// Keeps the per-device data-source listener in lockstep with the
    /// settled monitor state: registered on the monitor's device when an
    /// enable settles, and always removed the moment the monitor is no
    /// longer armed (disable claimed, trip, teardown) — a listener left
    /// behind would fire for a monitor that no longer exists.
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
                // Delivered on the main queue, which is the main actor.
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
                // Without the registration the listener is dead — do
                // not store it, and surface why the safety check
                // stopped reacting to jack reassignments.
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

    /// Keeps the nominal-rate listener in lockstep with the transport:
    /// registered on the session's microphone when a native-capture
    /// start settles (`activeCaptureRate` is the split path's marker)
    /// and kept through busy monitor/model transitions — the transport
    /// stays up during those, so keying on `liveSession` would drop the
    /// listener for the length of every Preview toggle. Removed the
    /// moment the transport is gone. The 48 kHz aggregate path never
    /// registers one, so its behavior is unchanged. Bluetooth headsets
    /// renegotiate profiles (A2DP ↔ HFP), and the split transport
    /// captures at the rate fixed at start time, so a live change must
    /// rebuild the transport.
    func syncInputRateObservation() {
        if let session = model.transportSession,
           activeCaptureRate != nil,
           let device = allDevices.first(where: { $0.uid == session.inputUID }) {
            guard inputRateListener?.device != device.id else {
                return
            }
            // The running microphone changed underneath us (a start
            // attempt switched mics while a listener survived from the
            // previous transport): rebind before registering.
            removeInputRateListener()
            let block: AudioObjectPropertyListenerBlock = { [weak self] _, _ in
                // Delivered on the main queue, which is the main actor.
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
                // The 1 Hz stall poll still catches a rate
                // renegotiation; log why the notification path is dead.
                Self.log.warning(
                    "Input-rate listener registration failed (status \(status))"
                )
            }
            // The effective rate settles at capture start (Bluetooth
            // renegotiates when the HFP link comes up), which can race
            // the start effect's read: compare once at attach time so a
            // renegotiation that landed during the start is caught now
            // rather than waiting for a notification that may never
            // fire again.
            checkInputRate()
        } else {
            removeInputRateListener()
        }
    }

    /// Drops the input-rate listener, if any. Removal of a device that
    /// disappeared in the meantime fails harmlessly, so the status is
    /// ignored.
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

    /// Re-reads the running microphone's nominal rate and rebuilds the
    /// transport (via the reducer) when it no longer matches the rate
    /// the transport was started with. Comparing against the recorded
    /// baseline filters spurious notifications; the busy machine
    /// serializes overlapping rebuilds (a flip landing *during* a busy
    /// transition is dropped here and caught by the attach-time check
    /// or the 1 Hz health poll after the transition settles).
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
