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

/// Menu bar icon: the glyph is the selected mode (`mic.slash` Off,
/// `waveform.badge.mic` On, `headphones` Preview), the color is its
/// health — monochrome Off, green while the mode is actually delivering,
/// red (matching the status dot and the pill's warning tint) while the
/// selection is not delivering, so trouble is visible without opening
/// the popover.
private struct MenuBarIcon: View {
    let mode: EngineMode
    let isUnfulfilled: Bool

    var body: some View {
        icon.accessibilityLabel("Noican")
    }

    @ViewBuilder
    private var icon: some View {
        if isUnfulfilled {
            Image(nsImage: Self.redIcon(for: mode))
        } else {
            switch mode {
            case .off:
                Image(systemName: "mic.slash")
            case .preview:
                Image(nsImage: Self.greenHeadphones)
            case .on:
                Image(nsImage: Self.greenWaveform)
            }
        }
    }

    private static let greenWaveform = colored("waveform.badge.mic", .systemGreen)
    private static let greenHeadphones = colored("headphones", .systemGreen)
    private static let redWaveform = colored("waveform.badge.mic", .systemRed)
    private static let redHeadphones = colored("headphones", .systemRed)
    private static let redMicSlash = colored("mic.slash", .systemRed)

    private static func redIcon(for mode: EngineMode) -> NSImage {
        switch mode {
        case .off: redMicSlash
        case .preview: redHeadphones
        case .on: redWaveform
        }
    }

    /// Status items render template images tint-free; a colored glyph
    /// needs a non-template `NSImage` with palette colors.
    private static func colored(_ symbolName: String, _ color: NSColor) -> NSImage {
        let configuration = NSImage.SymbolConfiguration(pointSize: 13, weight: .regular)
            .applying(NSImage.SymbolConfiguration(paletteColors: [color]))
        let image = NSImage(
            systemSymbolName: symbolName,
            accessibilityDescription: "Noican"
        )?.withSymbolConfiguration(configuration) ?? NSImage()
        image.isTemplate = false
        return image
    }
}
