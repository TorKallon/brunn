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

    // Retain only in memory so account/permission transitions can register the
    // same actual-app token again without depending on another APNs callback.
    private var deviceToken: Data?
    private var registeredAccountUserID: String?
    private let appID: String
    private let environment: String
    private let registerWithAPNs: () -> Void
    private let loadPermissionState: () async -> PushPermissionState
    private var registrationOperationHeld = false
    private var registrationOperationWaiters: [CheckedContinuation<Void, Never>] = []
    private var registrationGeneration = 0

    init(
        installationID: UUID? = nil,
        appID: String? = nil,
        environment: String? = nil,
        registerWithAPNs: @escaping () -> Void = {
            UIApplication.shared.registerForRemoteNotifications()
        },
        loadPermissionState: @escaping () async -> PushPermissionState = {
            await NotificationCoordinator.systemPermissionState()
        }
    ) {
        self.installationID = installationID ?? NotificationInstallationIdentity.loadOrCreate()
        self.appID = appID ?? Bundle.main.bundleIdentifier ?? "com.rourkem.brunn"
        self.environment = environment ?? Self.defaultEnvironment
        self.registerWithAPNs = registerWithAPNs
        self.loadPermissionState = loadPermissionState
        requestRemoteRegistration()
        Task { await refreshAuthorizationStatus() }
    }

    func requestPermission() async {
        do {
            let granted = try await UNUserNotificationCenter.current().requestAuthorization(
                options: [.alert, .badge, .sound]
            )
            await refreshAuthorizationStatus()
            if granted {
                requestRemoteRegistration()
            }
        } catch {
            lastError = error.localizedDescription
        }
    }

    func receiveDeviceToken(_ token: Data) {
        guard !token.isEmpty else { return }
        // Apple device tokens are forwarded from memory and are never logged or
        // persisted by the app. APNs may rotate them between registrations.
        if deviceToken != token || registrationState != .registered {
            hasPendingDeviceToken = true
        }
        deviceToken = token
        lastError = nil
    }

    func receiveRegistrationFailure(_ error: Error) {
        registrationState = .notRegistered
        lastError = error.localizedDescription
    }

    func requestRemoteRegistration() {
        // Silent APNs registration does not require alert, sound or badge
        // permission. The server upload still requires authenticated access.
        registerWithAPNs()
    }

    func synchronizeLocationRecovery(
        using api: BrunnAPI,
        accountUserID: String?,
        reportingEnabled: Bool
    ) async {
        let generation = registrationGeneration
        guard reportingEnabled, let accountUserID, !accountUserID.isEmpty else { return }
        guard await api.hasAuthenticatedSession() else {
            lastError = "Reconnect to Brunn to restore background location refresh."
            return
        }
        // Use the already authenticated owner session, including CSRF, for this
        // installation operation. Do not issue a broader device credential.
        await registerInstallation(
            using: api, bearerToken: nil, accountUserID: accountUserID,
            generation: generation
        )
    }

    func synchronizeInstallation(
        using api: BrunnAPI,
        canManageNotifications: Bool,
        bearerToken: String?
    ) async {
        let generation = registrationGeneration
        guard canManageNotifications, let bearerToken else { return }
        await refreshAuthorizationStatus()
        guard permissionState == .enabled || permissionState == .provisional else { return }

        await registerInstallation(
            using: api, bearerToken: bearerToken, accountUserID: nil,
            generation: generation
        )
    }

    private func registerInstallation(
        using api: BrunnAPI,
        bearerToken: String?,
        accountUserID: String?,
        generation: Int
    ) async {
        await acquireRegistrationOperation()
        defer { releaseRegistrationOperation() }
        guard generation == registrationGeneration, let deviceToken else { return }
        guard hasPendingDeviceToken || registrationState != .registered
                || (accountUserID != nil && registeredAccountUserID != accountUserID)
        else { return }

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
                    deviceToken: deviceToken.map { String(format: "%02x", $0) }.joined()
                ),
                bearerToken: bearerToken
            )
            hasPendingDeviceToken = self.deviceToken != deviceToken
            registrationState = .registered
            registeredAccountUserID = accountUserID
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
        return await revokeInstallation(using: api, bearerToken: bearerToken)
    }

    func revokeLocationRecovery(using api: BrunnAPI, accountUserID: String?) async -> Bool {
        guard let accountUserID, !accountUserID.isEmpty,
              await api.hasAuthenticatedSession()
        else {
            lastError = "Reconnect to Brunn before disconnecting this iPhone's background refresh."
            return false
        }
        guard registrationState != .revoking else { return false }
        return await revokeInstallation(using: api, bearerToken: nil)
    }

    private func revokeInstallation(using api: BrunnAPI, bearerToken: String?) async -> Bool {
        // Invalidate already queued uploads before waiting for an in-flight
        // registration, then revoke its server result after it completes.
        registrationGeneration += 1
        await acquireRegistrationOperation()
        defer { releaseRegistrationOperation() }
        let previousState = registrationState
        registrationState = .revoking
        do {
            _ = try await api.revokeNotificationInstallation(
                installationID: installationID,
                bearerToken: bearerToken
            )
            registrationState = .notRegistered
            registeredAccountUserID = nil
            lastRegisteredAt = nil
            lastError = nil
            return true
        } catch {
            registrationState = previousState
            lastError = "The notification installation could not be revoked. \(error.localizedDescription)"
            return false
        }
    }

    private func acquireRegistrationOperation() async {
        if registrationOperationHeld {
            await withCheckedContinuation { registrationOperationWaiters.append($0) }
        } else {
            registrationOperationHeld = true
        }
    }

    private func releaseRegistrationOperation() {
        if registrationOperationWaiters.isEmpty {
            registrationOperationHeld = false
        } else {
            registrationOperationWaiters.removeFirst().resume()
        }
    }

    func refreshAuthorizationStatus() async {
        permissionState = await loadPermissionState()
    }

    private static func systemPermissionState() async -> PushPermissionState {
        let settings = await UNUserNotificationCenter.current().notificationSettings()
        switch settings.authorizationStatus {
        case .authorized, .ephemeral:
            return .enabled
        case .provisional:
            return .provisional
        case .denied:
            return .denied
        case .notDetermined:
            return .unknown
        @unknown default:
            return .unknown
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

    static func isLocationHeartbeat(_ userInfo: [AnyHashable: Any]) -> Bool {
        userInfo["schema"] as? String == "brunn-push@v1"
            && userInfo["kind"] as? String == "location_heartbeat"
    }
}

extension Notification.Name {
    static let brunnPushRoute = Notification.Name("brunn.push-route")
    static let brunnReviewRefresh = Notification.Name("brunn.review-refresh")
    static let brunnPushToken = Notification.Name("brunn.push-token")
    static let brunnPushRegistrationFailed = Notification.Name("brunn.push-registration-failed")
    static let brunnMessagingPrefetch = Notification.Name("brunn.messaging-prefetch")
}

final class NotificationBackgroundFetch: @unchecked Sendable {
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
        route: AppRoute? = nil,
        completionHandler: @escaping @Sendable (UNNotificationPresentationOptions) -> Void
    ) {
        DispatchQueue.main.async {
            completionHandler(options)
            // A valid notification is only a signal to reload through the
            // current authenticated account. No push body becomes Review data.
            if case .notification = route {
                NotificationCenter.default.post(name: .brunnReviewRefresh, object: nil)
            }
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
        let request = NotificationBackgroundFetch(completionHandler: completionHandler)
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

@MainActor
final class AppDelegate: NSObject, UIApplicationDelegate, UNUserNotificationCenterDelegate {
    let locationReporter: LocationReporter

    override convenience init() {
        self.init(locationReporter: LocationReporter())
    }

    init(locationReporter: LocationReporter) {
        self.locationReporter = locationReporter
        super.init()
    }

    nonisolated static func handlesBackgroundURLSession(identifier: String) -> Bool {
        MessagingBackgroundTransport.handlesBackgroundSession(identifier: identifier)
    }

    func application(
        _: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil
    ) -> Bool {
        UNUserNotificationCenter.current().delegate = self
        locationReporter.applicationDidFinishLaunching(
            relaunchedForLocation: launchOptions?[.location] != nil
        )
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
        if NotificationRouteParser.isLocationHeartbeat(userInfo) {
            locationReporter.handleHeartbeat(completionHandler: completionHandler)
            return
        }
        NotificationDelegateHandoff.finishBackgroundFetch(
            shouldPrefetch: NotificationRouteParser.isMessagingPrefetch(userInfo),
            completionHandler: completionHandler
        )
    }

    nonisolated func userNotificationCenter(
        _: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping @Sendable (UNNotificationPresentationOptions) -> Void
    ) {
        NotificationDelegateHandoff.finishPresentation(
            [.banner, .list, .sound],
            route: NotificationRouteParser.route(from: notification.request.content.userInfo),
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
