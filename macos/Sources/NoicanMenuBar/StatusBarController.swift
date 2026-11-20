import AppKit
import Combine
import SwiftUI

@MainActor
final class StatusBarController: NSObject {
    private let state: AppState
    private let license: LicenseModel
    private let statusItem: NSStatusItem
    private var popover: NSPopover?
    private var iconSubscription: AnyCancellable?
    private var lastCloseTime: TimeInterval = 0

    override init() {
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        let state = AppState()
        self.state = state
        license = LicenseModel(onAllowanceChange: { state.setProcessingAllowed($0) })
        super.init()
        if let button = statusItem.button {
            button.target = self
            button.action = #selector(toggleMenu)
            button.image = MenuBarIcon.image(mode: state.model.mode, isUnfulfilled: state.model.isModeUnfulfilled)
        }
        iconSubscription = state.$model
            .map { (mode: $0.mode, isUnfulfilled: $0.isModeUnfulfilled) }
            .removeDuplicates { $0 == $1 }
            .sink { [weak self] health in
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
        let hosting = NSHostingController(rootView: MenuView(state: state, license: license))
        hosting.sizingOptions = .preferredContentSize
        hosting.view.layoutSubtreeIfNeeded()
        let popover = NSPopover()
        popover.behavior = .transient
        popover.animates = false
        popover.delegate = self
        popover.contentViewController = hosting
        self.popover = popover
        NSApp.activate()
        popover.show(relativeTo: button.bounds, of: button, preferredEdge: .minY)
        popover.contentViewController?.view.window?.makeKey()
    }

    private func closeMenu() {
        popover?.performClose(nil)
    }
}

extension StatusBarController: NSPopoverDelegate {
    func popoverDidClose(_ notification: Notification) {
        lastCloseTime = ProcessInfo.processInfo.systemUptime
        popover = nil
    }
}
