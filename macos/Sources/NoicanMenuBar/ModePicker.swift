import SwiftUI

/// Custom three-segment mode control: a capsule track with a highlight
/// pill that slides between segments. Every segment stays tappable
/// (except while busy); when a selection is refused — Preview on an
/// unsafe output — the caller keeps the mode where it was and explains
/// why below the control, which confuses less than a segment that
/// cannot be pressed.
///
/// The selection is the user's *intent* and never moves on its own: when
/// the selected mode is not actually delivering (`isUnfulfilled`), the
/// pill turns red instead, and tapping the same segment retries.
struct ModePicker: View {
    let mode: EngineMode
    let isBusy: Bool
    let isUnfulfilled: Bool
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
        // Isolate the sliding pill's matched geometry from outside layout
        // shifts (status/error text above changing the control's vertical
        // position mid-slide would otherwise distort the animation).
        .geometryGroup()
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
        .disabled(isBusy)
    }

    /// Off reads as a neutral pill; the active modes carry the accent;
    /// an unfulfilled selection warns in red.
    private func pillColor(for candidate: EngineMode) -> Color {
        if isUnfulfilled {
            return .red
        }
        return candidate == .off ? Color.primary.opacity(0.18) : Color.accentColor
    }

    private func labelStyle(for candidate: EngineMode) -> AnyShapeStyle {
        if mode == candidate {
            return candidate == .off && !isUnfulfilled
                ? AnyShapeStyle(.primary)
                : AnyShapeStyle(Color.white)
        }
        return AnyShapeStyle(.secondary)
    }
}
