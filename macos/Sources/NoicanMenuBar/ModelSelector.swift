import AppKit
import SwiftUI

struct ModelSelector: View {
    let models: [ModelInfo]
    let selectedID: String
    let isBusy: Bool
    let select: (String) -> Void

    @State private var hoveredModel: ModelInfo?
    @State private var hoverTransition: Task<Void, Never>?
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
            MainActor.assumeIsolated {
                rowFrames = frames
            }
        }
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
                hoverTransition = Task {
                    try? await Task.sleep(for: .milliseconds(200))
                    if !Task.isCancelled {
                        hoveredModel = entry
                    }
                }
            } else {
                hoveredModel = entry
            }
        } else {
            hoverTransition = Task {
                try? await Task.sleep(for: .milliseconds(150))
                if !Task.isCancelled {
                    hoveredModel = nil
                }
            }
        }
    }
}

private struct RowFramePreference: PreferenceKey {
    static let defaultValue: [String: CGRect] = [:]

    static func reduce(value: inout [String: CGRect], nextValue: () -> [String: CGRect]) {
        value.merge(nextValue()) { _, next in next }
    }
}

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
                if isDefault {
                    Text("Default")
                        .font(.caption)
                        .foregroundStyle(.tertiary)
                        .lineLimit(1)
                }
            }
            .padding(.horizontal, 6)
            .padding(.vertical, 5)
            .contentShape(RoundedRectangle(cornerRadius: 6))
            .background {
                if isHovering, !isBusy {
                    RoundedRectangle(cornerRadius: 6)
                        .fill(.quaternary.opacity(0.7))
                }
            }
        }
        .buttonStyle(StaticButtonStyle())
        .foregroundStyle(isSelected ? AnyShapeStyle(.primary) : AnyShapeStyle(.secondary))
        .disabled(isBusy)
        .onHover { hovering in
            isHovering = hovering
            hoverChanged(hovering)
        }
    }
}

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
            popover.animates = false
            popover.contentViewController = hosting
            self.hosting = hosting
            self.popover = popover
            return popover
        }
    }
}

private final class FlippedAnchorView: NSView {
    override var isFlipped: Bool { true }
}

private struct ModelDetailCard: View {
    let model: ModelInfo

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            VStack(alignment: .leading, spacing: 2) {
                Text(model.displayName)
                    .font(.headline)
                Text(model.tagline)
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
