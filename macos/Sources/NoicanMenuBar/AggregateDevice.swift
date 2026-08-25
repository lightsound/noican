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
            kAudioAggregateDeviceNameKey: "noican Private Aggregate",
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
        try check(
            AudioObjectSetPropertyData(
                identifier,
                &sampleRateAddress,
                0,
                nil,
                UInt32(MemoryLayout<Double>.size),
                &requestedRate
            ),
            operation: "Set Aggregate Device sample rate"
        )
        var actualRate = 0.0
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
        guard abs(actualRate - requestedRate) < 0.5 else {
            throw CoreAudioControlError(
                operation: "Aggregate Device did not accept 48 kHz",
                status: kAudioHardwareUnspecifiedError
            )
        }
    }

    private func configureBufferSize() throws {
        var bufferAddress = AudioObjectPropertyAddress(
            mSelector: kAudioDevicePropertyBufferFrameSize,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        var requestedFrames: UInt32 = 256
        try check(
            AudioObjectSetPropertyData(
                identifier,
                &bufferAddress,
                0,
                nil,
                UInt32(MemoryLayout<UInt32>.size),
                &requestedFrames
            ),
            operation: "Set Aggregate Device buffer size"
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
        guard actualFrames == requestedFrames else {
            throw CoreAudioControlError(
                operation: "Aggregate Device did not accept 256 frames",
                status: kAudioHardwareUnspecifiedError
            )
        }
    }
}
