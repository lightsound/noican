import AppKit
import NoicanState
import SwiftUI

/// The app is an AppKit status-item shell around the SwiftUI menu.
///
/// It deliberately does *not* use `MenuBarExtra(.window)`: that scene
/// resizes its popover window around the AppKit bottom-left origin with
/// its own repositioning heuristics, so any content-height change (the
/// "Model & strength" section collapsing, the monitoring section
/// appearing) could visibly drop or shift the whole menu, and nothing
/// outside the framework can deterministically prevent it. Owning the
/// status item and the panel (see `StatusBarController`) makes the
/// geometry a pure function of the status item's position: the top edge
/// never moves, and height changes only ever grow or shrink downward.
@main
struct NoicanMenuBarApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate

    var body: some Scene {
        // The menu lives in the AppKit panel; SwiftUI still requires a
        // Scene, and an empty Settings scene contributes no UI.
        Settings {}
    }
}

/// Boots the status-item shell once AppKit is ready.
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var statusBar: StatusBarController?

    func applicationDidFinishLaunching(_ notification: Notification) {
        statusBar = StatusBarController()
    }
}

/// Menu bar icon renderer: the glyph is the selected mode (`mic.slash`
/// Off, `waveform.badge.mic` On, `headphones` Preview), the color is its
/// health — monochrome Off, green while the mode is actually delivering,
/// red (matching the status dot and the pill's warning tint) while the
/// selection is not delivering, so trouble is visible without opening
/// the menu.
///
/// Every state draws into the same fixed-size canvas: the glyphs have
/// different intrinsic widths, and a status item that changes width
/// shifts the menu's anchor on every mode change.
///
/// Main-actor bound: the image cache is UI state and every caller is
/// the status-item controller.
@MainActor
enum MenuBarIcon {
    /// The prerendered status-item image for a mode × health state.
    static func image(mode: EngineMode, isUnfulfilled: Bool) -> NSImage {
        icons["\(mode.rawValue)-\(isUnfulfilled)"] ?? NSImage()
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
