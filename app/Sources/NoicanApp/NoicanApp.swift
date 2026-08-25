// noican menu-bar app: on/off, input device picker, model selector,
// status. Phase 0 scope (docs/tech-research.md §12).
import SwiftUI

@main
struct NoicanApp: App {
    @StateObject private var engine = EngineModel()

    var body: some Scene {
        MenuBarExtra("noican", systemImage: engine.isRunning ? "mic.fill" : "mic.slash") {
            ContentView()
                .environmentObject(engine)
        }
        .menuBarExtraStyle(.window)
    }
}

struct ContentView: View {
    @EnvironmentObject private var engine: EngineModel

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Toggle(isOn: Binding(
                get: { engine.isRunning },
                set: { _ in engine.toggle() }
            )) {
                Text(engine.isRunning ? "On" : "Off")
                    .font(.headline)
            }
            .toggleStyle(.switch)

            Picker("Input", selection: $engine.selectedDeviceUID) {
                ForEach(engine.devices) { device in
                    Text(device.name).tag(device.uid)
                }
            }
            .disabled(engine.isRunning)

            Picker("Model", selection: Binding(
                get: { engine.selectedModelID },
                set: { engine.switchModel(to: $0) }
            )) {
                ForEach(engine.models) { model in
                    Text(model.fetched ? model.name : "\(model.name) (not fetched)")
                        .tag(model.id)
                }
            }

            Divider()

            Text(engine.statusLine)
                .font(.caption)
                .foregroundStyle(.secondary)

            if let error = engine.lastError {
                Text(error)
                    .font(.caption2)
                    .foregroundStyle(.red)
                    .lineLimit(4)
            }

            HStack {
                Button("Refresh devices") { engine.refreshLists() }
                Spacer()
                Button("Quit") {
                    engine.stop()
                    NSApplication.shared.terminate(nil)
                }
            }
            .font(.caption)
        }
        .padding(14)
        .frame(width: 300)
    }
}
