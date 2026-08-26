import SwiftUI

@main
struct NoicanMenuBarApp: App {
    @StateObject private var state = AppState()

    var body: some Scene {
        MenuBarExtra {
            MenuView(state: state)
        } label: {
            Label(
                "Noican",
                systemImage: state.mode == .off ? "mic.slash" : "waveform.badge.mic"
            )
        }
        .menuBarExtraStyle(.window)
    }
}
