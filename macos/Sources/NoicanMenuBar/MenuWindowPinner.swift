import AppKit
import SwiftUI

/// Keeps the menu window's top edge fixed while its content height
/// changes.
///
/// `MenuBarExtra(.window)` applies content-size changes around the
/// AppKit frame origin — the *bottom-left* corner — so shrinking the
/// content (collapsing "Model & strength") kept the bottom edge fixed
/// and dropped the whole menu away from the status item by the height
/// difference. Growth only looks correct because macOS clamps windows
/// below the menu bar. This representable watches the hosting window,
/// adopts the top edge from the system's own positioning (menu open,
/// display changes — moves without a height change), and whenever a
/// resize displaces that top edge, puts it back — so the menu stays
/// anchored under the status item and grows and shrinks downward.
struct MenuWindowPinner: NSViewRepresentable {
    func makeCoordinator() -> Coordinator {
        Coordinator()
    }

    func makeNSView(context: Context) -> WindowObservingView {
        let view = WindowObservingView()
        let coordinator = context.coordinator
        view.onWindowChange = { window in
            coordinator.attach(to: window)
        }
        return view
    }

    func updateNSView(_ nsView: WindowObservingView, context: Context) {
        context.coordinator.attach(to: nsView.window)
    }

    static func dismantleNSView(_ nsView: WindowObservingView, coordinator: Coordinator) {
        coordinator.attach(to: nil)
    }

    /// Observes the hosting window's move/resize notifications and
    /// restores the recorded top edge after resizes.
    @MainActor
    final class Coordinator {
        private weak var window: NSWindow?
        private var observers: [any NSObjectProtocol] = []
        /// Frame after the last observed change, to tell pure moves
        /// (system positioning — adopt the new top) from resizes
        /// (content changes — restore the recorded top).
        private var lastFrame: NSRect = .zero
        /// The menu's anchored top edge in screen coordinates, as
        /// placed by the system.
        private var topEdge: CGFloat?
        /// Guards against reacting to this coordinator's own setFrame.
        private var isRepinning = false

        func attach(to window: NSWindow?) {
            guard window !== self.window else {
                return
            }
            detach()
            self.window = window
            guard let window else {
                return
            }
            topEdge = window.frame.maxY
            lastFrame = window.frame
            let center = NotificationCenter.default
            observers.append(center.addObserver(
                forName: NSWindow.didMoveNotification,
                object: window,
                queue: .main
            ) { [weak self] _ in
                // Delivered on the main queue, which is the main actor.
                MainActor.assumeIsolated {
                    self?.windowMoved()
                }
            })
            observers.append(center.addObserver(
                forName: NSWindow.didResizeNotification,
                object: window,
                queue: .main
            ) { [weak self] _ in
                // Delivered on the main queue, which is the main actor.
                MainActor.assumeIsolated {
                    self?.windowResized()
                }
            })
        }

        // No deinit cleanup: Swift 6 forbids touching the non-Sendable
        // observer tokens from a nonisolated deinit, and the normal
        // teardown path (`dismantleNSView` → `attach(to: nil)`) already
        // removes them; the blocks only capture weak references.

        private func detach() {
            for observer in observers {
                NotificationCenter.default.removeObserver(observer)
            }
            observers.removeAll()
            window = nil
            topEdge = nil
        }

        /// A move without a height change is the system placing the
        /// menu (opening it, or a display-layout change): adopt its top
        /// edge as the anchor.
        private func windowMoved() {
            guard let window, !isRepinning else {
                return
            }
            if window.frame.height == lastFrame.height {
                topEdge = window.frame.maxY
            }
            lastFrame = window.frame
        }

        /// A resize that displaced the top edge is the bottom-anchored
        /// content change: put the top back where the system placed it.
        private func windowResized() {
            guard let window else {
                return
            }
            defer {
                lastFrame = window.frame
            }
            guard !isRepinning, let topEdge, abs(window.frame.maxY - topEdge) > 0.5 else {
                return
            }
            var frame = window.frame
            frame.origin.y = topEdge - frame.height
            isRepinning = true
            window.setFrame(frame, display: true)
            isRepinning = false
        }
    }
}

/// Plain NSView that reports when it lands in (or leaves) a window, so
/// the pinner can attach its observers as soon as the menu window
/// exists.
final class WindowObservingView: NSView {
    var onWindowChange: ((NSWindow?) -> Void)?

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        onWindowChange?(window)
    }
}
