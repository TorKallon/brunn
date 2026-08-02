import UIKit
@preconcurrency import UserNotifications

enum PushPermissionState: Equatable {
    case unknown
    case denied
    case enabled
    case provisional

    var label: String {
        switch self {
        case .unknown: "Not enabled"
        case .denied: "Disabled in Settings"
        case .enabled: "Enabled on this device"
        case .provisional: "Delivering quietly"
        }
    }
}

@MainActor
final class NotificationCoordinator: ObservableObject {
    @Published private(set) var permissionState: PushPermissionState = .unknown
    @Published private(set) var hasPendingDeviceToken = false
    @Published private(set) var lastError: String?

    init() {
        Task { await registerForRemoteNotificationsIfAuthorized() }
    }

    func requestPermission() async {
        do {
            let granted = try await UNUserNotificationCenter.current().requestAuthorization(
                options: [.alert, .badge, .sound]
            )
            await refreshAuthorizationStatus()
            if granted {
                UIApplication.shared.registerForRemoteNotifications()
            }
        } catch {
            lastError = error.localizedDescription
        }
    }

    func receiveDeviceToken(_ token: Data) {
        // The token remains in memory until Straylight exposes an authenticated
        // device-registration contract. Never print or persist it locally.
        hasPendingDeviceToken = !token.isEmpty
    }

    func receiveRegistrationFailure(_ error: Error) {
        lastError = error.localizedDescription
    }

    func refreshAuthorizationStatus() async {
        let settings = await UNUserNotificationCenter.current().notificationSettings()
        switch settings.authorizationStatus {
        case .authorized, .ephemeral:
            permissionState = .enabled
        case .provisional:
            permissionState = .provisional
        case .denied:
            permissionState = .denied
        case .notDetermined:
            permissionState = .unknown
        @unknown default:
            permissionState = .unknown
        }
    }

    private func registerForRemoteNotificationsIfAuthorized() async {
        await refreshAuthorizationStatus()
        if permissionState == .enabled || permissionState == .provisional {
            UIApplication.shared.registerForRemoteNotifications()
        }
    }
}

enum NotificationRouteParser {
    static func route(from userInfo: [AnyHashable: Any]) -> AppRoute? {
        guard
            let rawRoute = userInfo["straylight_route"] as? String,
            let url = URL(string: rawRoute)
        else {
            return nil
        }
        return AppRoute(url: url)
    }
}

extension Notification.Name {
    static let straylightPushRoute = Notification.Name("straylight.push-route")
    static let straylightPushToken = Notification.Name("straylight.push-token")
    static let straylightPushRegistrationFailed = Notification.Name("straylight.push-registration-failed")
}

final class PushRouteBuffer: @unchecked Sendable {
    static let shared = PushRouteBuffer()

    private let lock = NSLock()
    private var pendingRoute: AppRoute?

    private init() {}

    func store(_ route: AppRoute) {
        lock.lock()
        pendingRoute = route
        lock.unlock()
    }

    func take() -> AppRoute? {
        lock.lock()
        defer { lock.unlock() }
        let route = pendingRoute
        pendingRoute = nil
        return route
    }
}

final class AppDelegate: NSObject, UIApplicationDelegate, UNUserNotificationCenterDelegate {
    func application(
        _: UIApplication,
        didFinishLaunchingWithOptions _: [UIApplication.LaunchOptionsKey: Any]? = nil
    ) -> Bool {
        UNUserNotificationCenter.current().delegate = self
        return true
    }

    func application(
        _: UIApplication,
        didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data
    ) {
        NotificationCenter.default.post(name: .straylightPushToken, object: deviceToken)
    }

    func application(
        _: UIApplication,
        didFailToRegisterForRemoteNotificationsWithError error: Error
    ) {
        NotificationCenter.default.post(name: .straylightPushRegistrationFailed, object: error)
    }

    nonisolated func userNotificationCenter(
        _: UNUserNotificationCenter,
        willPresent _: UNNotification
    ) async -> UNNotificationPresentationOptions {
        [.banner, .list, .sound]
    }

    nonisolated func userNotificationCenter(
        _: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse
    ) async {
        guard let route = NotificationRouteParser.route(
            from: response.notification.request.content.userInfo
        ) else { return }
        PushRouteBuffer.shared.store(route)
        await MainActor.run {
            NotificationCenter.default.post(name: .straylightPushRoute, object: nil)
        }
    }
}
