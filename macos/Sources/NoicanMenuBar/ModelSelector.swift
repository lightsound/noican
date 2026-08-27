import SwiftUI

/// The model list with hover profile cards.
///
/// Every catalog entry renders as a Control-Center-style row (same
/// chrome as the microphone list above). Hovering a row pops the
/// model's profile card out beside the menu — four dot-scale ratings
/// whose axes all point the same way (more filled = better) plus the
/// raw facts — so a normal user can compare models without knowing
/// their names.
///
/// One popover serves the whole list: it appears after a short delay on
/// the first hover, then **stays up while the pointer moves between
/// rows** (only its content swaps — re-presenting per row made the card
/// blink on every move), and hides after a short grace period once the
/// pointer leaves the rows entirely (the grace also bridges the gaps
/// between rows).
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

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            ForEach(models) { entry in
                ModelRow(
                    model: entry,
                    isSelected: entry.id == selectedID,
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
            }
        }
        // One shared popover anchored to the list, so moving between
        // rows swaps the card's content without re-presenting it.
        .popover(
            isPresented: Binding(
                get: { hoveredModel != nil },
                set: { presented in
                    if !presented {
                        hoveredModel = nil
                    }
                }
            ),
            attachmentAnchor: .rect(.bounds),
            arrowEdge: .trailing
        ) {
            if let hoveredModel {
                ModelDetailCard(model: hoveredModel)
            }
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
                // Already up: swap the content in place, no re-present.
                hoveredModel = entry
            }
        } else {
            // Grace period so the gap between rows (and the moment of a
            // click) does not blink the card; entering the next row
            // cancels this and swaps instead.
            hoverTransition = Task {
                try? await Task.sleep(for: .milliseconds(150))
                if !Task.isCancelled {
                    hoveredModel = nil
                }
            }
        }
    }
}

/// One selectable model row: checkmark, name, and the trailing tagline,
/// with the native-menu-style hover highlight. Enrollment-gated models
/// render disabled with the reason (their profile still shows on
/// hover). Hover enter/leave is reported upward — the shared profile
/// card belongs to the list, not the row.
private struct ModelRow: View {
    let model: ModelInfo
    let isSelected: Bool
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
                Text(model.needsEnrollment ? "requires enrollment" : model.tagline)
                    .font(.caption)
                    .foregroundStyle(.tertiary)
                    .lineLimit(1)
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
        .buttonStyle(.plain)
        .foregroundStyle(isSelected ? AnyShapeStyle(.primary) : AnyShapeStyle(.secondary))
        .disabled(isBusy || model.needsEnrollment)
        .onHover { hovering in
            isHovering = hovering
            hoverChanged(hovering)
        }
    }
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
