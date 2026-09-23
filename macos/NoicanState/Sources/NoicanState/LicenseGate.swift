extension AppReducer {
    static let licenseRequiredMessage = "Noican needs a license to run — enter your license key below."

    /// Records whether Preview/On may be started. Deliberately no effect
    /// when the license lapses mid-session: the check runs in the
    /// background (hourly), and cutting the microphone off in the middle
    /// of a call is worse than one more session.
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

    /// The gate on engine starts, for every path that reaches one (mode
    /// taps, microphone picks, rate-change rebuilds, the failed-switch
    /// fallback). Without a license only a rebuild of a session that is
    /// still live passes — the one exemption: a running session is never
    /// interrupted. A session that already died (runtime stop, failed
    /// start) is not restarted. Returns whether the start was refused;
    /// the refusal leaves the machine and the torn-down engine as they
    /// are.
    static func refusesStartWithoutLicense(_ state: inout AppModel) -> Bool {
        guard !state.isProcessingAllowed, !rebuildsLiveSession(state) else {
            return false
        }
        state.messages.licenseRequired = licenseRequiredMessage
        return true
    }

    /// Whether a start claimed from `state` replaces a live transport:
    /// a settled session that is still up, or the fallback of a live
    /// microphone switch (claimed while that switch's attempt, which
    /// carried the working device to return to, is still in flight).
    private static func rebuildsLiveSession(_ state: AppModel) -> Bool {
        if state.liveSession != nil {
            return true
        }
        if case let .busy(.starting(attempt), _) = state.machine {
            return attempt.revertInputUID != nil
        }
        return false
    }
}
