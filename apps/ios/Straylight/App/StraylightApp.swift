import SwiftUI

@main
struct StraylightApp: App {
    @Environment(\.scenePhase) private var scenePhase
    @UIApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @StateObject private var model = AppModel()
    @StateObject private var notifications = NotificationCoordinator()

    var body: some Scene {
        WindowGroup {
            RootView()
                .environmentObject(model)
                .environmentObject(notifications)
                .tint(StraylightTheme.signal)
                .task {
                    if let token = PushTokenBuffer.shared.take() {
                        notifications.receiveDeviceToken(token)
                    }
                    if let route = PushRouteBuffer.shared.take() {
                        await model.handle(route)
                    }
                    if model.phase == .launching {
                        await model.bootstrap()
                    }
                    await notifications.synchronizeInstallation(
                        using: model.api,
                        canManageNotifications: model.canManageNotifications,
                        bearerToken: model.deviceTaskBearer()
                    )
                }
                .onOpenURL { url in
                    guard let route = AppRoute(url: url) else { return }
                    Task { await model.handle(route) }
                }
                .onReceive(NotificationCenter.default.publisher(for: .straylightPushRoute)) { _ in
                    guard scenePhase == .active else { return }
                    guard let route = PushRouteBuffer.shared.take() else { return }
                    Task { await model.handle(route) }
                }
                .onReceive(NotificationCenter.default.publisher(
                    for: .straylightMessagingPrefetch
                )) { event in
                    guard let prefetch = event.object as? MessagingBackgroundPrefetch else {
                        return
                    }
                    Task {
                        switch await model.refreshMessaging(.notificationPush) {
                        case .newData:
                            prefetch.finish(.newData)
                        case .noData:
                            prefetch.finish(.noData)
                        case .failed:
                            prefetch.finish(.failed)
                        }
                    }
                }
                .onChange(of: scenePhase) { _, phase in
                    guard phase == .active else { return }
                    Task {
                        await notifications.refreshAuthorizationStatus()
                        if let token = PushTokenBuffer.shared.take() {
                            notifications.receiveDeviceToken(token)
                        }
                        if let route = PushRouteBuffer.shared.take() {
                            await model.handle(route)
                        }
                        await notifications.synchronizeInstallation(
                            using: model.api,
                            canManageNotifications: model.canManageNotifications,
                            bearerToken: model.deviceTaskBearer()
                        )
                        await model.refreshDashboardIfNeeded()
                        await model.refreshTaskSurface()
                        await model.refreshNotifications()
                        await model.refreshMessaging(.foreground)
                    }
                }
                .onReceive(NotificationCenter.default.publisher(for: .straylightPushToken)) { event in
                    guard let token = event.object as? Data else { return }
                    notifications.receiveDeviceToken(token)
                    _ = PushTokenBuffer.shared.take()
                    Task {
                        await notifications.synchronizeInstallation(
                            using: model.api,
                            canManageNotifications: model.canManageNotifications,
                            bearerToken: model.deviceTaskBearer()
                        )
                    }
                }
                .onReceive(NotificationCenter.default.publisher(for: .straylightPushRegistrationFailed)) { event in
                    guard let error = event.object as? Error else { return }
                    notifications.receiveRegistrationFailure(error)
                }
        }
    }
}
