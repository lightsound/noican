import AppKit
import SwiftUI

@main
struct NoicanMenuBarApp: App {
    @StateObject private var state = AppState()
    private let statusTimer = Timer.publish(every: 1, on: .main, in: .common).autoconnect()

    var body: some Scene {
        MenuBarExtra {
            Toggle(
                "Noise cancellation",
                isOn: Binding(
                    get: { state.isEnabled },
                    set: { state.setEnabled($0) }
                )
            )

            Picker("Input", selection: $state.selectedInputUID) {
                ForEach(state.inputDevices) { device in
                    Text(device.name).tag(device.uid)
                }
            }
            .disabled(state.isEnabled)

            Picker("Model", selection: $state.selectedModel) {
                ForEach(state.models, id: \.self) { model in
                    Text(model).tag(model)
                }
            }
            .onChange(of: state.selectedModel) {
                state.applySelectedModel()
            }

            Divider()
            Text(state.status)
                .font(.caption)
                .foregroundStyle(.secondary)

            Button("Refresh devices") {
                state.refreshDevices()
            }
            .disabled(state.isEnabled)

            Divider()
            Button("Quit") {
                NSApplication.shared.terminate(nil)
            }
        } label: {
            Label(
                "noican",
                systemImage: state.isEnabled ? "waveform.badge.mic" : "mic.slash"
            )
        }
        .menuBarExtraStyle(.window)
        .onReceive(statusTimer) { _ in
            state.updateStatus()
        }
    }
}
