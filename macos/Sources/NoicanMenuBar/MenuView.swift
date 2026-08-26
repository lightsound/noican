import AppKit
import SwiftUI

/// The menu bar popover, top to bottom: a status header, the single
/// Off / Preview / On mode control, a monitoring section that exists only
/// while the engine runs (level bars, and the headphone caption while
/// previewing), then the settings pickers — Microphone first, Model last,
/// since the model is rarely changed once chosen — and a utility footer.
/// Spacing and typography follow macOS menu bar app conventions (14 pt
/// content margins, caption-weight section labels, secondary text for
/// status detail).
struct MenuView: View {
    @ObservedObject var state: AppState

    private let contentPadding: CGFloat = 14

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            modePicker
            Divider()
                .padding(.horizontal, contentPadding)
            if state.mode != .off {
                monitoring
                Divider()
                    .padding(.horizontal, contentPadding)
            }
            settings
            Divider()
                .padding(.horizontal, contentPadding)
            footer
        }
        .frame(width: 320)
        // Poll the engine's peak meters (and the feedback-trip flag) only
        // while the popover is open; the task is cancelled when the view
        // disappears.
        .task {
            await state.pollLevels()
        }
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
        }
        .padding(.horizontal, contentPadding)
        .padding(.top, 12)
        .padding(.bottom, 8)
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

    /// The single top-level control. Preview = engine + self-monitor;
    /// On = engine only. Both feed the virtual microphone.
    private var modePicker: some View {
        Picker("Mode", selection: Binding(
            get: { state.mode },
            set: { state.setMode($0) }
        )) {
            ForEach(EngineMode.allCases) { mode in
                Text(mode.label).tag(mode)
            }
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .disabled(state.isBusy)
        .padding(.horizontal, contentPadding)
        .padding(.bottom, 10)
    }

    /// Live observation of the running stream: input (pre-model) and
    /// output (post-model) peak bars on a shared dB scale — speech moves
    /// both; noise-only passages leave the output bar clearly below the
    /// input bar, showing the suppression at a glance. The headphone
    /// warning appears only while previewing.
    private var monitoring: some View {
        VStack(alignment: .leading, spacing: 6) {
            LevelBar(label: "In", level: state.inputLevel, tint: .secondary)
            LevelBar(label: "Out", level: state.outputLevel, tint: .green)
            if state.mode == .preview {
                Text("Playing your processed voice on the default output. Use headphones — speakers will feed back.")
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let message = state.previewError {
                Text(message)
                    .font(.caption2)
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(.horizontal, contentPadding)
        .padding(.vertical, 10)
    }

    private var settings: some View {
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
                .disabled(state.mode != .off || state.isBusy)
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

/// One small horizontal peak bar. The linear peak is drawn on a
/// −60 dB…0 dB scale so quiet-but-present signal stays visible and the
/// input/output gap reads as suppression depth.
private struct LevelBar: View {
    let label: String
    let level: Float
    let tint: Color

    var body: some View {
        HStack(spacing: 8) {
            Text(label)
                .font(.caption2)
                .foregroundStyle(.secondary)
                .frame(width: 24, alignment: .leading)
            GeometryReader { proxy in
                ZStack(alignment: .leading) {
                    Capsule()
                        .fill(.quaternary)
                    Capsule()
                        .fill(tint)
                        .frame(width: proxy.size.width * fraction)
                }
            }
            .frame(height: 5)
        }
    }

    private var fraction: CGFloat {
        guard level > 0 else {
            return 0
        }
        let decibels = 20 * log10(Double(level))
        return CGFloat(min(1, max(0, 1 + decibels / 60)))
    }
}
