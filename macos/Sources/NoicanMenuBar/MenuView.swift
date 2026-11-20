import AppKit
import NoicanState
import SwiftUI

struct MenuView: View {
    @ObservedObject var state: AppState
    @ObservedObject var license: LicenseModel

    @AppStorage("isSettingsExpanded") private var isSettingsExpanded = false

    private let contentPadding: CGFloat = 14

    private var model: AppModel { state.model }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            modePicker
            Divider()
                .padding(.horizontal, contentPadding)
            if model.showsMonitoring {
                monitoring
                Divider()
                    .padding(.horizontal, contentPadding)
            }
            settings
            Divider()
                .padding(.horizontal, contentPadding)
            LicenseSection(license: license)
            Divider()
                .padding(.horizontal, contentPadding)
            footer
        }
        .frame(width: 320)
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
                Text(model.statusText)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .help(model.engineErrorMessage ?? model.statusText)
            }
            Spacer(minLength: 0)
            if model.isBusy {
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
            .animation(nil, value: model.phase)
    }

    private var statusColor: Color {
        switch model.phase {
        case .off:
            .secondary.opacity(0.5)
        case .running:
            .green
        case .failed:
            .red
        }
    }

    private var modePicker: some View {
        VStack(alignment: .leading, spacing: 6) {
            ModePicker(
                mode: model.mode,
                isBusy: model.isBusy,
                isUnfulfilled: model.isModeUnfulfilled,
                select: { state.setMode($0) }
            )
            if let message = model.engineErrorMessage {
                Text(message)
                    .font(.caption2)
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let message = model.messages.licenseRequired {
                Text(message)
                    .font(.caption2)
                    .foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let reason = model.messages.previewUnavailableReason {
                Text("Preview needs headphones — \(reason).")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let message = model.messages.previewError {
                Text(message)
                    .font(.caption2)
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let notice = model.virtualOutputLevelNotice {
                Text(notice)
                    .font(.caption2)
                    .foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(.horizontal, contentPadding)
        .padding(.bottom, 10)
    }

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
                if let message = model.messages.microphoneError {
                    Text(message)
                        .font(.caption2)
                        .foregroundStyle(.red)
                        .fixedSize(horizontal: false, vertical: true)
                }
                if let notice = model.microphoneNotice {
                    Text(notice)
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            Button {
                isSettingsExpanded.toggle()
            } label: {
                HStack(spacing: 4) {
                    Text("Model & strength")
                        .font(.caption)
                        .fontWeight(.semibold)
                        .foregroundStyle(.secondary)
                    Image(systemName: "chevron.right")
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(.secondary)
                        .rotationEffect(.degrees(isSettingsExpanded ? 90 : 0))
                    Spacer(minLength: 0)
                    if !isSettingsExpanded {
                        Text(settingsSummary)
                            .font(.caption)
                            .foregroundStyle(.tertiary)
                            .lineLimit(1)
                            .truncationMode(.middle)
                    }
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(StaticButtonStyle())
            if isSettingsExpanded {
                modelSection
                strengthSection
            }
        }
        .padding(.horizontal, contentPadding)
        .padding(.vertical, 10)
    }

    private var settingsSummary: String {
        let modelName = state.models
            .first { $0.id == model.selectedModelID }?
            .displayName ?? model.selectedModelID
        return "\(modelName) · \(intensityLabel)"
    }

    private var modelSection: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("Model")
                .font(.caption)
                .fontWeight(.semibold)
                .foregroundStyle(.secondary)
            ModelSelector(
                models: state.models,
                selectedID: model.selectedModelID,
                isBusy: model.isBusy,
                select: { state.selectModel($0) }
            )
            if let message = model.messages.modelError {
                Text(message)
                    .font(.caption2)
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private var strengthSection: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text("Strength")
                    .font(.caption)
                    .fontWeight(.semibold)
                    .foregroundStyle(.secondary)
                Spacer(minLength: 0)
                Text(intensityLabel)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .monospacedDigit()
            }
            Slider(value: Binding(
                get: { model.intensity },
                set: { state.setIntensity($0) }
            ), in: 0...1)
            .controlSize(.small)
        }
    }

    private var microphoneList: some View {
        VStack(alignment: .leading, spacing: 1) {
            if model.inputDevices.isEmpty {
                Text("No input devices")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
                    .padding(.vertical, 3)
            }
            ForEach(model.inputDevices) { device in
                MicrophoneRow(
                    device: device,
                    isSelected: device.uid == model.selectedInputUID,
                    isBusy: model.isBusy
                ) {
                    state.selectMicrophone(device.uid)
                }
            }
        }
    }

    private var intensityLabel: String {
        "\(Int((model.intensity * 100).rounded()))%"
    }

    private var footer: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Toggle("Start at login", isOn: Binding(
                    get: { model.isLaunchAtLoginEnabled },
                    set: { state.setLaunchAtLogin($0) }
                ))
                .toggleStyle(.checkbox)
                .font(.callout)
                Spacer(minLength: 0)
                Button("Quit") {
                    NSApplication.shared.terminate(nil)
                }
                .keyboardShortcut("q")
            }
            if let message = model.messages.launchAtLoginError {
                Text(message)
                    .font(.caption2)
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .controlSize(.small)
        .padding(.horizontal, contentPadding)
        .padding(.vertical, 10)
    }
}

private struct MicrophoneRow: View {
    let device: InputDevice
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
                if let rate = device.rateLabel {
                    Text(rate)
                        .font(.caption2)
                        .foregroundStyle(.tertiary)
                }
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
        .buttonStyle(StaticButtonStyle())
        .foregroundStyle(isSelected ? AnyShapeStyle(.primary) : AnyShapeStyle(.secondary))
        .disabled(isBusy)
        .onHover { hovering in
            isHovering = hovering
        }
    }
}

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
