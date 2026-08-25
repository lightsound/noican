import CoreAudio
import Foundation

final class AggregateDevice {
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
}
