import CoreAudio
import Foundation
import NoicanState

enum VirtualOutputLevelProbe {
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
            let status = withUnsafeMutablePointer(to: &value) { pointer in
                AudioObjectGetPropertyData(device, &address, 0, nil, &byteCount, pointer)
            }
            if status == noErr {
                return true
            }
        }
        return false
    }
}

extension AppState {
    func checkVirtualOutputLevel() {
        guard
            model.transportSession != nil,
            let device = AudioDeviceCatalog.virtualOutput(in: allDevices)
        else {
            return
        }
        let level = VirtualOutputLevelProbe.read(device.id)
        diagnostics.recordVirtualOutputLevel(level, deviceName: device.name)
        guard level != model.virtualOutputLevel else {
            return
        }
        dispatch(.virtualOutputLevelObserved(level))
    }
}
