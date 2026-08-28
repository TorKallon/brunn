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
                LabeledContent("Alerts source", value: "Durable Straylight inbox")
                LabeledContent("Push service", value: notifications.registrationState.label)
            } header: {
                Text("Briefing delivery")
            } footer: {
                Text("Alerts remain in Straylight even when APNs is delayed, denied, or unavailable. Provider acceptance is never labeled as device delivery.")
            }

            Section("Connection") {
                LabeledContent("Account", value: model.user?.displayName ?? "Owner")
                LabeledContent("Server", value: "straylight.rourkem.com")
                LabeledContent("Access", value: accessLabel)
            }

            Section {
                LabeledContent(
                    "Status",
                    value: model.canWriteTasks ? "Connected" : "View only"
                )
                if model.canWriteTasks {
                    Label("task.write + notification.manage only", systemImage: "checkmark.shield")
                        .font(.footnote)
                        .foregroundStyle(StraylightTheme.success)
                    Button("Revoke device task access", role: .destructive) {
                        Task {
                            if model.canManageNotifications {
                                _ = await notifications.revokeInstallation(
                                    using: model.api,
                                    canManageNotifications: true,
                                    bearerToken: model.deviceTaskBearer()
                                )
                            }
                            _ = await model.revokeDeviceTaskAccess()
                        }
                    }
                    .accessibilityIdentifier("device-task-access-revoke")
                } else if model.hasStoredDeviceTaskCredential, !model.isDemo {
                    Label("Protected credential disabled", systemImage: "exclamationmark.shield")
                        .font(.footnote)
                        .foregroundStyle(StraylightTheme.amber)
                    Button("Retry server revocation", role: .destructive) {
                        Task { _ = await model.revokeDeviceTaskAccess() }
                    }
                    .accessibilityIdentifier("device-task-access-retry-revoke")
                } else if !model.isDemo {
                    Button {
                        Task {
                            await model.bootstrapDeviceTaskAccess()
                            await notifications.synchronizeInstallation(
                                using: model.api,
                                canManageNotifications: model.canManageNotifications,
                                bearerToken: model.deviceTaskBearer()
                            )
                        }
                    } label: {
                        if model.isConfiguringDeviceTaskAccess {
                            ProgressView("Securing device access…")
                        } else {
                            Label("Secure device task access", systemImage: "key.viewfinder")
                        }
                    }
                    .disabled(!model.connectionValidated || model.isConfiguringDeviceTaskAccess)
                    .accessibilityIdentifier("device-task-access-bootstrap")
                }
                if let message = model.deviceTaskAccessMessage {
                    Text(message)
                        .font(.footnote)
                        .foregroundStyle(model.canWriteTasks ? StraylightTheme.success : StraylightTheme.amber)
                }
            } header: {
                Text("Device task access")
            } footer: {
                Text("Creates a one-time opaque credential and stores it in this iPhone's protected Keychain. It cannot read workspace content or manage credentials, and its token is never displayed.")
            }

            Section("Notifications") {
                LabeledContent("Permission", value: notifications.permissionState.label)
                LabeledContent("Installation", value: notifications.registrationState.label)
                if notifications.hasPendingDeviceToken {
                    Label("The current APNs token is waiting for authenticated registration.", systemImage: "server.rack")
                        .font(.footnote)
                        .foregroundStyle(StraylightTheme.amber)
                }
                if !model.canManageNotifications, !model.isDemo {
                    Label("This account can read Alerts but needs notification management access to register this iPhone.", systemImage: "key")
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
                    Label("Set up notifications", systemImage: "bell.badge")
                }
                .disabled(!model.canManageNotifications && !model.isDemo)
                if model.canManageNotifications, !model.isDemo {
                    Button("Disable push on this iPhone", role: .destructive) {
                        Task {
                            _ = await notifications.revokeInstallation(
                                using: model.api,
                                canManageNotifications: model.canManageNotifications,
                                bearerToken: model.deviceTaskBearer()
                            )
                        }
                    }
                }
                Text("Permission is requested only after you choose setup. Push payloads contain generic prose and opaque references; private detail is fetched after authenticated open.")
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
                    Task {
                        if !model.isDemo, model.canManageNotifications {
                            let revoked = await notifications.revokeInstallation(
                                using: model.api,
                                canManageNotifications: model.canManageNotifications,
                                bearerToken: model.deviceTaskBearer()
                            )
                            guard revoked else { return }
                            guard await model.revokeDeviceTaskAccess() else { return }
                        }
                        await model.disconnect()
                    }
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
    @EnvironmentObject private var model: AppModel
    @EnvironmentObject private var notifications: NotificationCoordinator

    var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 20) {
                Image(systemName: "bell.badge")
                    .font(.system(size: 42, weight: .medium))
                    .foregroundStyle(StraylightTheme.signal)
                Text("Useful previews, private detail after open")
                    .font(.largeTitle.bold())
                Text("Operational alerts can show a short preview on the lock screen. Briefings, material news, and corrections keep generic preview text. The app then authenticates and opens the durable alert detail.")
                    .font(.body)
                VStack(alignment: .leading, spacing: 12) {
                    Label("Register this installation with an opaque identifier", systemImage: "iphone.gen3")
                    Label("Preview operational alert text", systemImage: "text.bubble")
                    Label("Keep other lock-screen text generic", systemImage: "lock")
                    Label("Resolve private content only after authenticated open", systemImage: "arrow.down.doc")
                    Label("Record APNs attempts, opens, and acknowledgements", systemImage: "checkmark.seal")
                }
                .font(.subheadline)
                .foregroundStyle(.secondary)
                Spacer()
                Button("Enable notifications") {
                    Task {
                        await notifications.requestPermission()
                        await notifications.synchronizeInstallation(
                            using: model.api,
                            canManageNotifications: model.canManageNotifications,
                            bearerToken: model.deviceTaskBearer()
                        )
                        if notifications.permissionState != .unknown {
                            dismiss()
                        }
                    }
                }
                    .buttonStyle(.borderedProminent)
                    .frame(maxWidth: .infinity, minHeight: 44)
                    .disabled(!model.canManageNotifications && !model.isDemo)
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
