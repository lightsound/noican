import AppKit
import SwiftUI

/// The model list with hover profile cards.
///
/// Every catalog entry renders as a Control-Center-style row (same
/// chrome as the microphone list above). Hovering a row pops the
/// model's profile card out beside that row — four dot-scale ratings
/// whose axes all point the same way (more filled = better) plus the
/// raw facts — so a normal user can compare models without knowing
/// their names.
///
/// One card serves the whole list: it appears after a short delay on
/// the first hover, then **stays up and follows the pointer from row to
/// row** (position and content update in place — re-presenting per row
/// made the card blink), and hides after a short grace period once the
/// pointer leaves the rows (the grace also bridges the gaps between
/// rows). SwiftUI's `.popover` cannot move while presented, so the card
/// is hosted by [`HoverCardPresenter`]'s AppKit popover instead.
///
/// This replaces a system `Picker`: AppKit menus cannot host hover
/// tracking or custom row views, so the rows are ordinary SwiftUI
/// buttons. All text and ratings come from the Rust registry, never
/// from the UI.
struct ModelSelector: View {
    let models: [ModelInfo]
    let selectedID: String
    let isBusy: Bool
    let select: (String) -> Void

    /// Model the profile card currently describes; nil hides the card.
    @State private var hoveredModel: ModelInfo?
    /// Debounces the first appearance (skimming into the list must not
    /// flash the card) and the disappearance (row-to-row gaps must not
    /// blink it).
    @State private var hoverTransition: Task<Void, Never>?
    /// Row frames in the list's coordinate space, for anchoring the
    /// card beside the hovered row.
    @State private var rowFrames: [String: CGRect] = [:]

    private static let coordinateSpace = "model-selector"

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            ForEach(models) { entry in
                ModelRow(
                    model: entry,
                    isSelected: entry.id == selectedID,
                    isDefault: entry.id == AppState.defaultModelID,
                    isBusy: isBusy,
                    select: {
                        if entry.id != selectedID {
                            select(entry.id)
                        }
                    },
                    hoverChanged: { hovering in
                        hoverChanged(entry, hovering)
                    }
                )
                .background {
                    GeometryReader { proxy in
                        Color.clear.preference(
                            key: RowFramePreference.self,
                            value: [entry.id: proxy.frame(in: .named(Self.coordinateSpace))]
                        )
                    }
                }
            }
        }
        .coordinateSpace(name: Self.coordinateSpace)
        .onPreferenceChange(RowFramePreference.self) { frames in
            // The macOS 15 SDK marks this closure @Sendable while
            // preference changes are in fact delivered on the main
            // actor; assumeIsolated bridges the annotation gap.
            MainActor.assumeIsolated {
                rowFrames = frames
            }
        }
        // Invisible AppKit anchor spanning the list; it owns the one
        // NSPopover whose position tracks the hovered row.
        .background {
            HoverCardPresenter(
                model: hoveredModel,
                anchorRect: hoveredModel.flatMap { rowFrames[$0.id] } ?? .zero
            )
        }
        .onDisappear {
            hoverTransition?.cancel()
            hoveredModel = nil
        }
    }

    private func hoverChanged(_ entry: ModelInfo, _ hovering: Bool) {
        hoverTransition?.cancel()
        if hovering {
            if hoveredModel == nil {
                // First appearance: wait a beat so skimming past the
                // list does not flash the card.
                hoverTransition = Task {
                    try? await Task.sleep(for: .milliseconds(200))
                    if !Task.isCancelled {
                        hoveredModel = entry
                    }
                }
            } else {
                // Already up: move to this row in place, no re-present.
                hoveredModel = entry
            }
        } else {
            // Grace period so the gap between rows (and the moment of a
            // click) does not blink the card; entering the next row
            // cancels this and moves the card instead.
            hoverTransition = Task {
                try? await Task.sleep(for: .milliseconds(150))
                if !Task.isCancelled {
                    hoveredModel = nil
                }
            }
        }
    }
}

/// Row frames keyed by model id, collected in the list's space.
private struct RowFramePreference: PreferenceKey {
    static let defaultValue: [String: CGRect] = [:]

    static func reduce(value: inout [String: CGRect], nextValue: () -> [String: CGRect]) {
        value.merge(nextValue()) { _, next in next }
    }
}

/// One selectable model row: checkmark and name, with the
/// native-menu-style hover highlight. No per-row descriptions — the
/// hover card carries the profile, so trailing text would repeat it;
/// the only annotations are "Default" on the first-launch model and
/// "requires enrollment" on disabled rows (a disabled row must explain
/// itself immediately). Hover enter/leave is reported upward — the
/// shared profile card belongs to the list, not the row.
private struct ModelRow: View {
    let model: ModelInfo
    let isSelected: Bool
    let isDefault: Bool
    let isBusy: Bool
    let select: () -> Void
    let hoverChanged: (Bool) -> Void

    @State private var isHovering = false

    var body: some View {
        Button(action: select) {
            HStack(spacing: 8) {
                Image(systemName: "checkmark")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(Color.accentColor)
                    .opacity(isSelected ? 1 : 0)
                Text(model.displayName)
                    .font(.callout)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer(minLength: 0)
                if let annotation {
                    Text(annotation)
                        .font(.caption)
                        .foregroundStyle(.tertiary)
                        .lineLimit(1)
                }
            }
            .opacity(model.needsEnrollment ? 0.5 : 1)
            .padding(.horizontal, 6)
            .padding(.vertical, 5)
            .contentShape(RoundedRectangle(cornerRadius: 6))
            .background {
                if isHovering, !isBusy, !model.needsEnrollment {
                    RoundedRectangle(cornerRadius: 6)
                        .fill(.quaternary.opacity(0.7))
                }
            }
        }
        .buttonStyle(StaticButtonStyle())
        .foregroundStyle(isSelected ? AnyShapeStyle(.primary) : AnyShapeStyle(.secondary))
        .disabled(isBusy || model.needsEnrollment)
        .onHover { hovering in
            isHovering = hovering
            hoverChanged(hovering)
        }
    }

