import CoreLocation
import SwiftUI
import UIKit

struct LocationSettingsView: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.openURL) private var openURL
    @EnvironmentObject private var model: AppModel
    @EnvironmentObject private var reporter: LocationReporter
    @EnvironmentObject private var notifications: NotificationCoordinator
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
                Toggle("Report location", isOn: reportingBinding)
                    .disabled(
                        reporter.isWorking
                            || model.isDemo
                            || (!reporter.reportingEnabled && !model.connectionValidated)
                    )
                    .accessibilityIdentifier("location-reporting-toggle")
                LabeledContent("Queued", value: "\(reporter.queuedReportCount)")
                LabeledContent("Capture", value: captureLabel)
                if reporter.reportingEnabled {
                    Label("Location reporting is enabled", systemImage: "location")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
            } header: {
                Text("Reporting")
            } footer: {
                Text("Updates your location while you move and lets iOS pause when you stop. Background location uses more battery while moving. Visits are recorded separately for your history.")
            }

            Section {
                TimelineView(.periodic(from: .now, by: 30)) { context in
                    LabeledContent(
                        "Location",
                        value: reporter.presence?.displayLabel(at: context.date) ?? "No usable location"
                    )
                }
                if let presence = reporter.presence {
                    if let position = presence.position {
                        LabeledContent("Fix recorded", value: formattedTimestamp(position.observedAt))
                        LabeledContent("Accuracy", value: "±\(position.accuracyM.formatted(.number.precision(.fractionLength(0)))) m")
                        if let mapURL = position.mapURL {
                            Link(destination: mapURL) {
                                Label("Show reported position in Maps", systemImage: "map")
                            }
                        }
                    }
                    if let lastContact = presence.lastContact {
                        LabeledContent("Latest report", value: formattedTimestamp(lastContact))
                    }
                }
                if let lastUploadAt = reporter.lastUploadAt {
                    LabeledContent(
                        "Last upload",
                        value: lastUploadAt.formatted(date: .abbreviated, time: .shortened)
                    )
                }
                Button {
                    Task { await reporter.refreshLocation() }
                } label: {
                    if reporter.isRefreshingLocation {
                        ProgressView("Getting location…")
                    } else {
                        Label("Refresh location", systemImage: "arrow.clockwise")
                    }
                }
                .disabled(!reporter.reportingEnabled || !reporter.hasCredential || reporter.isWorking || reporter.isRefreshingLocation)
                if let result = refreshResultLabel {
                    Text(result)
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
            } header: {
                Text("Status")
            } footer: {
                Text("A last-known location keeps its original time. Approximate fixes may identify an area without identifying the place you are visiting.")
            }

            if reporter.reportingEnabled {
                Section {
                    LabeledContent("Connection", value: notifications.registrationState.label)
                    if let registeredAt = notifications.lastRegisteredAt {
                        LabeledContent("Registered", value: registeredAt.formatted(date: .abbreviated, time: .shortened))
                    }
                    if let error = notifications.lastError {
                        Label(error, systemImage: "exclamationmark.triangle")
                            .font(.footnote)
                            .foregroundStyle(BrunnTheme.amber)
                    }
                } header: {
                    Text("Background refresh recovery")
                } footer: {
                    Text("Brunn can ask this iPhone for an update if reports stop. iOS controls delivery, so a connected status does not confirm a fresh location.")
                }
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
            Label("Background location allowed", systemImage: "checkmark.shield")
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

    private var captureLabel: String {
        switch reporter.health.captureState {
        case "starting": "Starting location updates"
        case "tracking": "Receiving location updates"
        case "stationary": "Paused while stationary"
        case "awaiting_foreground": "Open Brunn to start updates"
        case "recovering": "Recovering location updates"
        default: "Stopped"
        }
    }

    private var refreshResultLabel: String? {
        switch reporter.health.lastRefreshResult {
        case "pending": "Waiting for a fresh fix and upload."
        case "timed_out": "The location refresh timed out. The last-known position is shown above."
        case "unavailable": "Location refresh is unavailable. Check location access and your Brunn connection."
        case "updated": "A fresh location was uploaded."
        case "approximate": "An approximate location was uploaded."
        case "upload_failed": "The fix could not be uploaded. It remains queued for retry."
        case "permission_required": "Always location access is required for reporting."
        case "cached_or_invalid_fix": "iOS returned an old or unusable fix. The last-known position is shown above."
        case "location_error": "iOS could not obtain a location. Try again."
        case "access_revoked": "Reconnect to Brunn to restore location access."
        case "stopped": "Location reporting is stopped."
        default: nil
        }
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
                Text("Let Brunn know where you are")
                    .font(.largeTitle.bold())
                Text("Brunn records the places you visit so your assistants know where you are and can answer questions about where you have been. Location is used only for your own Brunn workspace.")
                    .font(.body)
                VStack(alignment: .leading, spacing: 12) {
                    Label("Ask for When In Use access first", systemImage: "1.circle")
                    Label("Then ask for Always access", systemImage: "2.circle")
                    Label("Update while moving and pause when stationary", systemImage: "figure.walk")
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
