import SwiftUI

/// Collapsible model selector with hover details.
///
/// Collapsed, it is a single row showing the selected model (hover it to
/// see that model's profile). Clicking expands the full catalog as
/// Control-Center-style rows; hovering any row pops the model's profile
/// card out beside the menu — four dot-scale ratings whose axes all
/// point the same way (more filled = better) plus the raw facts — so a
/// normal user can compare models without knowing their names. Picking
/// a row selects it and collapses the list.
///
/// This replaces a system `Picker`: AppKit menus cannot host hover
/// tracking or custom row views, so the rows are ordinary SwiftUI
/// buttons like the microphone list above. All text and ratings come
/// from the Rust registry, never from the UI.
struct ModelSelector: View {
    let models: [ModelInfo]
    let selectedID: String
    let isBusy: Bool
    let select: (String) -> Void

    /// Pure view state: which presentation the selector is in. Selecting
    /// a row collapses back to the one-line summary.
    @State private var isExpanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            if isExpanded {
                ForEach(models) { entry in
                    ModelRow(
                        model: entry,
                        isSelected: entry.id == selectedID,
                        isBusy: isBusy
                    ) {
                        isExpanded = false
                        if entry.id != selectedID {
                            select(entry.id)
                        }
                    }
                }
            } else {
                collapsedRow
            }
        }
        .animation(.easeOut(duration: 0.15), value: isExpanded)
    }

    private var selected: ModelInfo? {
        models.first { $0.id == selectedID }
    }

    private var collapsedRow: some View {
        HoverDetailRow(
            model: selected,
            isBusy: isBusy,
            action: { isExpanded = true }
        ) {
            HStack(spacing: 8) {
                Text(selected?.displayName ?? "Select a model")
                    .font(.callout)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer(minLength: 0)
                if let tagline = selected?.tagline, !tagline.isEmpty {
                    Text(tagline)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
                Image(systemName: "chevron.up.chevron.down")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
        }
    }
}

/// One selectable model row (expanded state): checkmark, name, and the
/// trailing tagline, with the shared hover-highlight/hover-detail
/// behavior. Enrollment-gated models render disabled with the reason.
private struct ModelRow: View {
    let model: ModelInfo
    let isSelected: Bool
    let isBusy: Bool
    let select: () -> Void

    var body: some View {
        HoverDetailRow(
            model: model,
            isBusy: isBusy || model.needsEnrollment,
            action: select
        ) {
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
        }
    }
}

/// Shared row chrome: native-menu-style hover highlight, a debounced
/// hover-triggered popover with the model's profile, and the row action.
/// The highlight follows the pointer instantly; the popover waits a
/// beat so skimming across the list does not flash cards.
private struct HoverDetailRow<Label: View>: View {
    let model: ModelInfo?
    let isBusy: Bool
    let action: () -> Void
    @ViewBuilder let label: () -> Label

    @State private var isHovering = false
    @State private var showsDetail = false
    @State private var hoverDelay: Task<Void, Never>?

    var body: some View {
        Button(action: action) {
            label()
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
        .buttonStyle(.plain)
        .disabled(isBusy)
        .onHover { hovering in
            isHovering = hovering
            hoverDelay?.cancel()
            if hovering, model != nil {
                hoverDelay = Task {
                    try? await Task.sleep(for: .milliseconds(200))
                    if !Task.isCancelled {
                        showsDetail = true
                    }
                }
            } else {
                showsDetail = false
            }
        }
        .popover(isPresented: $showsDetail, arrowEdge: .trailing) {
            if let model {
                ModelDetailCard(model: model)
            }
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
