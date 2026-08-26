import AppKit
import SwiftUI

@main
struct NoicanMenuBarApp: App {
    @StateObject private var state = AppState()

    var body: some Scene {
        MenuBarExtra {
            MenuView(state: state)
        } label: {
            MenuBarIcon(mode: state.mode, isUnfulfilled: state.isModeUnfulfilled)
        }
        .menuBarExtraStyle(.window)
    }
}

/// Menu bar glyph per mode: `mic.slash` (Off), a green `waveform.badge.mic`
/// (On — the color says "actively cleaning"), and `headphones` (Preview,
/// the state worth noticing from the menu bar: you are hearing yourself).
/// While the selected mode is not actually delivering, the On glyph stays
/// monochrome so green always means "really running".
private struct MenuBarIcon: View {
    let mode: EngineMode
    let isUnfulfilled: Bool

    var body: some View {
        icon.accessibilityLabel("Noican")
    }

    @ViewBuilder
    private var icon: some View {
        switch mode {
        case .off:
            Image(systemName: "mic.slash")
        case .preview:
            Image(systemName: "headphones")
        case .on:
            if isUnfulfilled {
                Image(systemName: "waveform.badge.mic")
            } else {
                // Status items render template images tint-free; a colored
                // glyph needs a non-template NSImage with palette colors.
                Image(nsImage: Self.activeIcon)
            }
        }
    }

    private static let activeIcon: NSImage = {
        let configuration = NSImage.SymbolConfiguration(pointSize: 13, weight: .regular)
            .applying(NSImage.SymbolConfiguration(paletteColors: [.systemGreen]))
        let image = NSImage(
            systemSymbolName: "waveform.badge.mic",
            accessibilityDescription: "Noican on"
        )?.withSymbolConfiguration(configuration) ?? NSImage()
        image.isTemplate = false
        return image
    }()
}
