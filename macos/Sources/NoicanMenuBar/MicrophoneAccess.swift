import AVFoundation

enum MicrophoneAccess {
    static let deniedMessage = "Microphone access is off for Noican — allow it in System Settings › "
        + "Privacy & Security › Microphone, then select the mode again."

    static func isGranted() async -> Bool {
        switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .authorized:
            return true
        case .notDetermined:
            return await AVCaptureDevice.requestAccess(for: .audio)
        case .denied, .restricted:
            return false
        @unknown default:
            return false
        }
    }
}
