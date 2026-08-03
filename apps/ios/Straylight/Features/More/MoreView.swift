import SwiftUI

struct MoreView: View {
    @EnvironmentObject private var model: AppModel
    @EnvironmentObject private var notifications: NotificationCoordinator
    @AppStorage(AppAppearance.storageKey) private var appearanceRawValue =
        AppAppearance.defaultValue.rawValue
    @State private var showingNotificationPrimer = false

    var body: some View {
        List {
            Section {
                Picker("Color mode", selection: $appearanceRawValue) {
                    ForEach(AppAppearance.allCases) { appearance in
                        Text(appearance.label).tag(appearance.rawValue)
                    }
                }
                .pickerStyle(.segmented)
                .accessibilityIdentifier("appearance-mode")
            } header: {
                Text("Appearance")
            } footer: {
                Text("Saved on this iPhone and applied to every Straylight screen.")
            }

            Section {
                LabeledContent("News source", value: "Published briefing activity")
                LabeledContent("Push service", value: "Not connected")
            } header: {
                Text("Briefing delivery")
            } footer: {
                Text("The current server publishes briefings and topic activity, but it does not yet expose APNs device registration, a delivery inbox, or receipts. The app does not mislabel briefing inclusion as phone delivery.")
            }

            Section("Connection") {
                LabeledContent("Account", value: model.user?.displayName ?? "Owner")
                LabeledContent("Server", value: "straylight.rourkem.com")
                LabeledContent("Access", value: accessLabel)
            }

            Section("Notifications") {
                LabeledContent("Permission", value: notifications.permissionState.label)
                if notifications.hasPendingDeviceToken {
                    Label("Device token is held only in memory; no server registration endpoint exists.", systemImage: "server.rack")
                        .font(.footnote)
                        .foregroundStyle(StraylightTheme.amber)
                }
                if let error = notifications.lastError {
                    Text(error)
                        .font(.footnote)
                        .foregroundStyle(StraylightTheme.red)
                }
                Button {
                    showingNotificationPrimer = true
                } label: {
                    Label("Notification readiness", systemImage: "bell.badge")
                }
                Text("The one-time iOS permission prompt stays disabled until Straylight can register this installation and complete a signed-device canary.")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }

            Section("Data & privacy") {
                LabeledContent("Offline data", value: offlineDataLabel)
                Button("Clear protected briefing cache") {
                    Task { await model.clearBriefingCache() }
                }
                .disabled(model.cacheSavedAt == nil)
                if let privacyMessage = model.privacyMessage {
                    Label(privacyMessage, systemImage: "exclamationmark.shield")
                        .font(.footnote)
                        .foregroundStyle(StraylightTheme.red)
                }
                Text("Search strings are not persisted. The app never creates a full-corpus cache, local embeddings, or a second task database.")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }

            Section {
                Button(model.isDemo ? "Leave demo" : "Disconnect this iPhone", role: .destructive) {
                    Task { await model.disconnect() }
                }
            } footer: {
                Text(model.isDemo
                    ? "Leaving the demo returns to the sign-in screen."
                    : "This signs out this iPhone and removes its protected briefing cache. Other signed-in devices stay connected.")
            }

            Section {
                LabeledContent("App", value: "Straylight 0.2.0 MVP")
                LabeledContent("Platform", value: "Native SwiftUI")
            }
        }
        .navigationTitle("Settings")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarLeading) { BrandMark() }
        }
        .sheet(isPresented: $showingNotificationPrimer) {
            NotificationPrimerView()
        }
    }

    private var accessLabel: String {
        if model.isDemo { return "Demo" }
        return model.connectionValidated ? "Signed in" : "Offline snapshot · unvalidated"
    }

    private var offlineDataLabel: String {
        guard let savedAt = model.cacheSavedAt else { return "No saved briefing" }
        return "Latest briefing · \(savedAt.formatted(date: .omitted, time: .shortened))"
    }
}

private struct NotificationPrimerView: View {
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 20) {
                Image(systemName: "bell.badge")
                    .font(.system(size: 42, weight: .medium))
                    .foregroundStyle(StraylightTheme.signal)
                Text("Push is waiting on the delivery service")
                    .font(.largeTitle.bold())
                Text("The native reader is complete for published morning briefings, intraday activity, revisions, sources, and tracked topics. Remote notifications remain off because Straylight has no APNs device or delivery API yet.")
                    .font(.body)
                VStack(alignment: .leading, spacing: 12) {
                    Label("Register each installation with an opaque identifier", systemImage: "iphone.gen3")
                    Label("Keep default lock-screen text generic", systemImage: "lock")
                    Label("Resolve private content only after authenticated open", systemImage: "arrow.down.doc")
                    Label("Record APNs attempts, opens, and acknowledgements", systemImage: "checkmark.seal")
                }
                .font(.subheadline)
                .foregroundStyle(.secondary)
                Spacer()
                Button("Done") { dismiss() }
                    .buttonStyle(.borderedProminent)
                    .frame(maxWidth: .infinity, minHeight: 44)
            }
            .padding(24)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Close") { dismiss() }
                }
            }
        }
        .presentationDetents([.large])
    }
}
