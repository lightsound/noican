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
            if state.showsMonitoring {
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
        // Ease the layout when status text or sections change height, so
        // the mode control glides instead of jumping (its sliding pill is
        // additionally isolated via geometryGroup in ModePicker).
        .animation(.easeOut(duration: 0.15), value: state.phase)
        .animation(.easeOut(duration: 0.15), value: state.mode)
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
                // Always one line: the header must never change height,
                // or everything below it (including the sliding pill)
                // would shift mid-animation. Full errors render below
                // the mode control.
                Text(state.statusText)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .help(state.engineErrorMessage ?? state.statusText)
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
        if state.isBusy {
            return .orange
        }
        switch state.phase {
        case .off:
            return .secondary.opacity(0.5)
        case .running:
            return .green
        case .failed:
            return .red
        }
    }

    /// The single top-level control. Preview = engine + self-monitor;
    /// On = engine only. Both feed the virtual microphone. All prose
    /// feedback — engine failures, refused Preview presses (cleared live
    /// once the output is safe), preview failures and feedback trips —
    /// renders below the control, where changing height cannot move it.
    private var modePicker: some View {
        VStack(alignment: .leading, spacing: 6) {
            ModePicker(
                mode: state.mode,
                isBusy: state.isBusy,
                isUnfulfilled: state.isModeUnfulfilled,
                select: { state.setMode($0) }
            )
            if let message = state.engineErrorMessage {
                Text(message)
                    .font(.caption2)
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let reason = state.previewUnavailableReason {
                Text("Preview is unavailable: \(reason)")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
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
        .padding(.bottom, 10)
    }

    /// Live observation of the running stream: input (pre-model) and
    /// output (post-model) peak bars on a shared dB scale — speech moves
    /// both; noise-only passages leave the output bar clearly below the
    /// input bar, showing the suppression at a glance.
    ///
    /// No persistent headphone warning: its job is done by code — unsafe
    /// outputs are refused on press with the reason shown, and actual
    /// feedback through an unclassifiable output trips the killswitch,
    /// which also explains itself.
    private var monitoring: some View {
        VStack(alignment: .leading, spacing: 6) {
            LevelBar(label: "Before", level: state.inputLevel, tint: .secondary)
            LevelBar(label: "After", level: state.outputLevel, tint: .green)
        }
        .padding(.horizontal, contentPadding)
        .padding(.vertical, 10)
    }

    private var settings: some View {
        VStack(alignment: .leading, spacing: 12) {
            VStack(alignment: .leading, spacing: 4) {
                Text("Mic")
                    .font(.caption)
                    .fontWeight(.semibold)
                    .foregroundStyle(.secondary)
                microphoneList
                if let message = state.microphoneError {
                    Text(message)
                        .font(.caption2)
                        .foregroundStyle(.red)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            VStack(alignment: .leading, spacing: 4) {
                Text("Model")
                    .font(.caption)
                    .fontWeight(.semibold)
                    .foregroundStyle(.secondary)
                // User picks route through selectModel; programmatic
                // reverts write the property directly and must not
                // re-enter the apply path (they would wipe the failure
                // message they accompany).
                Picker("Model", selection: Binding(
                    get: { state.selectedModel },
                    set: { state.selectModel($0) }
                )) {
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
                if let message = state.modelError {
                    Text(message)
                        .font(.caption2)
                        .foregroundStyle(.red)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
        .padding(.horizontal, contentPadding)
        .padding(.vertical, 10)
    }

    /// All selectable inputs as an always-visible, checkmarked list
    /// (Control Center style) instead of a popup: newly connected devices
    /// appear immediately (the device list follows hot-plug), and the
    /// microphone can be switched while running — the engine rebuilds its
    /// transport around the new device with the same model and mode.
    private var microphoneList: some View {
        VStack(alignment: .leading, spacing: 1) {
            if state.inputDevices.isEmpty {
                Text("No input devices")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
                    .padding(.vertical, 3)
            }
            ForEach(state.inputDevices) { device in
                MicrophoneRow(
                    device: device,
                    isSelected: device.uid == state.selectedInputUID,
                    isBusy: state.isBusy
                ) {
                    state.selectMicrophone(device.uid)
                }
            }
        }
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

/// One selectable microphone row: comfortable click target (callout
/// text, ~28 pt row) with a native-menu-style hover highlight so the
/// rows read as pressable, and an accent checkmark on the selection.
private struct MicrophoneRow: View {
    let device: AudioDeviceInfo
    let isSelected: Bool
    let isBusy: Bool
    let select: () -> Void

    @State private var isHovering = false

    var body: some View {
        Button(action: select) {
            HStack(spacing: 8) {
                Image(systemName: "checkmark")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(Color.accentColor)
                    .opacity(isSelected ? 1 : 0)
                Text(device.name)
                    .font(.callout)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 6)
            .padding(.vertical, 5)
            .contentShape(RoundedRectangle(cornerRadius: 6))
            .background {
                if isHovering, !isBusy {
                    RoundedRectangle(cornerRadius: 6)
                        .fill(.quaternary.opacity(0.7))
                }
            }
        }
        .buttonStyle(.plain)
        .foregroundStyle(isSelected ? AnyShapeStyle(.primary) : AnyShapeStyle(.secondary))
        .disabled(isBusy)
        .onHover { hovering in
            isHovering = hovering
        }
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
                .frame(width: 42, alignment: .leading)
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
