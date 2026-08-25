import Observation

/// Peak levels, updated at the status poll rate.
///
/// Separate from `EngineController.Status` on purpose. The menu bar label
/// observes the controller's status, and SwiftUI re-evaluates the whole Scene
/// when an observed value changes — including redrawing the label while the
/// popover is closed. Levels change on every poll, so keeping them here means
/// only the meter views depend on them.
@Observable
@MainActor
final class MeterModel {
    /// Peak input level since the previous poll, in `[0, 1]`.
    var inputPeak: Float = 0

    /// Peak output level since the previous poll, in `[0, 1]`.
    var outputPeak: Float = 0
}
