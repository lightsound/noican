extension AppReducer {
    static let licenseRequiredMessage = "Noican needs a license to run — enter your license key below."

    /// Records whether Preview/On may be started. Deliberately no effect
    /// when the license lapses mid-session: the check runs in the
    /// background (hourly, after wake), and cutting the microphone off in
    /// the middle of a call is worse than one more session.
    static func processingAllowanceChanged(
        _ state: AppModel,
        _ isAllowed: Bool
    ) -> (state: AppModel, effects: [AppEffect]) {
        var state = state
        state.isProcessingAllowed = isAllowed
        if isAllowed {
            state.messages.licenseRequired = nil
        }
        return (state, [])
    }

    /// The gate on mode taps: a Preview/On tap without a license is
    /// refused in place like an unsafe Preview (mode and engine stay as
    /// they are, the reason shows under the control); Off always passes.
    /// Returns whether the tap was refused.
    static func refusesWithoutLicense(_ state: inout AppModel, _ newMode: EngineMode) -> Bool {
        state.messages.licenseRequired = nil
        guard newMode != .off, !state.isProcessingAllowed else {
            return false
        }
        state.messages.licenseRequired = licenseRequiredMessage
        return true
    }
}
