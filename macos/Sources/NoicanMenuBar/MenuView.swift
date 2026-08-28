import AppKit
import NoicanState
import SwiftUI

/// The menu bar popover, top to bottom: a status header, the single
/// Off / Preview / On mode control, a monitoring section that exists only
/// while the engine runs (level bars, and the headphone caption while
/// previewing), the Microphone list, a collapsed-by-default
/// "Model & strength" section holding the model selector and strength
/// slider (progressive disclosure: the default model is meant to be good
/// enough that first-time users only touch the mode control and the
/// microphone), and a utility footer. Spacing and typography follow
/// macOS menu bar app conventions (14 pt content margins, caption-weight
/// section labels, secondary text for status detail).
struct MenuView: View {
    @ObservedObject var state: AppState

    /// Whether the "Model & strength" section is expanded. Pure view
    /// chrome, so it lives in AppStorage rather than the reducer;
    /// remembered across launches so power users keep it open while
    /// first-time users start with the short menu.
    @AppStorage("isSettingsExpanded") private var isSettingsExpanded = false

    private let contentPadding: CGFloat = 14

    /// The settled reducer state every section renders from.
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
            footer
        }
        .frame(width: 320)
        // Deliberately no layout animation on height-changing state:
        // every intermediate height would update the hosting
        // controller's preferredContentSize and make the popover chase
        // the animation frame by frame, which visibly warped the layout
        // (including the mode control's sliding pill — its own spring in
        // ModePicker is unaffected and still animates). System menus
        // change height instantly too.
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
            // Opt out of the popover's eased layout animation: the fill
            // styles (hierarchical gray vs. plain colors) cannot
            // interpolate, so animating them cross-fades through
            // transparent — the dot visibly blinked on Off transitions.
            .animation(nil, value: model.phase)
    }

    /// Settled health only — no transitional color: the spinner already
    /// says "busy", and flashing the dot orange for a milliseconds-long
    /// Preview/On toggle just made it churn.
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

    /// The single top-level control. Preview = engine + self-monitor;
    /// On = engine only. Both feed the virtual microphone. All prose
    /// feedback — engine failures, refused Preview presses (cleared live
    /// once the output is safe), preview failures and feedback trips —
    /// renders below the control, where changing height cannot move it.
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
            if let reason = model.messages.previewUnavailableReason {
                Text("Preview is unavailable: \(reason)")
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
                if let message = model.messages.microphoneError {
                    Text(message)
                        .font(.caption2)
                        .foregroundStyle(.red)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            // Progressive disclosure: the default model is meant to be
            // good enough that first-time users only touch the mode
            // control and the microphone, so the model list and the
            // strength slider live behind this header. The expansion
            // state is remembered across launches (power users keep it
            // open; new users start with the short menu). The collapsed
            // row advertises what is inside — the active model and
            // strength — so the fold never hides state.
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
            // Deliberately no expand/collapse animation: animating the
            // content height fights the MenuBarExtra window resize
            // (AppKit windows anchor at their bottom-left corner, so
            // mid-animation frames shifted the whole menu downward).
            if isSettingsExpanded {
                modelSection
                strengthSection
            }
        }
        .padding(.horizontal, contentPadding)
        .padding(.vertical, 10)
    }

    /// What the collapsed row advertises: the active model and
    /// strength, so folding the controls never hides state.
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
            // User picks route through selectModel; programmatic
            // reverts write the reducer state directly and must not
            // re-enter the apply path (they would wipe the failure
            // message they accompany). Hover any row for the model's
            // profile card.
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
            // Dry/wet mix applied inside the inference worker as one
            // atomic value: dragging never rebuilds the engine and
            // stays live during busy transitions, so the slider is
            // deliberately not disabled with the pickers. Preview
            // plays the same mix the virtual microphone receives.
            Slider(value: Binding(
                get: { model.intensity },
                set: { state.setIntensity($0) }
            ), in: 0...1)
            .controlSize(.small)
        }
    }

    /// All selectable inputs as an always-visible, checkmarked list
    /// (Control Center style) instead of a popup: newly connected devices
    /// appear immediately (the device list follows hot-plug), and the
    /// microphone can be switched while running — the engine rebuilds its
    /// transport around the new device with the same model and mode.
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

    /// Whole-percent strength readout next to the section label.
    private var intensityLabel: String {
        "\(Int((model.intensity * 100).rounded()))%"
    }

    // Device hot-plug is followed automatically (AppState registers a
    // Core Audio device-list listener), so no manual refresh is needed.
    private var footer: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                // The toggle renders the settled/optimistic reducer state;
                // a failed registration snaps it back with the reason
                // shown below (registration depends on the app's location
                // and signature — see docs/macos-hardware-test.md).
                // A checkbox, not a switch: this is a secondary setting
                // in a utility footer, and macOS reserves the prominent
                // switch style for primary states. Deliberately not
                // disabled while a registration attempt is in flight —
                // the reducer's serialization already ignores such
                // clicks, and the disabled dimming made the label
                // flicker gray on every click.
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

/// One selectable microphone row: comfortable click target (callout
/// text, ~28 pt row) with a native-menu-style hover highlight so the
/// rows read as pressable, and an accent checkmark on the selection.
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
