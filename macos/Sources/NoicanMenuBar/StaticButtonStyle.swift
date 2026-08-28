import SwiftUI

/// Button style that renders the label untouched in every state — no
/// pressed-state dimming. The menu's rows and disclosure headers carry
/// their own hover highlight, and `.plain`'s opacity flash on press
/// reads as flicker on text-only controls, so they opt out of any
/// system press effect.
struct StaticButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
    }
}
