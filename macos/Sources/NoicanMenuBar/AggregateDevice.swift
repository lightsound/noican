import CoreAudio
import Foundation

/// Owns the private Aggregate Device combining the physical input and the
/// virtual output. `@unchecked Sendable`: creation happens on the detached
/// start task and teardown on the main actor, but never concurrently —
/// `AppState.isBusy` serializes every operation that touches this object.
final class AggregateDevice: @unchecked Sendable {
    private(set) var identifier = AudioObjectID(kAudioObjectUnknown)

    deinit {
        destroy()
    }

    func create(
        input: AudioDeviceInfo,
        virtualOutput: AudioDeviceInfo
    ) throws -> AudioObjectID {
        destroy()
        // Switch the subdevices themselves to 48 kHz before composing the
        // aggregate: the aggregate follows its clock master (the mic), and
        // setting the rate on the aggregate alone is rejected ('nope') by
        // some configurations while the mic idles at another rate (16 kHz
        // Bluetooth profiles, 44.1 kHz defaults, ...).
        try Self.ensure48k(input, role: "microphone \"\(input.name)\"")
        try Self.ensure48k(virtualOutput, role: "virtual output \"\(virtualOutput.name)\"")
        let subdevices: [[String: Any]] = [
            [
                kAudioSubDeviceUIDKey: input.uid,
                kAudioSubDeviceDriftCompensationKey: false
            ],
            [
                kAudioSubDeviceUIDKey: virtualOutput.uid,
                kAudioSubDeviceDriftCompensationKey: true
            ]
        ]
        let description: [String: Any] = [
            kAudioAggregateDeviceNameKey: "Noican Private Aggregate",
            kAudioAggregateDeviceUIDKey: "com.lightsound.noican.aggregate.\(UUID().uuidString)",
            kAudioAggregateDeviceMainSubDeviceKey: input.uid,
            kAudioAggregateDeviceClockDeviceKey: input.uid,
            kAudioAggregateDeviceIsPrivateKey: true,
            kAudioAggregateDeviceIsStackedKey: false,
            kAudioAggregateDeviceSubDeviceListKey: subdevices
        ]
        var aggregate = AudioObjectID(kAudioObjectUnknown)
        try check(
            AudioHardwareCreateAggregateDevice(description as CFDictionary, &aggregate),
            operation: "AudioHardwareCreateAggregateDevice"
        )
        guard aggregate != AudioObjectID(kAudioObjectUnknown) else {
            throw CoreAudioControlError(
                operation: "AudioHardwareCreateAggregateDevice returned no device",
                status: kAudioHardwareBadObjectError
            )
        }
        identifier = aggregate
        try waitUntilAlive()
        try configureTiming()
        return aggregate
    }

    func destroy() {
        guard identifier != AudioObjectID(kAudioObjectUnknown) else {
            return
        }
        AudioHardwareDestroyAggregateDevice(identifier)
        identifier = AudioObjectID(kAudioObjectUnknown)
    }

    /// Switches `device` to 48 kHz (polling the asynchronous change),
    /// or throws when it cannot run there. Shared with the split
    /// native-capture path, which has no aggregate but must still hold
    /// the virtual output at the 48 kHz engine rate for consumers.
    static func ensure48k(_ device: AudioDeviceInfo, role: String) throws {
        let target = 48_000.0
        if let rate = AudioDeviceCatalog.nominalSampleRate(device.id),
            abs(rate - target) < 0.5 {
            return
        }
        guard AudioDeviceCatalog.supportsSampleRate(device.id, target) else {
            // Defensive only: microphones that cannot run at 48 kHz
            // (Bluetooth telephony profiles) are routed to the split
            // native-capture transport by the control plane (issue #7)
            // and never reach the aggregate path.
            throw CoreAudioControlError(
                operation: "The \(role) cannot run at 48 kHz",
                status: kAudioDeviceUnsupportedFormatError
            )
        }
        let status = AudioDeviceCatalog.requestSampleRate(device.id, target)
        // The change lands asynchronously; poll on this background task.
        for _ in 0..<40 {
            if let rate = AudioDeviceCatalog.nominalSampleRate(device.id),
                abs(rate - target) < 0.5 {
                return
            }
            Thread.sleep(forTimeInterval: 0.05)
        }
        throw CoreAudioControlError(
            operation: "The \(role) did not switch to 48 kHz",
            status: status == noErr ? kAudioHardwareUnspecifiedError : status
        )
    }

