import SwiftUI

/// The menu bar app.
///
/// `MenuBarExtra` with no `WindowGroup`: the app has no window, and the
/// `LSUIElement` key in the generated bundle's `Info.plist` keeps it out of the
/// Dock and the app switcher.
@main
struct NoicanMenuBarApp: App {
    @State private var controller = EngineController()

    var body: some Scene {
        MenuBarExtra {
            MenuContent(controller: controller)
        } label: {
            Label("noican", systemImage: menuBarSymbol)
        }
        .menuBarExtraStyle(.window)
    }

    /// The icon says, at a glance, whether the microphone is being cleaned.
    private var menuBarSymbol: String {
        if !controller.status.running {
            return "waveform.slash"
        }
        return controller.status.bypassed ? "mic" : "mic.badge.plus"
    }
}
