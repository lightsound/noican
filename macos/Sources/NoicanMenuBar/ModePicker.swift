import SwiftUI

/// Custom three-segment mode control: a capsule track with a highlight
/// pill that slides between segments. Replaces the AppKit segmented
/// control so a single segment (Preview, while the current output must
/// not receive it) can be disabled individually, with the reason shown
/// by the caller below the control.
struct ModePicker: View {
    let mode: EngineMode
    let isBusy: Bool
    /// Non-nil disables the Preview segment (unsafe output target).
    let previewUnavailableReason: String?
    let select: (EngineMode) -> Void

    @Namespace private var highlightNamespace

    var body: some View {
        HStack(spacing: 2) {
            ForEach(EngineMode.allCases) { candidate in
                segment(candidate)
            }
        }
        .padding(3)
        .background(Capsule().fill(.quaternary.opacity(0.5)))
        .animation(.spring(response: 0.25, dampingFraction: 0.9), value: mode)
    }

    private func segment(_ candidate: EngineMode) -> some View {
        Button {
            select(candidate)
        } label: {
            HStack(spacing: 5) {
                Image(systemName: candidate.symbolName)
                Text(candidate.label)
                    .fontWeight(mode == candidate ? .semibold : .regular)
            }
            .font(.caption)
            .foregroundStyle(labelStyle(for: candidate))
            .frame(maxWidth: .infinity)
            .padding(.vertical, 5)
            .background {
                if mode == candidate {
                    Capsule()
                        .fill(pillColor(for: candidate))
                        .matchedGeometryEffect(id: "selection", in: highlightNamespace)
                }
            }
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .disabled(isBusy || isPreviewDisabled(candidate))
    }

    /// Preview is unselectable while the output target is unsafe; the
    /// segment stays enabled when it is the current mode so the state
    /// remains readable and escapable.
    private func isPreviewDisabled(_ candidate: EngineMode) -> Bool {
        candidate == .preview && previewUnavailableReason != nil && mode != .preview
    }

    /// Off reads as a neutral pill; the active modes carry the accent.
    private func pillColor(for candidate: EngineMode) -> Color {
        candidate == .off ? Color.primary.opacity(0.18) : Color.accentColor
    }

    private func labelStyle(for candidate: EngineMode) -> AnyShapeStyle {
        if mode == candidate {
            return candidate == .off
                ? AnyShapeStyle(.primary)
                : AnyShapeStyle(Color.white)
        }
        if isPreviewDisabled(candidate) {
            return AnyShapeStyle(.tertiary)
        }
        return AnyShapeStyle(.secondary)
    }
}
