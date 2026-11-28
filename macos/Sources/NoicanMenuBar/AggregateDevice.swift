import CoreAudio
import Foundation
import NoicanState
import os

struct AggregateComposition {
    let deviceID: AudioObjectID
    let virtualOutputChannels: VirtualOutputChannels
}

final class AggregateDevice: @unchecked Sendable {
    private static let log = Logger(
        subsystem: "com.lightsound.noican", category: "engine-diagnostics"
    )

    private(set) var identifier = AudioObjectID(kAudioObjectUnknown)

    deinit {
        destroy()
    }

    func create(
        input: AudioDeviceInfo,
        virtualOutput: AudioDeviceInfo
    ) throws -> AggregateComposition {
        destroy()
        try Self.ensure48k(input, role: "microphone \"\(input.name)\"")
        try Self.ensure48k(virtualOutput, role: "virtual output \"\(virtualOutput.name)\"")
        let ahead = [input]
        let order = ahead + [virtualOutput]
        let virtualOutputChannels = VirtualOutputChannels(
            outputChannelsAhead: ahead.map(Self.liveOutputChannels),
            virtualOutputChannels: Self.liveOutputChannels(virtualOutput)
        )
        let subdevices: [[String: Any]] = order.map { device -> [String: Any] in
            [
                kAudioSubDeviceUIDKey: device.uid,
                kAudioSubDeviceDriftCompensationKey: device.uid == virtualOutput.uid
            ]
        }
        let description: [String: Any] = [
            kAudioAggregateDeviceNameKey: "Noican Private Aggregate",
            kAudioAggregateDeviceUIDKey: "com.lightsound.noican-aggregate.\(UUID().uuidString)",
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
        do {
            try waitUntilAlive()
            try configureTiming()
        } catch {
            destroy()
            throw error
        }
        let composition = AggregateComposition(
            deviceID: aggregate,
            virtualOutputChannels: virtualOutputChannels
        )
        Self.logComposition(composition, input: input, virtualOutput: virtualOutput)
        return composition
    }

    private static func logComposition(
        _ composition: AggregateComposition,
        input: AudioDeviceInfo,
        virtualOutput: AudioDeviceInfo
    ) {
        let layout = composition.virtualOutputChannels
        let aggregate = composition.deviceID
        log.info(
            """
            Aggregate composed: microphone "\(input.name, privacy: .public)" \
            (in \(AudioDeviceCatalog.liveInputChannelCount(input.id) ?? 0, privacy: .public) / \
            out \(layout.firstChannel, privacy: .public)), \
            virtual output "\(virtualOutput.name, privacy: .public)" \
            (out \(layout.channelCount, privacy: .public)); \
            aggregate reports \
            \(AudioDeviceCatalog.liveOutputChannelCount(aggregate) ?? 0, privacy: .public) \
            output channel(s) in \
            \(AudioDeviceCatalog.liveOutputStreamCount(aggregate) ?? 0, privacy: .public) \
            stream(s)
            """
        )
    }

    func destroy() {
        guard identifier != AudioObjectID(kAudioObjectUnknown) else {
            return
        }
        AudioHardwareDestroyAggregateDevice(identifier)
        identifier = AudioObjectID(kAudioObjectUnknown)
    }

    private static func liveOutputChannels(_ device: AudioDeviceInfo) -> UInt32 {
        AudioDeviceCatalog.liveOutputChannelCount(device.id) ?? device.outputChannels
    }

    static func ensure48k(_ device: AudioDeviceInfo, role: String) throws {
        let target = 48_000.0
        if let rate = AudioDeviceCatalog.nominalSampleRate(device.id),
            abs(rate - target) < 0.5 {
            return
        }
        guard AudioDeviceCatalog.supportsSampleRate(device.id, target) else {
            throw CoreAudioControlError(
                operation: "The \(role) cannot run at 48 kHz",
                status: kAudioDeviceUnsupportedFormatError
            )
        }
        let status = AudioDeviceCatalog.requestSampleRate(device.id, target)
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
