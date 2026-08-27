import Testing

@testable import NoicanState

// Reducer coverage for the Phase 2 preference features: restoration at
// launch, the strength (dry/wet intensity) slider, and the login-item
// toggle. Fixtures and drivers are shared with the other reducer suites;
// see ReducerTestSupport.swift.

// MARK: - Preference restoration

@Suite("Preference restoration at launch")
struct PreferenceRestorationTests {
    @Test("A restore applies model, microphone, and intensity while Off")
    func restoreApplies() {
        let (state, effects) = step(
            readyModel(),
            .preferencesRestored(modelID: "dfn3", inputUID: usbMic.uid, intensity: 0.6)
        )
        #expect(state.selectedModelID == "dfn3")
        #expect(state.selectedInputUID == usbMic.uid)
        #expect(state.intensity == 0.6)
        #expect(effects == [.setIntensity(0.6)], "the engine atomic is seeded")
        #expect(state.mode == .off, "the mode is never restored")
        #expect(state.machine == .settled(.off), "no engine transition is claimed")
    }

    @Test("Nil fields and a disconnected stored microphone are skipped")
    func restoreSkipsInvalid() {
        let (state, effects) = step(
            readyModel(),
            .preferencesRestored(modelID: nil, inputUID: "gone-device", intensity: nil)
        )
        #expect(state.selectedModelID == "fastenhancer-b", "nil model keeps the default")
        #expect(state.selectedInputUID == builtInMic.uid, "an absent microphone keeps the default")
        #expect(state.intensity == 1.0)
        #expect(effects.isEmpty)
    }

    @Test("A restore is ignored once the user has chosen a mode")
    func restoreIgnoredAfterUserIntent() {
        let running = runningModel()
        let (state, effects) = step(
            running,
            .preferencesRestored(modelID: "dfn3", inputUID: usbMic.uid, intensity: 0.2)
        )
        #expect(state == running, "a late restore must not disturb a session")
        #expect(effects.isEmpty)
    }

    @Test("A restored intensity is clamped like a slider change")
    func restoreClampsIntensity() {
        let (state, effects) = step(
            readyModel(),
            .preferencesRestored(modelID: nil, inputUID: nil, intensity: 7.5)
        )
        #expect(state.intensity == 1.0)
        #expect(effects == [.setIntensity(1.0)])
    }
}

// MARK: - Intensity (strength slider)

@Suite("Intensity control")
struct IntensityTests {
    @Test("A slider move updates the state and emits the atomic write")
    func sliderMoveApplies() {
        let (state, effects) = step(readyModel(), .intensityChanged(0.4))
        #expect(state.intensity == 0.4)
        #expect(effects == [.setIntensity(0.4)])
        #expect(state.machine == .settled(.off), "no engine transition is claimed")
    }

    @Test("Out-of-range values clamp; non-finite values are ignored")
    func sliderClamps() {
        let high = step(readyModel(), .intensityChanged(3.0))
        #expect(high.state.intensity == 1.0)
        #expect(high.effects.isEmpty, "already at 1.0 — nothing to publish")
        let low = step(readyModel(), .intensityChanged(-2.0))
        #expect(low.state.intensity == 0.0)
        #expect(low.effects == [.setIntensity(0.0)])
        let bogus = step(readyModel(), .intensityChanged(.nan))
        #expect(bogus.state == readyModel())
        #expect(bogus.effects.isEmpty)
    }

    @Test("The slider stays live while busy and while running")
    func sliderWorksWhileBusyAndRunning() {
        let busy = drive(readyModel(), [tap(.on)])
        let (state, effects) = step(busy, .intensityChanged(0.25))
        #expect(state.intensity == 0.25)
        #expect(effects == [.setIntensity(0.25)], "an atomic write needs no serialization")
        #expect(state.machine == busy.machine, "the in-flight transition is untouched")

        let running = runningModel()
        let (liveState, liveEffects) = step(running, .intensityChanged(0.8))
        #expect(liveState.intensity == 0.8)
        #expect(liveEffects == [.setIntensity(0.8)])
        #expect(liveState.machine == running.machine)
    }
}

// MARK: - Launch at login

@Suite("Login item registration")
struct LaunchAtLoginTests {
    @Test("The status read at launch seeds the toggle")
    func statusReadSeeds() {
        let (state, effects) = step(readyModel(), .launchAtLoginStatusRead(isEnabled: true))
        #expect(state.isLaunchAtLoginEnabled)
        #expect(effects.isEmpty)
    }

    @Test("A toggle moves optimistically and requests the registration")
    func toggleIsOptimistic() {
        let (state, effects) = step(readyModel(), .launchAtLoginToggled(true))
        #expect(state.isLaunchAtLoginEnabled)
        #expect(state.isLaunchAtLoginBusy, "the attempt claims the serialization gate")
        #expect(effects == [.setLaunchAtLogin(enabled: true)])

        let settled = drive(state, [.launchAtLoginChangeCompleted(isEnabled: true, error: nil)])
        #expect(settled.isLaunchAtLoginEnabled)
        #expect(!settled.isLaunchAtLoginBusy, "the completion releases the gate")
        #expect(settled.messages.launchAtLoginError == nil)
    }

    @Test("Rapid re-toggles are serialized: one attempt in flight at a time")
    func rapidTogglesAreSerialized() {
        // Flip on, then flip off before the registration settles: the
        // second flip must be ignored (two concurrent register/unregister
        // calls would race and the toggle would settle on whichever
        // completion arrived last).
        let requested = drive(readyModel(), [.launchAtLoginToggled(true)])
        let (whileBusy, busyEffects) = step(requested, .launchAtLoginToggled(false))
        #expect(whileBusy == requested, "a flip while busy changes nothing")
        #expect(busyEffects.isEmpty, "no second registration attempt is spawned")

        // A status read racing the attempt is stale and ignored too.
        let read = drive(requested, [.launchAtLoginStatusRead(isEnabled: false)])
        #expect(read.isLaunchAtLoginEnabled, "the in-flight attempt owns the toggle")

        // After the completion, toggling works again.
        let settled = drive(requested, [.launchAtLoginChangeCompleted(isEnabled: true, error: nil)])
        let (next, nextEffects) = step(settled, .launchAtLoginToggled(false))
        #expect(next.isLaunchAtLoginBusy)
        #expect(nextEffects == [.setLaunchAtLogin(enabled: false)])
    }

    @Test("A completion without a claimed attempt is stale and ignored")
    func staleCompletionIgnored() {
        let (state, effects) = step(
            readyModel(),
            .launchAtLoginChangeCompleted(isEnabled: true, error: nil)
        )
        #expect(state == readyModel())
        #expect(effects.isEmpty)
    }

    @Test("A failed registration snaps the toggle back and explains why")
    func failureRevertsToggle() {
        let requested = drive(readyModel(), [.launchAtLoginToggled(true)])
        let (state, effects) = step(
            requested,
            .launchAtLoginChangeCompleted(isEnabled: false, error: "Operation not permitted")
        )
        #expect(!state.isLaunchAtLoginEnabled, "the toggle shows the re-read real status")
        #expect(state.messages.launchAtLoginError == "Operation not permitted")
        #expect(effects.isEmpty)

        // The next attempt clears the stale reason while in flight.
        let retried = drive(state, [.launchAtLoginToggled(true)])
        #expect(retried.messages.launchAtLoginError == nil)
    }

    @Test("Re-toggling to the current state is a no-op")
    func sameValueIgnored() {
        let (state, effects) = step(readyModel(), .launchAtLoginToggled(false))
        #expect(state == readyModel())
        #expect(effects.isEmpty)
    }
}
