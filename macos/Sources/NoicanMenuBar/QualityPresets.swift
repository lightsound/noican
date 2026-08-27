import Foundation

/// Outcome-first shortcuts over the model catalog.
///
/// Most users do not want to choose between model code names — they
/// want a result ("lighter", "strongest", "mute other voices"). Each
/// preset maps to one representative model, chosen from the measured
/// profiles in the Rust registry (docs in
/// crates/noican-models/src/traits.rs); picking a preset routes through
/// the exact same reducer path as picking that model in the Settings
/// list, and picking any other model there renders as "Custom". The
/// mapping is a UI shortcut, not state: the active preset is derived
/// from the selected model id, so the two controls can never disagree.
enum QualityPreset: String, CaseIterable, Identifiable {
    /// Lightest and fastest, for battery or older machines.
    case light
    /// The first-launch default: solid cleanup at low latency and cost.
    case balanced
    /// Strongest cleanup, including keyboard/trackpad clicks.
    case max
    /// Also mutes other people talking nearby.
    case voices

    var id: String { rawValue }

    /// Segment title.
    var label: String {
        switch self {
        case .light: "Light"
        case .balanced: "Balanced"
        case .max: "Max"
        case .voices: "Voices"
        }
    }

    /// The representative model this preset selects.
    var modelID: String {
        switch self {
        case .light: "fastenhancer-t"
        case .balanced: AppState.defaultModelID
        case .max: "dpdfnet8"
        case .voices: "hush"
        }
    }

    /// Tooltip: what the preset trades, and which model it selects.
    var help: String {
        switch self {
        case .light:
            "Lightest and fastest, with milder cleanup (FastEnhancer-T)."
        case .balanced:
            "The default: solid cleanup, low delay, light on the battery (FastEnhancer-B)."
        case .max:
            "Strongest cleanup — removes keyboard and trackpad clicks too, "
                + "at ~60 ms extra delay and higher CPU cost (DPDFNet8)."
        case .voices:
            "Also mutes other people talking nearby; voice narrows to "
                + "phone quality (Hush)."
        }
    }

    /// The preset the selected model corresponds to, or nil for any
    /// other model ("Custom").
    static func matching(_ modelID: String) -> Self? {
        allCases.first { $0.modelID == modelID }
    }
}
