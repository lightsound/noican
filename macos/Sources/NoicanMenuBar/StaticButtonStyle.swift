import SwiftUI

/// Button style that renders the label untouched in every state — no
/// pressed-state dimming. The mode control's sliding pill, the menu's
/// rows, and the disclosure header all carry their own selection or
/// hover feedback, and `.plain`'s opacity flash on press reads as
/// flicker on them, so they opt out of any system press effect.
struct StaticButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
    }
}
