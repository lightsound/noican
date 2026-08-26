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

    /// True for the loopback device Noican routes into (stock BlackHole 2ch
    /// in Phase 0, or the Noican-branded fork later). It registers input
    /// channels too, but selecting it as the microphone would only feed the
    /// loopback back into itself, so pickers must exclude it.
    ///
    /// Keep in sync with the Rust preview-monitor policy
    /// (`classify_monitor_target` in crates/noican-coreaudio/src/monitor.rs),
    /// which matches the same UIDs and additionally rejects any
    /// virtual/aggregate transport.
    static func isNoicanVirtualDevice(_ device: AudioDeviceInfo) -> Bool {
        device.uid == "BlackHole2ch_UID"
            || device.uid.lowercased().hasPrefix("com.lightsound.noican.")
    }

    /// True for devices that make sense as the physical microphone. Virtual
    /// devices (JoyCast, other loopbacks — they report the `virt` transport
    /// type) and aggregates are excluded: routing another virtual mic into
    /// Noican would double-process or loop audio, and fixed-rate virtual
    /// subdevices also make the private aggregate reject the 48 kHz setup.
    static func isSelectableInput(_ device: AudioDeviceInfo) -> Bool {
        device.inputChannels > 0
            && device.transportType != kAudioDeviceTransportTypeVirtual
            && device.transportType != kAudioDeviceTransportTypeAggregate
            && !isNoicanVirtualDevice(device)
    }

    static func virtualOutput(in devices: [AudioDeviceInfo]) -> AudioDeviceInfo? {
        devices.first { device in
            device.outputChannels > 0 && isNoicanVirtualDevice(device)
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
            outputChannels: channelCount(identifier, scope: kAudioObjectPropertyScopeOutput),
            transportType: transportType(identifier)
        )
    }

    /// Current nominal sample rate of a device, or nil when unreadable.
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

    /// Whether the device advertises support for `rate`. Returns true when
    /// the supported-rate list is unreadable (let the actual set decide).
    static func supportsSampleRate(_ device: AudioObjectID, _ rate: Double) -> Bool {
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
            return true
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
            return true
        }
        return ranges.contains { range in
            range.mMinimum - 0.5 <= rate && rate <= range.mMaximum + 0.5
        }
    }

    /// Requests a nominal sample rate; the change lands asynchronously, so
    /// callers must poll [`nominalSampleRate`] for the result.
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
