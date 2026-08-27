import AppKit
import NoicanState
import SwiftUI

@main
struct NoicanMenuBarApp: App {
    @StateObject private var state = AppState()

    var body: some Scene {
        MenuBarExtra {
            MenuView(state: state)
        } label: {
            MenuBarIcon(mode: state.model.mode, isUnfulfilled: state.model.isModeUnfulfilled)
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
///
/// Every state draws into the same fixed-size canvas: the glyphs have
/// different intrinsic widths, and a status item that changes width
/// shifts the popover's anchor on every mode change.
private struct MenuBarIcon: View {
    let mode: EngineMode
    let isUnfulfilled: Bool

    var body: some View {
        Image(nsImage: Self.icons["\(mode.rawValue)-\(isUnfulfilled)"] ?? NSImage())
            .accessibilityLabel("Noican")
    }

    private static func symbolName(for mode: EngineMode) -> String {
        switch mode {
        case .off: "mic.slash"
        case .preview: "headphones"
        case .on: "waveform.badge.mic"
        }
    }

    /// One prerendered image per mode × health, keyed
    /// "<mode>-<isUnfulfilled>". Off is a template image (the system
    /// adapts it to the menu bar appearance); colored states are
    /// non-template with palette colors, because status items render
    /// template images tint-free.
    private static let icons: [String: NSImage] = {
        var icons: [String: NSImage] = [:]
        for mode in EngineMode.allCases {
            let name = symbolName(for: mode)
            icons["\(mode.rawValue)-false"] = canvas(name, tint: mode == .off ? nil : .systemGreen)
            icons["\(mode.rawValue)-true"] = canvas(name, tint: .systemRed)
        }
        return icons
    }()

    /// Draws `symbolName` centered on a constant-size canvas; `nil` tint
    /// produces a template image.
    private static func canvas(_ symbolName: String, tint: NSColor?) -> NSImage {
        var configuration = NSImage.SymbolConfiguration(pointSize: 13, weight: .regular)
        if let tint {
            configuration = configuration
                .applying(NSImage.SymbolConfiguration(paletteColors: [tint]))
        }
        let symbol = NSImage(
            systemSymbolName: symbolName,
            accessibilityDescription: "Noican"
        )?.withSymbolConfiguration(configuration) ?? NSImage()
        // System symbols are template images; drawn directly, a template
        // is treated as a monochrome mask and the palette tint would not
        // survive the bake. The canvas re-applies templateness for the
        // untinted (Off) state below.
        symbol.isTemplate = false
        let size = NSSize(width: 22, height: 16)
        let image = NSImage(size: size, flipped: false) { rect in
            symbol.draw(
                at: NSPoint(
                    x: (rect.width - symbol.size.width) / 2,
                    y: (rect.height - symbol.size.height) / 2
                ),
                from: .zero,
                operation: .sourceOver,
                fraction: 1
            )
            return true
        }
        image.isTemplate = tint == nil
        return image
    }
}
