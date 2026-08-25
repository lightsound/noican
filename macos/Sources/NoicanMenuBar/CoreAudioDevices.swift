import CoreAudio
import Foundation

struct AudioDeviceInfo: Hashable, Identifiable {
    let id: UInt32
    let uid: String
    let name: String
    let inputChannels: UInt32
    let outputChannels: UInt32
}

enum AudioDeviceCatalog {
    static func devices() throws -> [AudioDeviceInfo] {
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioHardwarePropertyDevices,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        var byteCount: UInt32 = 0
        try check(
            AudioObjectGetPropertyDataSize(
                AudioObjectID(kAudioObjectSystemObject),
                &address,
                0,
                nil,
                &byteCount
            ),
            operation: "AudioObjectGetPropertyDataSize(devices)"
        )
        let count = Int(byteCount) / MemoryLayout<AudioObjectID>.size
        var identifiers = [AudioObjectID](repeating: 0, count: count)
        try identifiers.withUnsafeMutableBytes { bytes in
            guard let baseAddress = bytes.baseAddress else {
                throw CoreAudioControlError(
                    operation: "Core Audio returned an empty device list",
                    status: kAudioHardwareBadPropertySizeError
                )
            }
            try check(
                AudioObjectGetPropertyData(
                    AudioObjectID(kAudioObjectSystemObject),
                    &address,
                    0,
                    nil,
                    &byteCount,
                    baseAddress
                ),
                operation: "AudioObjectGetPropertyData(devices)"
            )
        }
        return identifiers.compactMap(deviceInfo)
    }

    static func virtualOutput(in devices: [AudioDeviceInfo]) -> AudioDeviceInfo? {
        devices.first { device in
            guard device.outputChannels > 0 else {
                return false
            }
            let normalizedUID = device.uid.lowercased()
            return device.uid == "BlackHole2ch_UID"
                || normalizedUID.hasPrefix("com.lightsound.noican.")
        }
    }

    private static func deviceInfo(_ identifier: AudioObjectID) -> AudioDeviceInfo? {
        guard
            let uid = try? stringProperty(
                identifier,
                selector: kAudioDevicePropertyDeviceUID
            ),
            let name = try? stringProperty(
                identifier,
                selector: kAudioObjectPropertyName
            )
        else {
            return nil
        }
        return AudioDeviceInfo(
            id: identifier,
            uid: uid,
            name: name,
            inputChannels: channelCount(identifier, scope: kAudioObjectPropertyScopeInput),
            outputChannels: channelCount(identifier, scope: kAudioObjectPropertyScopeOutput)
        )
    }

    private static func stringProperty(
        _ object: AudioObjectID,
        selector: AudioObjectPropertySelector
    ) throws -> String {
        var address = AudioObjectPropertyAddress(
            mSelector: selector,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        // A properly typed, explicitly allocated landing slot: passing
        // `&value` of an `Optional<CFString>` local forms a raw pointer to a
        // non-trivial Swift-managed value, which the compiler warns about.
        let value = UnsafeMutablePointer<CFString?>.allocate(capacity: 1)
        value.initialize(to: nil)
        defer {
            value.deinitialize(count: 1)
            value.deallocate()
        }
        var byteCount = UInt32(MemoryLayout<CFString?>.size)
        try check(
            AudioObjectGetPropertyData(
                object,
                &address,
                0,
                nil,
                &byteCount,
                value
            ),
            operation: "AudioObjectGetPropertyData(string)"
        )
        guard let string = value.pointee else {
            throw CoreAudioControlError.missingProperty(selector)
        }
        return string as String
    }

    private static func channelCount(
        _ device: AudioDeviceID,
        scope: AudioObjectPropertyScope
    ) -> UInt32 {
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioDevicePropertyStreamConfiguration,
            mScope: scope,
            mElement: kAudioObjectPropertyElementMain
        )
        var byteCount: UInt32 = 0
        guard
            AudioObjectGetPropertyDataSize(device, &address, 0, nil, &byteCount) == noErr,
            byteCount >= UInt32(MemoryLayout<AudioBufferList>.size)
        else {
            return 0
        }
        let storage = UnsafeMutableRawPointer.allocate(
            byteCount: Int(byteCount),
            alignment: MemoryLayout<AudioBufferList>.alignment
        )
        defer {
            storage.deallocate()
        }
        guard
            AudioObjectGetPropertyData(
                device,
                &address,
                0,
                nil,
                &byteCount,
                storage
            ) == noErr
        else {
            return 0
        }
        let buffers = UnsafeMutableAudioBufferListPointer(
            storage.assumingMemoryBound(to: AudioBufferList.self)
        )
        return buffers.reduce(0) { total, buffer in
            total + buffer.mNumberChannels
        }
    }
}

struct CoreAudioControlError: LocalizedError {
    let operation: String
    let status: OSStatus

    init(operation: String, status: OSStatus) {
        self.operation = operation
        self.status = status
    }

    static func missingProperty(
        _ selector: AudioObjectPropertySelector
    ) -> CoreAudioControlError {
        CoreAudioControlError(
            operation: "Missing Core Audio property \(selector)",
            status: kAudioHardwareUnknownPropertyError
        )
    }

    var errorDescription: String? {
        "\(operation) failed with OSStatus \(status)"
    }
}

func check(_ status: OSStatus, operation: String) throws {
    guard status == noErr else {
        throw CoreAudioControlError(operation: operation, status: status)
    }
}
