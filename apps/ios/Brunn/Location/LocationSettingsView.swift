import CoreLocation
import SwiftUI
import UIKit

struct LocationSettingsView: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.openURL) private var openURL
    @EnvironmentObject private var model: AppModel
    @EnvironmentObject private var reporter: LocationReporter
    @State private var showingPrimer = false

    var body: some View {
        List {
            Section {
                LabeledContent("Access", value: permissionLabel)
                permissionStatus
                Button {
                    if permissionNeedsSettings {
                        openURL(URL(string: UIApplication.openSettingsURLString)!)
                    } else {
                        showingPrimer = true
                    }
                } label: {
                    Label(
                        permissionNeedsSettings ? "Open Settings" : "Set up location access",
                        systemImage: permissionNeedsSettings ? "gear" : "location.circle"
                    )
                }
                .disabled((!model.connectionValidated || model.isDemo) && !permissionNeedsSettings)
            } header: {
                Text("Permission")
            } footer: {
                Text("Brunn asks for When In Use access first, then Always access. Limited access cannot record visits reliably in the background.")
            }

            Section {
                Toggle("Report visits", isOn: reportingBinding)
                    .disabled(
                        reporter.isWorking
                            || model.isDemo
                            || (!reporter.reportingEnabled && !model.connectionValidated)
                    )
                    .accessibilityIdentifier("location-reporting-toggle")
                LabeledContent("Queued", value: "\(reporter.queuedReportCount)")
                if reporter.reportingEnabled {
                    Label("Visit reporting is active", systemImage: "checkmark.circle")
                        .font(.footnote)
                        .foregroundStyle(BrunnTheme.success)
                }
            } header: {
                Text("Reporting")
            } footer: {
                Text("Uses iOS visit monitoring and significant location changes only. Brunn does not run continuous GPS updates or a background timer.")
            }

            Section {
                LabeledContent("Presence", value: presenceLabel)
                if let presence = reporter.presence {
                    LabeledContent("Last seen", value: formattedTimestamp(presence.lastSeen))
                    if let city = presence.city {
                        LabeledContent("City", value: city)
                    }
                }
                if let lastUploadAt = reporter.lastUploadAt {
                    LabeledContent(
                        "Last upload",
                        value: lastUploadAt.formatted(date: .abbreviated, time: .shortened)
                    )
                }
                Button {
                    Task { await reporter.refreshPresence() }
                } label: {
                    Label("Refresh", systemImage: "arrow.clockwise")
                }
                .disabled(!reporter.hasCredential || reporter.isWorking)
            } header: {
                Text("Status")
            }

            Section {
                Text("Brunn stores raw location reports for 30 days in your Brunn account so it can derive current presence and visit history. Precise location is linked only to your Brunn account and is not used for tracking or advertising.")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                Button("Delete live location data", role: .destructive) {
                    Task { await reporter.deleteLiveData() }
                }
                .disabled(!reporter.hasCredential || reporter.isWorking)
            } header: {
                Text("Data")
            }

            if let error = reporter.lastError {
                Section("Needs attention") {
                    Label(error, systemImage: "exclamationmark.triangle")
                        .font(.footnote)
                        .foregroundStyle(BrunnTheme.red)
                }
            }
        }
        .navigationTitle("Location")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarLeading) { BrandMark() }
        }
        .sheet(isPresented: $showingPrimer) {
            LocationPrimerView()
        }
        .transaction { transaction in
            if reduceMotion {
                transaction.animation = nil
            }
        }
    }

    @ViewBuilder
    private var permissionStatus: some View {
        switch reporter.authorizationStatus {
        case .authorizedAlways:
            Label("Ready for background visits", systemImage: "checkmark.shield")
                .font(.footnote)
                .foregroundStyle(BrunnTheme.success)
        case .authorizedWhenInUse:
            Label("Limited to foreground use", systemImage: "exclamationmark.shield")
                .font(.footnote)
                .foregroundStyle(BrunnTheme.amber)
        case .denied, .restricted:
            Label("Location access is off", systemImage: "location.slash")
                .font(.footnote)
                .foregroundStyle(BrunnTheme.red)
        case .notDetermined:
            Label("Not requested", systemImage: "location")
                .font(.footnote)
                .foregroundStyle(.secondary)
        @unknown default:
            Label("Unknown", systemImage: "questionmark.circle")
                .font(.footnote)
                .foregroundStyle(BrunnTheme.amber)
        }
    }

    private var reportingBinding: Binding<Bool> {
        Binding(
            get: { reporter.reportingEnabled },
            set: { enabled in
                if enabled {
                    showingPrimer = true
                } else {
                    Task { await reporter.disableReporting() }
                }
            }
        )
    }

    private var permissionNeedsSettings: Bool {
        reporter.authorizationStatus == .denied || reporter.authorizationStatus == .restricted
    }

    private var permissionLabel: String {
        switch reporter.authorizationStatus {
        case .notDetermined: "Not requested"
        case .restricted: "Restricted"
        case .denied: "Denied"
        case .authorizedWhenInUse: "Limited"
        case .authorizedAlways: "Always"
        @unknown default: "Unknown"
        }
    }

    private var presenceLabel: String {
        guard let presence = reporter.presence else { return "No live location" }
        if let label = presence.place?.label, !label.isEmpty { return label }
        switch presence.status {
        case "at_place": return "At a place"
        case "between_places": return "Between places"
        case "stale": return "Stale"
        default: return presence.status.replacingOccurrences(of: "_", with: " ").capitalized
        }
    }

    private func formattedTimestamp(_ value: String) -> String {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        let date = formatter.date(from: value)
            ?? ISO8601DateFormatter().date(from: value)
        return date?.formatted(date: .abbreviated, time: .shortened) ?? value
    }
}

