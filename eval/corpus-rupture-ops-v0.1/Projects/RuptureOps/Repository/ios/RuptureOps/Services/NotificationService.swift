import Foundation
import UIKit
import UserNotifications

struct NotificationDescriptor: Equatable, Sendable {
    let identifier: String
    let date: Date
    let title: String
    let body: String
    let cue: FeedbackCue
    let soundName: String?
    let soundEnabled: Bool
    let hapticsEnabled: Bool
    let audibleInSilentMode: Bool
}

enum NotificationPermissionState: String, Sendable {
    case notDetermined
    case denied
    case authorized
    case provisional
    case ephemeral
    case unknown

    var label: String {
        switch self {
        case .notDetermined: "Not enabled"
        case .denied: "Disabled in Settings"
        case .authorized: "Enabled"
        case .provisional: "Quiet delivery"
        case .ephemeral: "Temporary"
        case .unknown: "Unknown"
        }
    }
}

struct NotificationCapability: Equatable, Sendable {
    var permission: NotificationPermissionState
    var alertsEnabled: Bool
    var soundsEnabled: Bool
    var lockScreenEnabled: Bool

    static let unknown = NotificationCapability(
        permission: .unknown,
        alertsEnabled: false,
        soundsEnabled: false,
        lockScreenEnabled: false
    )
}

@MainActor
protocol NotificationScheduling: AnyObject {
    func capability() async -> NotificationCapability
    func requestAuthorization() async -> NotificationCapability
    func replacePending(with descriptors: [NotificationDescriptor]) async
    func cancelRuptureOpsNotifications() async
    func openSettings()
}

@MainActor
final class NotificationService: NotificationScheduling {
    static let identifierPrefix = "ruptureops."

    private let center: UNUserNotificationCenter

    init(center: UNUserNotificationCenter = .current()) {
        self.center = center
    }

    func capability() async -> NotificationCapability {
        let settings = await center.notificationSettings()
        return NotificationCapability(
            permission: permissionState(for: settings.authorizationStatus),
            alertsEnabled: settings.alertSetting == .enabled,
            soundsEnabled: settings.soundSetting == .enabled,
            lockScreenEnabled: settings.lockScreenSetting == .enabled
        )
    }

    func requestAuthorization() async -> NotificationCapability {
        do {
            _ = try await center.requestAuthorization(options: [.alert, .sound, .providesAppNotificationSettings])
        } catch {
            return await capability()
        }
        return await capability()
    }

    func replacePending(with descriptors: [NotificationDescriptor]) async {
        await cancelRuptureOpsNotifications()

        for descriptor in descriptors where descriptor.date > Date().addingTimeInterval(1) {
            let content = UNMutableNotificationContent()
            content.title = descriptor.title
            content.body = descriptor.body
            content.threadIdentifier = "rupture-cycle"
            content.userInfo = [
                "occurrenceID": descriptor.identifier,
                "cue": descriptor.cue.rawValue,
                "soundEnabled": descriptor.soundEnabled,
                "hapticsEnabled": descriptor.hapticsEnabled,
                "audibleInSilentMode": descriptor.audibleInSilentMode
            ]
            if let soundName = descriptor.soundName, descriptor.soundEnabled {
                content.sound = UNNotificationSound(named: UNNotificationSoundName(soundName))
            }

            let components = Calendar.current.dateComponents(
                in: TimeZone.current,
                from: descriptor.date
            )
            let trigger = UNCalendarNotificationTrigger(dateMatching: components, repeats: false)
            let request = UNNotificationRequest(
                identifier: descriptor.identifier,
                content: content,
                trigger: trigger
            )
            try? await center.add(request)
        }
    }

    func cancelRuptureOpsNotifications() async {
        let pending = await center.pendingNotificationRequests()
        let identifiers = pending
            .map(\.identifier)
            .filter { $0.hasPrefix(Self.identifierPrefix) }
        center.removePendingNotificationRequests(withIdentifiers: identifiers)
        center.removeDeliveredNotifications(withIdentifiers: identifiers)
    }

    func openSettings() {
        guard let url = URL(string: UIApplication.openNotificationSettingsURLString) else { return }
        UIApplication.shared.open(url)
    }

    private func permissionState(for status: UNAuthorizationStatus) -> NotificationPermissionState {
        switch status {
        case .notDetermined: .notDetermined
        case .denied: .denied
        case .authorized: .authorized
        case .provisional: .provisional
        case .ephemeral: .ephemeral
        @unknown default: .unknown
        }
    }
}

@MainActor
final class NoopNotificationService: NotificationScheduling {
    private(set) var descriptors: [NotificationDescriptor] = []
    var reportedCapability = NotificationCapability(
        permission: .authorized,
        alertsEnabled: true,
        soundsEnabled: true,
        lockScreenEnabled: true
    )

    func capability() async -> NotificationCapability { reportedCapability }
    func requestAuthorization() async -> NotificationCapability { reportedCapability }

    func replacePending(with descriptors: [NotificationDescriptor]) async {
        self.descriptors = descriptors
    }

    func cancelRuptureOpsNotifications() async {
        descriptors = []
    }

    func openSettings() {}
}

