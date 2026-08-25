import AppKit
import SwiftUI

/// The contents of the menu bar popover.
struct MenuContent: View {
    @Bindable var controller: EngineController

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            header
            Divider()
            devicePickers
            Divider()
            modelPicker
            Divider()
            statusSection
            if let error = controller.lastError {
                Divider()
                errorSection(error)
            }
            Divider()
            footer
        }
        .padding(14)
        .frame(width: 320)
        .onAppear {
            controller.reloadDevices()
            controller.reloadCatalog()
        }
    }

    private var header: some View {
        Toggle(isOn: runningBinding) {
            Text(controller.status.running ? "Cleaning your microphone" : "Off")
                .font(.headline)
        }
        .toggleStyle(.switch)
        .disabled(!controller.status.running && !controller.canStart)
    }

    private var devicePickers: some View {
        VStack(alignment: .leading, spacing: 8) {
            Picker("Microphone", selection: inputBinding) {
                ForEach(controller.inputDevices) { device in
                    Text(device.name).tag(device.id)
                }
            }
            Picker("Virtual output", selection: outputBinding) {
                ForEach(controller.outputDevices) { device in
                    Text(device.name).tag(device.id)
                }
            }
            if controller.outputDevices.isEmpty {
                Text("No output device found. Install the noican driver or BlackHole.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            // Devices come and go as things are plugged in, and the engine has
            // to be restarted for a change to take effect.
            Button("Refresh devices") { controller.reloadDevices() }
                .font(.caption)
                .disabled(controller.status.running)
        }
        .disabled(controller.status.running)
    }

    private var modelPicker: some View {
        VStack(alignment: .leading, spacing: 8) {
            // Switching here is the whole point of the design: the engine ramps
            // between models without stopping, so A/B comparison is one click.
            Picker("Model", selection: $controller.selectedModelID) {
                ForEach(controller.models) { model in
                    Text(model.menuLabel).tag(model.id)
                }
            }
            if let model = selectedModel, !model.downloaded {
                Button("Download \(model.displayName)") {
                    Task { await controller.fetchModel(model.id) }
                }
                .font(.caption)
            }
            Toggle("Bypass", isOn: bypassBinding)
                .font(.caption)
                .disabled(!controller.status.running)
        }
    }

    private var statusSection: some View {
        VStack(alignment: .leading, spacing: 6) {
            LevelMeter(title: "In", level: controller.status.inputPeak)
            LevelMeter(title: "Out", level: controller.status.outputPeak)
            HStack {
                Text(latencyText)
                Spacer()
                if controller.status.switching {
                    Text("switching…")
                } else if controller.status.dropouts > 0 {
                    // Surfaced rather than hidden: a dropout is audible, and
                    // knowing the count is the first step in diagnosing it.
                    Text("\(controller.status.dropouts) dropouts")
                        .foregroundStyle(.orange)
                }
            }
            .font(.caption)
            .foregroundStyle(.secondary)
        }
    }

    private func errorSection(_ error: String) -> some View {
        Text(error)
            .font(.caption)
            .foregroundStyle(.red)
            .textSelection(.enabled)
            .fixedSize(horizontal: false, vertical: true)
    }

    private var footer: some View {
        HStack {
            Text("Select the virtual device as your microphone in Zoom or Meet.")
                .font(.caption2)
                .foregroundStyle(.secondary)
            Spacer()
            Button("Quit") { NSApplication.shared.terminate(nil) }
                .font(.caption)
        }
    }

    private var selectedModel: EngineController.Model? {
        controller.models.first { $0.id == controller.selectedModelID }
    }

    private var latencyText: String {
        guard controller.status.running else { return "stopped" }
        return String(format: "%.1f ms latency", controller.status.latencyMilliseconds)
    }

    // MARK: - Bindings
    //
    // Written out rather than using `$controller.…` directly because each one
    // has a side effect on the engine, and `Picker` needs a non-optional tag
    // type while the stored selections are optional until a device is chosen.

    private var runningBinding: Binding<Bool> {
        Binding(
            get: { controller.status.running },
            set: { controller.setRunning($0) }
        )
    }

    private var bypassBinding: Binding<Bool> {
        Binding(
            get: { controller.status.bypassed },
            set: { controller.setBypass($0) }
        )
    }

    private var inputBinding: Binding<String> {
        Binding(
            get: { controller.selectedInputUID ?? "" },
            set: { controller.selectedInputUID = $0 }
        )
    }

    private var outputBinding: Binding<String> {
        Binding(
            get: { controller.selectedOutputUID ?? "" },
            set: { controller.selectedOutputUID = $0 }
        )
    }
}
