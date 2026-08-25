import SwiftUI

@main
struct NoicanMenuBarApp: App {
    @StateObject private var state = AppState()
    private let statusTimer = Timer.publish(every: 1, on: .main, in: .common).autoconnect()

    var body: some Scene {
        MenuBarExtra {
            MenuView(state: state)
                .onReceive(statusTimer) { _ in
                    state.updateStatus()
                }
        } label: {
            Label(
                "noican",
                systemImage: state.isEnabled ? "waveform.badge.mic" : "mic.slash"
            )
        }
        .menuBarExtraStyle(.window)
    }
}
