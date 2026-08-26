import Foundation

// MARK: - Status projections for the menu UI

extension AppState {
    /// One-line status for the header. `phase` is settled by definition,
    /// so transitions do not churn this text (the spinner is the busy
    /// feedback). Deliberately never multi-line: text above the mode
    /// control must not change the control's vertical position (the
    /// sliding pill would move mid-animation). Full failure text lives in
    /// `engineErrorMessage`, shown below the control instead.
    var statusText: String {
        switch phase {
        case .off:
            return inputDevices.isEmpty ? "No input device" : "Off"
        case .running:
            // No model name here: the Model picker below already shows it
            // (and reverts on failed switches, so it never lies).
            // "Previewing" only while the monitor actually plays; after a
            // trip or a monitor failure the engine still runs but the
            // preview does not.
            return mode == .preview && previewError == nil ? "Previewing" : "Running"
        case .failed:
            return "Error"
        }
    }

    /// Full engine failure text, displayed under the mode control (with
    /// the preview messages) so the header height stays constant. It
    /// stays visible while a retry is in flight and clears only when the
    /// retry actually succeeds.
    var engineErrorMessage: String? {
        if case let .failed(message) = phase {
            return message
        }
        return nil
    }

    /// Whether the monitoring section (level meters) is shown: only for
    /// a settled, healthy run, so the section neither slides in during a
    /// start that may still fail nor disappears during a model switch.
    var showsMonitoring: Bool {
        mode != .off && phase == .running
    }

    /// True while the selected mode is not actually delivering (start
    /// failure, runtime stop, or a preview whose monitor is not playing).
    /// The mode control is the user's intent and never moves on its own;
    /// this drives the warning tint that says "you asked for this, but it
    /// is not running".
    var isModeUnfulfilled: Bool {
        engineErrorMessage != nil || (mode == .preview && previewError != nil)
    }

    /// Why `uid` cannot serve as the engine's microphone, or nil when it
    /// can. Reads the device's advertised sample rates only — no audio
    /// object is created, so this is safe to call before any transition
    /// (Bluetooth headset microphones advertise telephony rates only and
    /// are the common refusal; see issue #7).
    func microphoneCapabilityError(for uid: String) -> String? {
        guard let device = inputDevices.first(where: { $0.uid == uid }) else {
            return nil
        }
        guard AudioDeviceCatalog.supportsSampleRate(device.id, 48_000) else {
            return "The microphone \"\(device.name)\" can't run at 48 kHz "
                + "(Bluetooth headset mics use telephony rates) — choose another microphone."
        }
        return nil
    }
}
