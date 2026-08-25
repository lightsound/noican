import Foundation
import SwiftUI

/// A peak meter for one signal.
struct LevelMeter: View {
    let title: String
    let level: Float

    var body: some View {
        HStack(spacing: 8) {
            Text(title)
                .font(.caption)
                .foregroundStyle(.secondary)
                .frame(width: 24, alignment: .leading)
            GeometryReader { geometry in
                ZStack(alignment: .leading) {
                    Capsule().fill(.quaternary)
                    Capsule()
                        .fill(level > 0.95 ? Color.red : Color.accentColor)
                        .frame(width: geometry.size.width * CGFloat(displayLevel))
                }
            }
            .frame(height: 6)
        }
    }

    /// Peak level on a decibel-ish scale.
    ///
    /// A linear bar spends almost all its travel in the top few decibels and
    /// looks dead at speech levels, so the bottom of the scale is stretched.
    private var displayLevel: Float {
        guard level > 0 else { return 0 }
        let decibels = 20 * log10(max(level, 1e-5))
        // -60 dBFS at the left edge, 0 dBFS at the right.
        return min(max((decibels + 60) / 60, 0), 1)
    }
}
