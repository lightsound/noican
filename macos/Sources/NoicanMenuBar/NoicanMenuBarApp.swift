import SwiftUI

@main
struct NoicanMenuBarApp: App {
    @StateObject private var state = AppState()

    var body: some Scene {
        MenuBarExtra {
            MenuView(state: state)
        } label: {
            Label(
                "noican",
                systemImage: state.isEnabled ? "waveform.badge.mic" : "mic.slash"
            )
        }
        .menuBarExtraStyle(.window)
    }
}