    private var annotation: String? {
        if model.needsEnrollment {
            return "requires enrollment"
        }
        return isDefault ? "Default" : nil
    }
}

/// AppKit-backed presenter for the hover profile card.
///
/// SwiftUI's `.popover` cannot reposition while presented — changing
/// its anchor or item dismisses and re-presents, which blinked the card
/// on every row change. `NSPopover` can: its `positioningRect` may be
/// updated live. This representable spans the list as an invisible,
/// SwiftUI-coordinate (flipped) anchor view and drives one popover:
/// show on the first hover, move + swap content in place across rows,
/// close on leave. `.applicationDefined` behavior keeps AppKit from
/// auto-dismissing it (the hover state machine owns its lifetime).
private struct HoverCardPresenter: NSViewRepresentable {
    let model: ModelInfo?
    let anchorRect: CGRect

    func makeCoordinator() -> Coordinator {
        Coordinator()
    }

    func makeNSView(context: Context) -> FlippedAnchorView {
        FlippedAnchorView()
    }

    func updateNSView(_ nsView: FlippedAnchorView, context: Context) {
        context.coordinator.update(anchor: nsView, model: model, anchorRect: anchorRect)
    }

    static func dismantleNSView(_ nsView: FlippedAnchorView, coordinator: Coordinator) {
        coordinator.hide()
    }

    /// Owns the popover across SwiftUI updates. Presentation is
    /// deferred one main-actor hop: `updateNSView` runs inside SwiftUI's
    /// view update, where presenting an AppKit window synchronously can
    /// re-enter layout. The latest desired state wins (stale hops apply
    /// whatever is current, so rapid hover changes coalesce).
    @MainActor
    final class Coordinator {
        private var popover: NSPopover?
        private var hosting: NSHostingController<ModelDetailCard>?
        private weak var anchor: NSView?
        private var desired: (model: ModelInfo, rect: CGRect)?

        func update(anchor: NSView, model: ModelInfo?, anchorRect: CGRect) {
            self.anchor = anchor
            if let model, !anchorRect.isEmpty {
                desired = (model, anchorRect)
            } else {
                desired = nil
            }
            Task { @MainActor in
                self.apply()
            }
        }

        func hide() {
            desired = nil
            if popover?.isShown == true {
                // close() is synchronous; performClose would defer, and
                // a card outliving its rows is what this must prevent.
                popover?.close()
            }
        }

        private func apply() {
            guard let desired, let anchor, anchor.window != nil else {
                hide()
                return
            }
            let popover = ensurePopover(for: desired.model)
            hosting?.rootView = ModelDetailCard(model: desired.model)
            if popover.isShown {
                popover.positioningRect = desired.rect
            } else {
                popover.show(relativeTo: desired.rect, of: anchor, preferredEdge: .maxX)
            }
        }

        private func ensurePopover(for model: ModelInfo) -> NSPopover {
            if let popover {
                return popover
            }
            let hosting = NSHostingController(rootView: ModelDetailCard(model: model))
            hosting.sizingOptions = .preferredContentSize
            let popover = NSPopover()
            popover.behavior = .applicationDefined
            // No show/hide animation: an animated close is asynchronous
            // and can overlap the menu's own resize when the list
            // collapses while a card is up (the card must be gone before
            // the rows are).
            popover.animates = false
            popover.contentViewController = hosting
            self.hosting = hosting
            self.popover = popover
            return popover
        }
    }
}

/// Invisible anchor with SwiftUI's top-left origin, so row frames
/// measured in SwiftUI coordinates serve directly as AppKit
/// positioning rects.
private final class FlippedAnchorView: NSView {
    override var isFlipped: Bool { true }
}

/// The hover card: name, purpose tag, four dot-scale ratings, and the
/// raw facts behind them (native rate, measured delay, size).
private struct ModelDetailCard: View {
    let model: ModelInfo

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            VStack(alignment: .leading, spacing: 2) {
                Text(model.displayName)
                    .font(.headline)
                Text(model.needsEnrollment ? "requires enrollment" : model.tagline)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            VStack(alignment: .leading, spacing: 4) {
                ratingRow("Noise removal", model.ratings.noiseRemoval)
                ratingRow("Voice quality", model.ratings.voiceQuality)
                ratingRow("Responsiveness", model.ratings.responsiveness)
                ratingRow("Efficiency", model.ratings.efficiency)
            }
            if !model.details.isEmpty {
                Text(model.details)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(12)
        .frame(width: 240, alignment: .leading)
    }

    private func ratingRow(_ label: String, _ value: Int) -> some View {
        HStack(spacing: 8) {
            Text(label)
                .font(.caption)
                .foregroundStyle(.secondary)
                .frame(width: 100, alignment: .leading)
            RatingDots(value: value)
            Spacer(minLength: 0)
        }
    }
}

/// Five-step dot scale (filled = accent), the compact stand-in for the
/// segmented rating bars common in model pickers.
private struct RatingDots: View {
    let value: Int

    var body: some View {
        HStack(spacing: 4) {
            ForEach(0..<5, id: \.self) { position in
                Circle()
                    .fill(
                        position < value
                            ? AnyShapeStyle(Color.accentColor)
                            : AnyShapeStyle(.quaternary)
                    )
                    .frame(width: 6, height: 6)
            }
        }
    }
}
