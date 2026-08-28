import AppKit
import Combine
import SwiftUI

/// Owns the status item and shows the menu in a native `NSPopover`.
///
/// This is the classic status-item + popover architecture, chosen after
/// two failed shells: `MenuBarExtra(.window)` and a hand-rolled panel
/// both fell to the same root cause — `NSHostingView` automatically
/// resizes its window around the AppKit *bottom-left* origin, so any
/// content-height reduction dropped the whole menu (and fighting that
/// resize with manual frame corrections raced it, glitching layout).
/// `NSPopover` solves the geometry natively: its content size follows
/// the hosting controller's `preferredContentSize` and resizes anchored
/// to the status item, growing and shrinking downward only. It also
/// restores the system menu chrome — material, corner radius, shadow —
/// and the native dismissal contract (click outside, Escape) instead of
/// reimplementations of them.
///
/// The menu content is built fresh on every open and released on close,
/// which scopes the 20 Hz level poll (`MenuView.task`) exactly to the
/// time the menu is visible.
@MainActor
final class StatusBarController: NSObject {
    private let state = AppState()
    private let statusItem: NSStatusItem
    private var popover: NSPopover?
    private var iconSubscription: AnyCancellable?
    /// When the menu last closed. A click on the status item while the
    /// menu is open dismisses the transient popover on mouse-down and
    /// then fires the button action on mouse-up; ignoring an open
    /// request right after a close keeps that from reopening the menu.
    private var lastCloseTime: TimeInterval = 0

    override init() {
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        super.init()
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
        if popover == nil {
            openMenu()
        } else {
            closeMenu()
        }
    }

    private func openMenu() {
        guard
            ProcessInfo.processInfo.systemUptime - lastCloseTime > 0.25,
            let button = statusItem.button
        else {
            return
        }
        let hosting = NSHostingController(rootView: MenuView(state: state))
        // The popover tracks the SwiftUI content's ideal size through
        // preferredContentSize — the one window-sizing path that is
        // anchored to the status item instead of the bottom-left origin.
        hosting.sizingOptions = .preferredContentSize
        hosting.view.layoutSubtreeIfNeeded()
        let popover = NSPopover()
        popover.behavior = .transient
        popover.animates = false
        popover.delegate = self
        popover.contentViewController = hosting
        self.popover = popover
        // The app must be active while the menu shows (MenuBarExtra did
        // this internally): an inactive app renders the popover with the
        // dimmed inactive-window materials, and transient dismissal on
        // outside clicks only works reliably for the active app.
        NSApp.activate()
        popover.show(relativeTo: button.bounds, of: button, preferredEdge: .minY)
        popover.contentViewController?.view.window?.makeKey()
    }

    private func closeMenu() {
        popover?.performClose(nil)
    }
}

extension StatusBarController: NSPopoverDelegate {
    /// Covers every close path (toggle, click outside, Escape): drop the
    /// popover and its hosting controller so SwiftUI tears the content
    /// down (cancelling MenuView's level-poll task), and stamp the time
    /// for the reopen guard.
    func popoverDidClose(_ notification: Notification) {
        lastCloseTime = ProcessInfo.processInfo.systemUptime
        popover = nil
    }
}
