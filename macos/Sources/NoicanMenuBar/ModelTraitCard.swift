import SwiftUI

/// Compact profile of the selected model, rendered under the Model
/// picker: four dot-scale ratings whose axes all point the same way
/// (more filled = better — latency is shown as responsiveness, compute
/// cost as efficiency), so a normal user can compare models without
/// knowing their names. Hovering reveals the raw facts behind the dots
/// (native rate, measured delay, size) via the tooltip.
struct ModelTraitCard: View {
    let model: ModelInfo

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            ratingRow("Noise removal", model.ratings.noiseRemoval)
            ratingRow("Voice quality", model.ratings.voiceQuality)
            ratingRow("Responsiveness", model.ratings.responsiveness)
            ratingRow("Efficiency", model.ratings.efficiency)
        }
        .padding(.top, 2)
        .help(model.details)
    }

    private func ratingRow(_ label: String, _ value: Int) -> some View {
        HStack(spacing: 8) {
            Text(label)
                .font(.caption2)
                .foregroundStyle(.secondary)
                .frame(width: 96, alignment: .leading)
            RatingDots(value: value)
            Spacer(minLength: 0)
        }
    }
}

/// Five-step dot scale (filled = accent), the menu-bar-sized stand-in
/// for the segmented rating bars common in model pickers.
private struct RatingDots: View {
    let value: Int

    var body: some View {
        HStack(spacing: 3) {
            ForEach(0..<5, id: \.self) { position in
                Circle()
                    .fill(
                        position < value
                            ? AnyShapeStyle(Color.accentColor)
                            : AnyShapeStyle(.quaternary)
                    )
                    .frame(width: 5, height: 5)
            }
        }
    }
}
