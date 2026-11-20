import NoicanState
import SwiftUI

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
        .background {
            Capsule()
                .fill(pillColor(for: mode))
                .matchedGeometryEffect(id: mode, in: highlightNamespace, isSource: false)
        }
        .background(Capsule().fill(.quaternary.opacity(0.5)))
        .animation(.spring(response: 0.25, dampingFraction: 0.9), value: mode)
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
            .matchedGeometryEffect(id: candidate, in: highlightNamespace, isSource: true)
            .contentShape(Capsule())
        }
        .buttonStyle(StaticButtonStyle())
        .focusEffectDisabled()
        .disabled(isBusy)
    }

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
