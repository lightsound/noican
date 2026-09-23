import NoicanLicensing
import SwiftUI

/// The popover's license section. Without a working license the key
/// field is always visible (it is what the refused mode control points
/// at); a working license folds into one row that advertises its status
/// and expands to the key, the last verification, and "Deactivate this
/// Mac".
struct LicenseSection: View {
    @ObservedObject var license: LicenseModel

    @State private var keyInput = ""
    @State private var isExpanded = false
    @State private var isConfirmingDeactivation = false

    private static let graceDays = Int(LicensePolicy.standard.offlineGracePeriod / 86_400)

    private var state: LicenseState { license.state }
    private var isWorking: Bool { state.activity != nil }

    private var isOffline: Bool {
        if case .offline = state.status {
            return true
        }
        return false
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            header
            switch state.status {
            case .unconfigured:
                EmptyView()
            case .unlicensed:
                keyEntry
            case let .rejected(_, rejection):
                caption(rejection.message, color: .red)
                keyEntry
            case .verificationRequired:
                caption(
                    "The license couldn't be verified for \(Self.graceDays) days. "
                        + "Connect to the internet, then select Verify.",
                    color: .orange
                )
                verifyButton
            case let .active(summary):
                if isExpanded {
                    details(summary)
                }
            case let .offline(summary, graceEndsAt):
                caption(
                    "Couldn't reach the license server — works offline until "
                        + "\(graceEndsAt.formatted(date: .abbreviated, time: .omitted)).",
                    color: .secondary
                )
                if isExpanded {
                    details(summary)
                }
            }
            if let notice = state.notice {
                // Offline, the status caption already says the license
                // still works; the reason is detail, not an error.
                caption(notice, color: isOffline ? .secondary : .red)
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .onChange(of: state.status.allowsProcessing) {
            isConfirmingDeactivation = false
            if state.status.allowsProcessing {
                keyInput = ""
            }
        }
    }

    // MARK: - Pieces

    /// "License" label with the status summary; foldable only while the
    /// license works (otherwise the key field must stay in view).
    private var header: some View {
        Button {
            isExpanded.toggle()
        } label: {
            HStack(spacing: 4) {
                Text("License")
                    .font(.caption)
                    .fontWeight(.semibold)
                    .foregroundStyle(.secondary)
                if isFoldable {
                    Image(systemName: "chevron.right")
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(.secondary)
                        .rotationEffect(.degrees(isExpanded ? 90 : 0))
                }
                Spacer(minLength: 0)
                if isWorking {
                    ProgressView()
                        .controlSize(.mini)
                }
                Text(summaryText)
                    .font(.caption)
                    .foregroundStyle(.tertiary)
                    .lineLimit(1)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(StaticButtonStyle())
        .disabled(!isFoldable)
    }

    private var isFoldable: Bool {
        switch state.status {
        case .active, .offline:
            true
        case .unconfigured, .unlicensed, .verificationRequired, .rejected:
            false
        }
    }

    private var summaryText: String {
        let text = switch state.status {
        case .unconfigured: "Not configured in this build"
        case .unlicensed: "Not activated"
        case .active: "Active"
        case .offline: "Active (offline)"
        case .verificationRequired: "Verification needed"
        case .rejected: "Not valid"
        }
        return license.isSandbox ? "\(text) · sandbox" : text
    }

    private var keyEntry: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                TextField("License key", text: $keyInput)
                    .textFieldStyle(.roundedBorder)
                    .font(.callout)
                    .onSubmit(activate)
                Button("Activate", action: activate)
                    .disabled(isWorking || keyInput.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
            links
        }
        .controlSize(.small)
    }

    private var verifyButton: some View {
        HStack {
            Button("Verify") { license.verifyNow() }
                .disabled(isWorking)
            Spacer(minLength: 0)
            links
        }
        .controlSize(.small)
    }

    private func details(_ summary: LicenseSummary) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            caption(
                "Key \(summary.displayKey) · verified "
                    + summary.lastValidatedAt.formatted(.relative(presentation: .named)),
                color: .secondary
            )
            if isConfirmingDeactivation {
                caption(
                    "Deactivate this Mac? It frees one of the key's devices; you'll need the key to activate again.",
                    color: .secondary
                )
                HStack {
                    Button("Deactivate", role: .destructive) { license.deactivate() }
                    Button("Cancel") { isConfirmingDeactivation = false }
                }
                .disabled(isWorking)
            } else {
                HStack {
                    Button("Verify now") { license.verifyNow() }
                    Button("Deactivate this Mac…") { isConfirmingDeactivation = true }
                }
                .disabled(isWorking)
                links
            }
        }
        .controlSize(.small)
    }

    /// Purchase and customer-portal links, each shown when configured.
    private var links: some View {
        HStack(spacing: 10) {
            if let url = license.purchaseURL, !state.status.allowsProcessing {
                Link("Buy Noican", destination: url)
            }
            if let url = license.managementURL {
                Link("Manage devices", destination: url)
            }
        }
        .font(.caption)
    }

    private func caption(_ text: String, color: Color) -> some View {
        Text(text)
            .font(.caption2)
            .foregroundStyle(color)
            .fixedSize(horizontal: false, vertical: true)
    }

    private func activate() {
        guard !isWorking else {
            return
        }
        license.activate(key: keyInput)
    }
}
