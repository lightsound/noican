import AppKit
import SwiftUI

/// The menu bar popover: a header combining identity, live status, and the
/// master toggle (Control Center style), grouped device/model pickers, and
/// a utility footer. Spacing and typography follow macOS menu bar app
/// conventions (14 pt content margins, caption-weight section labels,
/// secondary text for status detail).
struct MenuView: View {
    @ObservedObject var state: AppState

    private let contentPadding: CGFloat = 14

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
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
                Text("Noican")
                    .font(.headline)
                Text(state.statusText)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(4)
                    .fixedSize(horizontal: false, vertical: true)
                    // Full text on hover, in case an error still clips.
                    .help(state.statusText)
            }
            Spacer(minLength: 0)
            if state.isBusy {
                ProgressView()
                    .controlSize(.small)
            }
            Toggle("Noise cancellation", isOn: Binding(
                get: { state.isEnabled },
                set: { state.setEnabled($0) }
            ))
            .labelsHidden()
            .toggleStyle(.switch)
            .controlSize(.small)
            .disabled(state.isBusy)
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
                        Text(modelLabel(model))
                            .tag(model.id)
                            // Enrollment-gated models (tse-48k) stay visible
                            // but unselectable until the app grows an
                            // enrollment flow.
                            .selectionDisabled(model.needsEnrollment)
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

    // Device hot-plug is followed automatically (AppState registers a
    // Core Audio device-list listener), so no manual refresh is needed.
    private var footer: some View {
        HStack {
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