struct LocationPrimerView: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(\.openURL) private var openURL
    @EnvironmentObject private var model: AppModel
    @EnvironmentObject private var reporter: LocationReporter
    let onFinish: () -> Void

    init(onFinish: @escaping () -> Void = {}) {
        self.onFinish = onFinish
    }

    var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 20) {
                Image(systemName: "location.north.circle")
                    .font(.system(size: 42, weight: .medium))
                    .foregroundStyle(BrunnTheme.signal)
                Text("Let Brunn remember where you have been")
                    .font(.largeTitle.bold())
                Text("Brunn records the places you visit so your assistants know where you are and can answer questions about where you have been. Location is used only for your own Brunn workspace.")
                    .font(.body)
                VStack(alignment: .leading, spacing: 12) {
                    Label("Ask for When In Use access first", systemImage: "1.circle")
                    Label("Then ask for Always access", systemImage: "2.circle")
                    Label("Record visits and significant changes only", systemImage: "figure.walk")
                    Label("Keep reports queued when the network is unavailable", systemImage: "tray.full")
                }
                .font(.subheadline)
                .foregroundStyle(.secondary)
                Text("You can change this any time in Settings → Location.")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                if let error = reporter.lastError {
                    Label(error, systemImage: "exclamationmark.triangle")
                        .font(.footnote)
                        .foregroundStyle(BrunnTheme.red)
                        .accessibilityIdentifier("location-permission-error")
                }
                Spacer()
                Button {
                    performPrimaryAction()
                } label: {
                    if reporter.isWorking {
                        ProgressView("Preparing…")
                            .frame(maxWidth: .infinity)
                    } else {
                        Text(primaryActionLabel)
                            .frame(maxWidth: .infinity)
                    }
                }
                .buttonStyle(.borderedProminent)
                .tint(BrunnTheme.signal)
                .disabled(primaryActionDisabled)
                .accessibilityIdentifier("location-permission-primary-action")
            }
            .padding(24)
            .accessibilityIdentifier("location-permission-primer")
            .navigationTitle("Location setup")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Not now") {
                        onFinish()
                        dismiss()
                    }
                    .accessibilityIdentifier("location-permission-not-now")
                }
            }
        }
    }

    private var primaryAction: LocationPermissionPromptAction {
        LocationPermissionPromptPolicy.primaryAction(
            for: LocationPermissionState(reporter.authorizationStatus)
        )
    }

    private var primaryActionLabel: String {
        switch primaryAction {
        case .openSettings:
            "Open iPhone Settings"
        case .beginEnable:
            switch LocationPermissionState(reporter.authorizationStatus) {
            case .whenInUse:
                "Allow Always Access"
            case .always:
                "Start reporting"
            default:
                "Continue"
            }
        case .unavailable:
            "Continue"
        }
    }

    private var primaryActionDisabled: Bool {
        guard !reporter.isWorking else { return true }
        switch primaryAction {
        case .openSettings:
            return false
        case .beginEnable:
            return !model.connectionValidated || model.isDemo
        case .unavailable:
            return true
        }
    }

    private func performPrimaryAction() {
        guard primaryAction != .unavailable else { return }
        let userID = model.user?.id ?? ""
        Task {
            switch primaryAction {
            case .beginEnable:
                await reporter.beginEnable(
                    ownerAPI: model.api,
                    expectedUserID: userID
                ) {
                    onFinish()
                    dismiss()
                }
            case .openSettings:
                guard await reporter.prepareEnableFromSettings(
                    ownerAPI: model.api,
                    expectedUserID: userID
                ) else { return }
                onFinish()
                dismiss()
                openURL(URL(string: UIApplication.openSettingsURLString)!)
            case .unavailable:
                break
            }
        }
    }
}
