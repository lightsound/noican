import AppKit
import Combine
import SwiftUI

/// Owns the status item and the menu panel, replacing
/// `MenuBarExtra(.window)` so the menu's geometry is deterministic.
///
/// The panel's frame is a pure function of the status item: the top
/// edge sits a fixed gap below it and **never moves**; every
/// content-size change (sections expanding and collapsing, error text
/// appearing) re-derives the frame from that anchor, so the menu only
/// ever grows and shrinks downward. The menu content is built fresh on
/// every open and torn down on close, which also scopes the 20 Hz level
/// poll (`MenuView.task`) exactly to the time the menu is visible.
@MainActor
final class StatusBarController {
    private let state = AppState()
    private let statusItem: NSStatusItem
    private var panel: MenuPanel?
    private var hosting: NSHostingView<MenuRoot>?
    private var resignObserver: (any NSObjectProtocol)?
    private var iconSubscription: AnyCancellable?
    /// Screen Y of the menu's fixed top edge, captured at open time.
    private var topEdge: CGFloat = 0
    /// When the menu last closed. A click on the status item can close
    /// the menu twice over (panel resigns key, then the button action
    /// toggles); ignoring an open request right after a close keeps
    /// that from reopening the menu.
    private var lastCloseTime: TimeInterval = 0

    /// Gap between the menu bar and the panel's top edge.
    private static let menuBarGap: CGFloat = 5
    /// Minimum distance kept from the screen's edges.
    private static let screenMargin: CGFloat = 8

    init() {
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        if let button = statusItem.button {
            button.target = self
            button.action = #selector(toggleMenu)
            button.image = MenuBarIcon.image(mode: state.model.mode, isUnfulfilled: state.model.isModeUnfulfilled)
        }
        // Follow the reducer state so trouble is visible on the icon
        // without opening the menu.
        iconSubscription = state.$model
            .map { (mode: $0.mode, isUnfulfilled: $0.isModeUnfulfilled) }
            .removeDuplicates { $0 == $1 }
            .sink { [weak self] health in
                // @Published emits on the main actor (AppState is bound
                // to it); the annotation gap is bridged here.
                MainActor.assumeIsolated {
                    self?.statusItem.button?.image =
                        MenuBarIcon.image(mode: health.mode, isUnfulfilled: health.isUnfulfilled)
                }
            }
    }

    @objc private func toggleMenu() {
        if panel == nil {
            openMenu()
        } else {
            closeMenu()
        }
    }

    private func openMenu() {
        guard
            ProcessInfo.processInfo.systemUptime - lastCloseTime > 0.25,
            let button = statusItem.button,
            let anchorWindow = button.window,
            let screen = anchorWindow.screen ?? NSScreen.main
        else {
            return
        }
        let hosting = NSHostingView(rootView: MenuRoot(state: state) { [weak self] size in
            self?.contentSizeChanged(to: size)
        })
        hosting.layoutSubtreeIfNeeded()
        let size = hosting.fittingSize
        let panel = MenuPanel()
        panel.onCancel = { [weak self] in
            self?.closeMenu()
        }
        panel.contentView = hosting
        self.hosting = hosting
        self.panel = panel

        // The anchor: a fixed gap below the status item, horizontally
        // centered on it and clamped to the screen.
        topEdge = anchorWindow.frame.minY - Self.menuBarGap
        let visible = screen.visibleFrame
        let lowestX = visible.minX + Self.screenMargin
        let highestX = visible.maxX - size.width - Self.screenMargin
        let originX = min(max(anchorWindow.frame.midX - size.width / 2, lowestX), max(lowestX, highestX))
        panel.setFrame(
            NSRect(x: originX, y: topEdge - size.height, width: size.width, height: size.height),
            display: false
        )
        panel.makeKeyAndOrderFront(nil)
        // A non-activating key panel resigns key the moment the user
        // clicks anywhere else (another app, the desktop, the status
        // bar) — the same dismissal contract the system menu has.
        resignObserver = NotificationCenter.default.addObserver(
            forName: NSWindow.didResignKeyNotification,
            object: panel,
            queue: .main
        ) { [weak self] _ in
            // Delivered on the main queue, which is the main actor.
            MainActor.assumeIsolated {
                self?.closeMenu()
            }
        }
    }

