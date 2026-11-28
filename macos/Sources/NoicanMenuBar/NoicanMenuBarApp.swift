import AppKit
import NoicanState

@main
@MainActor
enum NoicanMenuBarApp {
    private static let delegate = AppDelegate()

    static func main() {
        let app = NSApplication.shared
        app.delegate = delegate
        app.mainMenu = mainMenu()
        app.run()
    }

    private static func mainMenu() -> NSMenu {
        let appMenu = NSMenu()
        appMenu.addItem(
            withTitle: "Quit Noican",
            action: #selector(NSApplication.terminate(_:)),
            keyEquivalent: "q"
        )
        let editMenu = NSMenu(title: "Edit")
        editMenu.addItem(withTitle: "Undo", action: Selector(("undo:")), keyEquivalent: "z")
        editMenu.addItem(withTitle: "Redo", action: Selector(("redo:")), keyEquivalent: "Z")
        editMenu.addItem(.separator())
        editMenu.addItem(withTitle: "Cut", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
        editMenu.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        editMenu.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        editMenu.addItem(withTitle: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
        let menu = NSMenu()
        for submenu in [appMenu, editMenu] {
            let item = NSMenuItem()
            item.submenu = submenu
            menu.addItem(item)
        }
        return menu
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var statusBar: StatusBarController?

    func applicationDidFinishLaunching(_ notification: Notification) {
        statusBar = StatusBarController()
    }
}

@MainActor
enum MenuBarIcon {
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

    private static let icons: [String: NSImage] = {
        var icons: [String: NSImage] = [:]
        for mode in EngineMode.allCases {
            let name = symbolName(for: mode)
            icons["\(mode.rawValue)-false"] = canvas(name, tint: mode == .off ? nil : .systemGreen)
            icons["\(mode.rawValue)-true"] = canvas(name, tint: .systemRed)
        }
        return icons
    }()

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
