extension AppReducer {
    static let licenseRequiredMessage = "Noican needs a license to run — enter your license key below."

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

    static func refusesWithoutLicense(_ state: inout AppModel, _ newMode: EngineMode) -> Bool {
        state.messages.licenseRequired = nil
        guard newMode != .off, !state.isProcessingAllowed else {
            return false
        }
        state.messages.licenseRequired = licenseRequiredMessage
        return true
    }

    static func refusesStartWithoutLicense(_ state: inout AppModel) -> Bool {
        guard !state.isProcessingAllowed, !rebuildsLiveSession(state) else {
            return false
        }
        state.messages.licenseRequired = licenseRequiredMessage
        return true
    }

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
