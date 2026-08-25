import AppKit
import SwiftUI

/// The menu bar popover: header with live status, the main toggle, grouped
/// device/model pickers, and a utility footer. Spacing and typography follow
/// macOS menu bar app conventions (14 pt content margins, caption-weight
/// section labels, secondary text for status detail).
struct MenuView: View {
    @ObservedObject var state: AppState

    private let contentPadding: CGFloat = 14

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Divider()
                .padding(.horizontal, contentPadding)
            mainToggle
            Divider()
                .padding(.horizontal, contentPadding)
            pickers
            Divider()
                .padding(.horizontal, contentPadding)
            footer
        }
        .frame(width: 320)
    }

    private var header: some View {
        HStack(alignment: .center, spacing: 10) {
            statusIndicator
            VStack(alignment: .leading, spacing: 2) {
                Text("noican")
                    .font(.headline)
                Text(state.statusText)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 0)
            if state.isBusy {
                ProgressView()
                    .controlSize(.small)
            }
        }
        .padding(.horizontal, contentPadding)
        .padding(.top, 12)
        .padding(.bottom, 10)
    }

    private var statusIndicator: some View {
        Circle()
            .fill(statusColor)
            .frame(width: 9, height: 9)
            .padding(.top, 1)
    }

    private var statusColor: Color {
        switch state.phase {
        case .off:
            .secondary.opacity(0.5)
        case .busy:
            .orange
        case .running:
            .green
        case .failed:
            .red
        }
    }

    private var mainToggle: some View {
        Toggle(isOn: Binding(
            get: { state.isEnabled },
            set: { state.setEnabled($0) }
        )) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Noise Cancellation")
                Text("Cleans the microphone into the virtual device")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .toggleStyle(.switch)
        .controlSize(.small)
        .disabled(state.isBusy)
        .padding(.horizontal, contentPadding)
        .padding(.vertical, 10)
    }

    private var pickers: some View {
        VStack(alignment: .leading, spacing: 12) {
            VStack(alignment: .leading, spacing: 4) {
                Text("Microphone")
                    .font(.caption)
                    .fontWeight(.semibold)
                    .foregroundStyle(.secondary)
                Picker("Microphone", selection: $state.selectedInputUID) {
                    ForEach(state.inputDevices) { device in
                        Text(device.name).tag(device.uid)
                    }
                }
                .labelsHidden()
                .disabled(state.isEnabled || state.isBusy)
            }
            VStack(alignment: .leading, spacing: 4) {
                Text("Model")
                    .font(.caption)
                    .fontWeight(.semibold)
                    .foregroundStyle(.secondary)
                Picker("Model", selection: $state.selectedModel) {
                    ForEach(state.models) { model in
                        Text(modelLabel(model)).tag(model.id)
                    }
                }
                .labelsHidden()
                .disabled(state.isBusy)
                .onChange(of: state.selectedModel) {
                    state.applySelectedModel()
                }
            }
        }
        .padding(.horizontal, contentPadding)
        .padding(.vertical, 10)
    }

    private func modelLabel(_ model: ModelInfo) -> String {
        model.needsEnrollment
            ? "\(model.displayName) — requires enrollment"
            : model.displayName
    }

    private var footer: some View {
        HStack {
            Button("Refresh Devices") {
                state.refreshDevices()
            }
            .disabled(state.isEnabled || state.isBusy)
            Spacer()
            Button("Quit") {
                NSApplication.shared.terminate(nil)
            }
            .keyboardShortcut("q")
        }
        .controlSize(.small)
        .padding(.horizontal, contentPadding)
        .padding(.vertical, 10)
    }
}
