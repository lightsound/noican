import CoreAudio
import Foundation
import NoicanState
import os

/// Reads the Noican virtual output device's own volume and mute controls.
///
/// The BlackHole-derived driver applies one master volume (−64…0 dB) and
/// one mute to every sample it loops, and macOS shows them as the
/// "Noican Microphone" input slider in System Settings › Sound › Input. A
/// slider left low there makes every consumer hear the processed voice
/// quietly, with nothing in Noican's own meters or diagnostics showing
/// it (2026-09-05 hardware investigation: about −35 dB). This probe is
/// read-only by design — see `VirtualOutputLevel` for why the level is
/// reported, never restored.
///
/// The controls are read on the input scope first (the slider the user
/// sees for a microphone), falling back to the output scope: BlackHole
/// registers the same single value on both, and a future output-only
/// virtual device would have only the latter. A control that is absent
/// on both scopes reads as nil, which classifies as nominal (fail open).
enum VirtualOutputLevelProbe {
    /// Classified reading for `device`, with the raw scalar for the log.
    static func read(_ device: AudioObjectID) -> VirtualOutputLevel {
        VirtualOutputLevel.classify(
            volumeScalar: readFloat(device, selector: kAudioDevicePropertyVolumeScalar),
            isMuted: readUInt32(device, selector: kAudioDevicePropertyMute).map { $0 != 0 }
        )
    }

    private static let scopes: [AudioObjectPropertyScope] = [
        kAudioObjectPropertyScopeInput, kAudioObjectPropertyScopeOutput
    ]

    private static func readFloat(_ device: AudioObjectID, selector: AudioObjectPropertySelector) -> Float? {
        var value: Float32 = 0
        return readProperty(device, selector: selector, into: &value) ? value : nil
    }

    private static func readUInt32(_ device: AudioObjectID, selector: AudioObjectPropertySelector) -> UInt32? {
        var value: UInt32 = 0
        return readProperty(device, selector: selector, into: &value) ? value : nil
    }

    /// Reads `selector` on the main element of the first scope that has
    /// it; false when neither scope exposes a readable value.
    private static func readProperty<T>(
        _ device: AudioObjectID,
        selector: AudioObjectPropertySelector,
        into value: inout T
    ) -> Bool {
        for scope in scopes {
            var address = AudioObjectPropertyAddress(
                mSelector: selector,
                mScope: scope,
                mElement: kAudioObjectPropertyElementMain
            )
            guard AudioObjectHasProperty(device, &address) else {
                continue
            }
            var byteCount = UInt32(MemoryLayout<T>.size)
            if AudioObjectGetPropertyData(device, &address, 0, nil, &byteCount, &value) == noErr {
                return true
            }
        }
        return false
    }
}

// MARK: - AppState hook

extension AppState {
    private static let levelLog = Logger(
        subsystem: "com.lightsound.noican", category: "engine-diagnostics"
    )

    /// Reads the virtual output's level controls and forwards a *changed*
    /// classification to the reducer, logging each detection (with the
    /// scalar reading, as evidence of how often and how far the slider
    /// moves) and each resolution. Called when an engine start settles
    /// and by the 1 Hz health poll; the reducer accepts it only while a
    /// transport is up, so a reading while Off is dropped before it can
    /// churn the model. The device read is the Noican virtual output
    /// itself (the same pick the aggregate is composed around), never the
    /// private aggregate.
    func checkVirtualOutputLevel() {
        guard
            model.transportSession != nil,
            let device = AudioDeviceCatalog.virtualOutput(in: allDevices)
        else {
            return
        }
        let level = VirtualOutputLevelProbe.read(device.id)
        let shown = model.messages.virtualOutputLevelNotice
        guard level.notice != shown else {
            return
        }
        switch level {
        case let .turnedDown(scalar):
            Self.levelLog.warning(
                """
                Virtual output level: "\(device.name, privacy: .public)" volume scalar \
                \(scalar, format: .fixed(precision: 3), privacy: .public) (below unity) — \
                consumers hear the engine output attenuated; not changed by Noican
                """
            )
        case .muted:
            Self.levelLog.warning(
                "Virtual output level: \"\(device.name, privacy: .public)\" is muted; not changed by Noican"
            )
        case .nominal:
            Self.levelLog.info(
                "Virtual output level: \"\(device.name, privacy: .public)\" back at unity and unmuted"
            )
        }
        dispatch(.virtualOutputLevelObserved(level))
    }
}
