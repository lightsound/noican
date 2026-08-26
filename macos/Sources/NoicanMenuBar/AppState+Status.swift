import Foundation

// MARK: - Status projections for the menu UI

extension AppState {
    /// One-line status for the header. Deliberately never multi-line:
    /// text above the mode control must not change the control's
    /// vertical position (the sliding pill would move mid-animation).
    /// Full failure text lives in `engineErrorMessage`, shown below the
    /// control instead.
    var statusText: String {
        switch phase {
        case .off:
            return inputDevices.isEmpty ? "No input device" : "Off"
        case let .busy(message):
            return message
        case .running:
            let name = displayName(for: activeModelID ?? selectedModel)
            // "Previewing" only while the monitor actually plays; after a
            // trip or a monitor failure the engine still runs but the
            // preview does not.
            let previewing = mode == .preview && previewError == nil
            return previewing ? "Previewing · \(name)" : "Running · \(name)"
        case .failed:
            return "Error"
        }
    }

    /// Full engine failure text, displayed under the mode control (with
    /// the preview messages) so the header height stays constant.
    var engineErrorMessage: String? {
        if case let .failed(message) = phase {
            return message
        }
        return nil
    }

    /// True while the selected mode is not actually delivering (start
    /// failure, runtime stop, or a preview whose monitor is not playing).
    /// The mode control is the user's intent and never moves on its own;
    /// this drives the warning tint that says "you asked for this, but it
    /// is not running".
    var isModeUnfulfilled: Bool {
        engineErrorMessage != nil || (mode == .preview && previewError != nil)
    }

    func displayName(for modelID: String) -> String {
        models.first { $0.id == modelID }?.displayName ?? modelID
    }
}
