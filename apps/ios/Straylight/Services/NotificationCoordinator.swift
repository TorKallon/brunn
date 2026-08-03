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

enum PushRegistrationState: Equatable {
    case unknown
    case notRegistered
    case registering
    case registered
    case revoking

    var label: String {
        switch self {
        case .unknown: "Not confirmed this launch"
        case .notRegistered: "Not registered"
        case .registering: "Registering…"
        case .registered: "Connected"
        case .revoking: "Disconnecting…"
        }
    }
}

@MainActor
final class NotificationCoordinator: ObservableObject {
    @Published private(set) var permissionState: PushPermissionState = .unknown
    @Published private(set) var hasPendingDeviceToken = false
    @Published private(set) var registrationState: PushRegistrationState = .unknown
    @Published private(set) var lastRegisteredAt: Date?
    @Published private(set) var lastError: String?

    let installationID: UUID

    private var pendingDeviceToken: Data?
    private let appID: String
    private let environment: String

    init(
        installationID: UUID? = nil,
        appID: String? = nil,
        environment: String? = nil
    ) {
        self.installationID = installationID ?? NotificationInstallationIdentity.loadOrCreate()
        self.appID = appID ?? Bundle.main.bundleIdentifier ?? "com.rourkem.straylight"
        self.environment = environment ?? Self.defaultEnvironment
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
        guard !token.isEmpty else { return }
        // Apple device tokens are forwarded from memory and are never logged or
        // persisted by the app. APNs may rotate them between registrations.
        pendingDeviceToken = token
        hasPendingDeviceToken = true
        lastError = nil
    }

    func receiveRegistrationFailure(_ error: Error) {
        lastError = error.localizedDescription
    }

    func synchronizeInstallation(
        using api: StraylightAPI,
        canManageNotifications: Bool
    ) async {
        guard canManageNotifications else { return }
        guard registrationState != .registering, let pendingDeviceToken else { return }
        await refreshAuthorizationStatus()
        guard permissionState == .enabled || permissionState == .provisional else { return }

        registrationState = .registering
        defer {
            if registrationState == .registering {
                registrationState = .notRegistered
            }
        }
        do {
            _ = try await api.upsertNotificationInstallation(
                installationID: installationID,
                request: NotificationInstallationRequest(
                    environment: environment,
                    appID: appID,
                    deviceToken: pendingDeviceToken.map { String(format: "%02x", $0) }.joined()
                )
            )
            self.pendingDeviceToken = nil
            hasPendingDeviceToken = false
            registrationState = .registered
            lastRegisteredAt = .now
            lastError = nil
        } catch {
            registrationState = .notRegistered
            lastError = "This iPhone could not be registered for notifications. \(error.localizedDescription)"
        }
    }

    func revokeInstallation(
        using api: StraylightAPI,
        canManageNotifications: Bool
    ) async -> Bool {
        guard canManageNotifications, registrationState != .revoking else { return false }
        let previousState = registrationState
        registrationState = .revoking
        do {
            _ = try await api.revokeNotificationInstallation(installationID: installationID)
            registrationState = .notRegistered
            lastRegisteredAt = nil
            lastError = nil
            return true
        } catch {
            registrationState = previousState
            lastError = "The notification installation could not be revoked. \(error.localizedDescription)"
            return false
        }
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

    static var defaultEnvironment: String {
        (Bundle.main.object(forInfoDictionaryKey: "StraylightAPNSEnvironment") as? String)
            ?? "development"
    }
}

enum NotificationRouteParser {
    static func route(from userInfo: [AnyHashable: Any]) -> AppRoute? {
        guard
            userInfo["schema"] as? String == "straylight-push@v1",
            let notificationRef = userInfo["notification_ref"] as? String,
            PushReference.isNotification(notificationRef),
            let deliveryRef = userInfo["delivery_ref"] as? String,
            PushReference.isDelivery(deliveryRef),
            let rawRoute = userInfo["straylight_route"] as? String,
            let url = URL(string: rawRoute),
            case let .notification(routeNotificationRef, routeDeliveryRef) = AppRoute(url: url),
            routeNotificationRef == notificationRef,
            routeDeliveryRef == deliveryRef
        else {
            return nil
        }
        return .notification(notificationRef: notificationRef, deliveryRef: deliveryRef)
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

final class PushTokenBuffer: @unchecked Sendable {
    static let shared = PushTokenBuffer()

    private let lock = NSLock()
    private var pendingToken: Data?

    private init() {}

    func store(_ token: Data) {
        lock.lock()
        pendingToken = token
        lock.unlock()
    }

    func take() -> Data? {
        lock.lock()
        defer { lock.unlock() }
        let token = pendingToken
        pendingToken = nil
        return token
    }
}

private enum NotificationInstallationIdentity {
    private static let key = "straylight.notification-installation-id"

    static func loadOrCreate(defaults: UserDefaults = .standard) -> UUID {
        if let value = defaults.string(forKey: key), let identifier = UUID(uuidString: value) {
            return identifier
        }
        let identifier = UUID()
        defaults.set(identifier.uuidString.lowercased(), forKey: key)
        return identifier
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
        PushTokenBuffer.shared.store(deviceToken)
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