    private func closeMenu() {
        lastCloseTime = ProcessInfo.processInfo.systemUptime
        if let resignObserver {
            NotificationCenter.default.removeObserver(resignObserver)
            self.resignObserver = nil
        }
        guard let panel else {
            return
        }
        self.panel = nil
        // Detach the content before ordering out so SwiftUI tears the
        // view down (cancelling MenuView's level-poll task) even though
        // AppKit keeps the panel object alive until autorelease.
        panel.contentView = NSView()
        hosting = nil
        panel.orderOut(nil)
    }

    /// Re-derives the frame from the fixed top edge whenever the SwiftUI
    /// content changes size: the top never moves, so expansion and
    /// collapse only ever grow or shrink the menu downward.
    private func contentSizeChanged(to size: CGSize) {
        guard let panel, size.width > 0, size.height > 0 else {
            return
        }
        var frame = panel.frame
        guard frame.size != size || frame.maxY != topEdge else {
            return
        }
        frame.origin.y = topEdge - size.height
        frame.size = size
        panel.setFrame(frame, display: true)
    }
}

/// Borderless, non-activating panel that hosts the menu: it can become
/// key without activating the app (so sliders and shortcuts work while
/// the previous app keeps focus) and closes itself on Escape via
/// `cancelOperation`.
final class MenuPanel: NSPanel {
    var onCancel: (@MainActor () -> Void)?

    init() {
        super.init(
            contentRect: .zero,
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: false
        )
        isOpaque = false
        backgroundColor = .clear
        hasShadow = true
        level = .popUpMenu
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        animationBehavior = .none
        isMovable = false
        hidesOnDeactivate = false
    }

    // Borderless windows refuse key status by default; the menu needs it
    // for keyboard handling (Escape, the Quit shortcut) and slider drags.
    override var canBecomeKey: Bool { true }

    override func cancelOperation(_ sender: Any?) {
        onCancel?()
    }
}

/// Root of the panel's SwiftUI content: the menu plus its own popover
/// chrome (material, rounded corners) — a borderless panel draws no
/// background of its own — and the size reporting that drives the
/// panel's top-anchored frame.
struct MenuRoot: View {
    let state: AppState
    let sizeChanged: @MainActor (CGSize) -> Void

    var body: some View {
        MenuView(state: state)
            .background {
                GeometryReader { proxy in
                    Color.clear.preference(key: MenuSizePreference.self, value: proxy.size)
                }
            }
            .background(MenuChrome())
            .clipShape(RoundedRectangle(cornerRadius: 13, style: .continuous))
            .onPreferenceChange(MenuSizePreference.self) { size in
                // The macOS 15 SDK marks this closure @Sendable while
                // preference changes are delivered on the main actor;
                // assumeIsolated bridges the annotation gap.
                MainActor.assumeIsolated {
                    sizeChanged(size)
                }
            }
    }
}

/// The menu content's size, reported to the panel controller.
private struct MenuSizePreference: PreferenceKey {
    static let defaultValue: CGSize = .zero

    static func reduce(value: inout CGSize, nextValue: () -> CGSize) {
        let next = nextValue()
        if next != .zero {
            value = next
        }
    }
}

/// The popover material backdrop the borderless panel does not provide.
private struct MenuChrome: NSViewRepresentable {
    func makeNSView(context: Context) -> NSVisualEffectView {
        let view = NSVisualEffectView()
        view.material = .popover
        view.blendingMode = .behindWindow
        view.state = .active
        return view
    }

    func updateNSView(_ nsView: NSVisualEffectView, context: Context) {}
}
