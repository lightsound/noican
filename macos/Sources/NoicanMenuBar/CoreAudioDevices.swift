import CoreAudio
import Foundation

struct AudioDeviceInfo: Hashable, Identifiable {
    let id: UInt32
    let uid: String
    let name: String
    let inputChannels: UInt32
    let outputChannels: UInt32
    let transportType: UInt32
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

    private static let noicanDriverUIDPrefix = "com.lightsound.noican."

    static func isNoicanVirtualDevice(_ device: AudioDeviceInfo) -> Bool {
        device.uid == "BlackHole2ch_UID" || isNoicanDriverDevice(device)
    }

    private static func isNoicanDriverDevice(_ device: AudioDeviceInfo) -> Bool {
        device.uid.lowercased().hasPrefix(noicanDriverUIDPrefix)
    }

    static func isSelectableInput(_ device: AudioDeviceInfo) -> Bool {
        device.inputChannels > 0
            && device.transportType != kAudioDeviceTransportTypeVirtual
            && device.transportType != kAudioDeviceTransportTypeAggregate
            && !isNoicanVirtualDevice(device)
    }

    static func virtualOutput(in devices: [AudioDeviceInfo]) -> AudioDeviceInfo? {
        let loopbacks = devices.filter { device in
            device.outputChannels > 0 && isNoicanVirtualDevice(device)
        }
        return loopbacks.first(where: isNoicanDriverDevice) ?? loopbacks.first
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
            outputChannels: channelCount(identifier, scope: kAudioObjectPropertyScopeOutput),
            transportType: transportType(identifier)
        )
    }

    static func nominalSampleRate(_ device: AudioObjectID) -> Double? {
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioDevicePropertyNominalSampleRate,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        var rate = 0.0
        var byteCount = UInt32(MemoryLayout<Double>.size)
        guard
            AudioObjectGetPropertyData(device, &address, 0, nil, &byteCount, &rate) == noErr
        else {
            return nil
        }
        return rate
    }

    static func supportsSampleRate(_ device: AudioObjectID, _ rate: Double) -> Bool {
        advertisesSampleRate(device, rate) ?? true
    }

    static func advertisesSampleRate(_ device: AudioObjectID, _ rate: Double) -> Bool? {
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioDevicePropertyAvailableNominalSampleRates,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        var byteCount: UInt32 = 0
        guard
            AudioObjectGetPropertyDataSize(device, &address, 0, nil, &byteCount) == noErr,
            byteCount >= UInt32(MemoryLayout<AudioValueRange>.size)
        else {
            return nil
        }
        let count = Int(byteCount) / MemoryLayout<AudioValueRange>.size
        var ranges = [AudioValueRange](repeating: AudioValueRange(), count: count)
        let status = ranges.withUnsafeMutableBytes { bytes -> OSStatus in
            guard let baseAddress = bytes.baseAddress else {
                return kAudioHardwareBadPropertySizeError
            }
            return AudioObjectGetPropertyData(device, &address, 0, nil, &byteCount, baseAddress)
        }
        guard status == noErr else {
            return nil
        }
        return ranges.contains { range in
            range.mMinimum - 0.5 <= rate && rate <= range.mMaximum + 0.5
        }
    }

    static func requestSampleRate(_ device: AudioObjectID, _ rate: Double) -> OSStatus {
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioDevicePropertyNominalSampleRate,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        var requested = rate
        return AudioObjectSetPropertyData(
            device,
            &address,
            0,
            nil,
            UInt32(MemoryLayout<Double>.size),
            &requested
        )
    }

    private static func transportType(_ device: AudioDeviceID) -> UInt32 {
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioDevicePropertyTransportType,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        var value: UInt32 = 0
        var byteCount = UInt32(MemoryLayout<UInt32>.size)
        guard
            AudioObjectGetPropertyData(device, &address, 0, nil, &byteCount, &value) == noErr
        else {
            return kAudioDeviceTransportTypeUnknown
        }
        return value
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

    static func liveOutputChannelCount(_ device: AudioDeviceID) -> UInt32? {
        readStreamConfiguration(device, scope: kAudioObjectPropertyScopeOutput)?.channels
    }

    static func liveInputChannelCount(_ device: AudioDeviceID) -> UInt32? {
        readStreamConfiguration(device, scope: kAudioObjectPropertyScopeInput)?.channels
    }

    static func liveOutputStreamCount(_ device: AudioDeviceID) -> UInt32? {
        readStreamConfiguration(device, scope: kAudioObjectPropertyScopeOutput)?.streams
    }

    private static func channelCount(
        _ device: AudioDeviceID,
        scope: AudioObjectPropertyScope
    ) -> UInt32 {
        readStreamConfiguration(device, scope: scope)?.channels ?? 0
    }

    private static func readStreamConfiguration(
        _ device: AudioDeviceID,
        scope: AudioObjectPropertyScope
    ) -> (streams: UInt32, channels: UInt32)? {
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
            return nil
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
            return nil
        }
        let buffers = UnsafeMutableAudioBufferListPointer(
            storage.assumingMemoryBound(to: AudioBufferList.self)
        )
        let channels = buffers.reduce(UInt32(0)) { total, buffer in
            total + buffer.mNumberChannels
        }
        return (streams: UInt32(buffers.count), channels: channels)
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