    private func waitUntilAlive() throws {
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioDevicePropertyDeviceIsAlive,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        for _ in 0..<30 {
            var alive: UInt32 = 0
            var byteCount = UInt32(MemoryLayout<UInt32>.size)
            let status = AudioObjectGetPropertyData(
                identifier,
                &address,
                0,
                nil,
                &byteCount,
                &alive
            )
            if status == noErr, alive != 0 {
                return
            }
            Thread.sleep(forTimeInterval: 0.05)
        }
        destroy()
        throw CoreAudioControlError(
            operation: "Aggregate Device did not become alive",
            status: kAudioHardwareNotRunningError
        )
    }

    private func configureTiming() throws {
        try configureSampleRate()
        try configureBufferSize()
    }

    private func configureSampleRate() throws {
        var sampleRateAddress = AudioObjectPropertyAddress(
            mSelector: kAudioDevicePropertyNominalSampleRate,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        var requestedRate = 48_000.0
        // Nominal-rate changes propagate to the subdevices asynchronously,
        // and some drivers reject the set while already at the target rate,
        // so poll the read-back instead of trusting one set + one read
        // (a single immediate read loses the race and made every start fail
        // while the microphone idled at 44.1 kHz). Runs on the background
        // start task, never on the main actor.
        var setStatus = AudioObjectSetPropertyData(
            identifier,
            &sampleRateAddress,
            0,
            nil,
            UInt32(MemoryLayout<Double>.size),
            &requestedRate
        )
        var actualRate = 0.0
        for attempt in 0..<40 {
            var rateSize = UInt32(MemoryLayout<Double>.size)
            try check(
                AudioObjectGetPropertyData(
                    identifier,
                    &sampleRateAddress,
                    0,
                    nil,
                    &rateSize,
                    &actualRate
                ),
                operation: "Read Aggregate Device sample rate"
            )
            if abs(actualRate - requestedRate) < 0.5 {
                return
            }
            // Re-issue once midway in case the first set landed before the
            // subdevices had finished attaching.
            if attempt == 20 {
                setStatus = AudioObjectSetPropertyData(
                    identifier,
                    &sampleRateAddress,
                    0,
                    nil,
                    UInt32(MemoryLayout<Double>.size),
                    &requestedRate
                )
            }
            Thread.sleep(forTimeInterval: 0.05)
        }
        throw CoreAudioControlError(
            operation: "48 kHz not reached (mic reports \(Int(actualRate)) Hz)",
            status: setStatus == noErr ? kAudioHardwareUnspecifiedError : setStatus
        )
    }

    private func configureBufferSize() throws {
        var bufferAddress = AudioObjectPropertyAddress(
            mSelector: kAudioDevicePropertyBufferFrameSize,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        // 256 frames is a preference, not a requirement: the transport's
        // ring buffers absorb whatever callback size the device settles on,
        // so a driver clamping to its supported range is fine.
        var requestedFrames: UInt32 = 256
        _ = AudioObjectSetPropertyData(
            identifier,
            &bufferAddress,
            0,
            nil,
            UInt32(MemoryLayout<UInt32>.size),
            &requestedFrames
        )
        var actualFrames: UInt32 = 0
        var frameSize = UInt32(MemoryLayout<UInt32>.size)
        try check(
            AudioObjectGetPropertyData(
                identifier,
                &bufferAddress,
                0,
                nil,
                &frameSize,
                &actualFrames
            ),
            operation: "Read Aggregate Device buffer size"
        )
        guard (32...4_096).contains(actualFrames) else {
            throw CoreAudioControlError(
                operation: "Aggregate Device reports an unusable buffer size "
                    + "(\(actualFrames) frames)",
                status: kAudioHardwareUnspecifiedError
            )
        }
    }
}
