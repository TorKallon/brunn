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
        self.appID = appID ?? Bundle.main.bundleIdentifier ?? "com.rourkem.brunn"
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
        using api: BrunnAPI,
        canManageNotifications: Bool,
        bearerToken: String?
    ) async {
        guard canManageNotifications, let bearerToken else { return }
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
                ),
                bearerToken: bearerToken
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
        using api: BrunnAPI,
        canManageNotifications: Bool,
        bearerToken: String?
    ) async -> Bool {
        guard canManageNotifications,
              let bearerToken,
              registrationState != .revoking
        else { return false }
        let previousState = registrationState
        registrationState = .revoking
        do {
            _ = try await api.revokeNotificationInstallation(
                installationID: installationID,
                bearerToken: bearerToken
            )
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
        (Bundle.main.object(forInfoDictionaryKey: "BrunnAPNSEnvironment") as? String)
            ?? "development"
    }
}

enum NotificationRouteParser {
    static func route(from userInfo: [AnyHashable: Any]) -> AppRoute? {
        guard
            userInfo["schema"] as? String == "brunn-push@v1",
            let notificationRef = userInfo["notification_ref"] as? String,
            PushReference.isNotification(notificationRef),
            let deliveryRef = userInfo["delivery_ref"] as? String,
            PushReference.isDelivery(deliveryRef),
            let rawRoute = userInfo["brunn_route"] as? String,
            let url = URL(string: rawRoute),
            let route = AppRoute(url: url)
        else {
            return nil
        }
        switch route {
        case let .notification(routeNotificationRef, routeDeliveryRef)
            where routeNotificationRef == notificationRef && routeDeliveryRef == deliveryRef:
            return route
        case .task, .conversation:
            return route
        default:
            return nil
        }
    }

    static func isMessagingPrefetch(_ userInfo: [AnyHashable: Any]) -> Bool {
        guard case .conversation = route(from: userInfo),
              let aps = userInfo["aps"] as? [AnyHashable: Any]
        else { return false }
        if let contentAvailable = aps["content-available"] as? Int {
            return contentAvailable == 1
        }
        if let contentAvailable = aps["content-available"] as? NSNumber {
            return contentAvailable.intValue == 1
        }
        return false
    }
}

extension Notification.Name {
    static let brunnPushRoute = Notification.Name("brunn.push-route")
    static let brunnPushToken = Notification.Name("brunn.push-token")
    static let brunnPushRegistrationFailed = Notification.Name("brunn.push-registration-failed")
    static let brunnMessagingPrefetch = Notification.Name("brunn.messaging-prefetch")
}

final class MessagingBackgroundPrefetch: @unchecked Sendable {
    private let lock = NSLock()
    private var completionHandler: ((UIBackgroundFetchResult) -> Void)?

    init(completionHandler: @escaping (UIBackgroundFetchResult) -> Void) {
        self.completionHandler = completionHandler
    }

    func finish(_ result: UIBackgroundFetchResult) {
        lock.lock()
        let completionHandler = completionHandler
        self.completionHandler = nil
        lock.unlock()
        completionHandler?(result)
    }
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

enum NotificationDelegateHandoff {
    static func finishPresentation(
        _ options: UNNotificationPresentationOptions,
        completionHandler: @escaping @Sendable (UNNotificationPresentationOptions) -> Void
    ) {
        DispatchQueue.main.async {
            completionHandler(options)
        }
    }

    static func finishResponse(
        route: AppRoute?,
        completionHandler: @escaping @Sendable () -> Void
    ) {
        if let route {
            PushRouteBuffer.shared.store(route)
        }

        DispatchQueue.main.async {
            // UIKit continues background response and state restoration on the
            // queue that calls this closure. Keep that continuation on main.
            completionHandler()

            // Defer navigation until the completion call and its synchronous
            // UIKit work have returned. An inactive scene retains the buffered
            // route and consumes it when the scene becomes active.
            guard route != nil else { return }
            DispatchQueue.main.async {
                NotificationCenter.default.post(name: .brunnPushRoute, object: nil)
            }
        }
    }

    static func finishBackgroundFetch(
        shouldPrefetch: Bool,
        completionHandler: @escaping (UIBackgroundFetchResult) -> Void
    ) {
        let request = MessagingBackgroundPrefetch(completionHandler: completionHandler)
        guard shouldPrefetch else {
            DispatchQueue.main.async {
                request.finish(.noData)
            }
            return
        }

        DispatchQueue.main.async {
            NotificationCenter.default.post(
                name: .brunnMessagingPrefetch,
                object: request
            )
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 25) {
            request.finish(.failed)
        }
    }
}

private enum NotificationInstallationIdentity {
    private static let key = "brunn.notification-installation-id"

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
    nonisolated static func handlesBackgroundURLSession(identifier: String) -> Bool {
        MessagingBackgroundTransport.handlesBackgroundSession(identifier: identifier)
    }

    func application(
        _: UIApplication,
        didFinishLaunchingWithOptions _: [UIApplication.LaunchOptionsKey: Any]? = nil
    ) -> Bool {
        UNUserNotificationCenter.current().delegate = self
        return true
    }

    func application(
        _: UIApplication,
        handleEventsForBackgroundURLSession identifier: String,
        completionHandler: @escaping () -> Void
    ) {
        guard Self.handlesBackgroundURLSession(identifier: identifier),
              MessagingBackgroundTransport.handleBackgroundEvents(
                  identifier: identifier,
                  completionHandler: completionHandler
              )
        else {
            DispatchQueue.main.async {
                completionHandler()
            }
            return
        }
    }

    func application(
        _: UIApplication,
        didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data
    ) {
        PushTokenBuffer.shared.store(deviceToken)
        NotificationCenter.default.post(name: .brunnPushToken, object: deviceToken)
    }

    func application(
        _: UIApplication,
        didFailToRegisterForRemoteNotificationsWithError error: Error
    ) {
        NotificationCenter.default.post(name: .brunnPushRegistrationFailed, object: error)
    }

    func application(
        _: UIApplication,
        didReceiveRemoteNotification userInfo: [AnyHashable: Any],
        fetchCompletionHandler completionHandler: @escaping (UIBackgroundFetchResult) -> Void
    ) {
        NotificationDelegateHandoff.finishBackgroundFetch(
            shouldPrefetch: NotificationRouteParser.isMessagingPrefetch(userInfo),
            completionHandler: completionHandler
        )
    }

    nonisolated func userNotificationCenter(
        _: UNUserNotificationCenter,
        willPresent _: UNNotification,
        withCompletionHandler completionHandler: @escaping @Sendable (UNNotificationPresentationOptions) -> Void
    ) {
        NotificationDelegateHandoff.finishPresentation(
            [.banner, .list, .sound],
            completionHandler: completionHandler
        )
    }

    nonisolated func userNotificationCenter(
        _: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse,
        withCompletionHandler completionHandler: @escaping @Sendable () -> Void
    ) {
        NotificationDelegateHandoff.finishResponse(
            route: NotificationRouteParser.route(
                from: response.notification.request.content.userInfo
            ),
            completionHandler: completionHandler
        )
    }
}
