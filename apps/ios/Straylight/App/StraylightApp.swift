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
                    if let route = PushRouteBuffer.shared.take() {
                        await model.handle(route)
                    }
                    guard model.phase == .launching else { return }
                    await model.bootstrap()
                }
                .onOpenURL { url in
                    guard let route = AppRoute(url: url) else { return }
                    Task { await model.handle(route) }
                }
                .onReceive(NotificationCenter.default.publisher(for: .straylightPushRoute)) { _ in
                    guard let route = PushRouteBuffer.shared.take() else { return }
                    Task { await model.handle(route) }
                }
                .onChange(of: scenePhase) { _, phase in
                    guard phase == .active, let route = PushRouteBuffer.shared.take() else { return }
                    Task { await model.handle(route) }
                }
                .onReceive(NotificationCenter.default.publisher(for: .straylightPushToken)) { event in
                    guard let token = event.object as? Data else { return }
                    notifications.receiveDeviceToken(token)
                }
                .onReceive(NotificationCenter.default.publisher(for: .straylightPushRegistrationFailed)) { event in
                    guard let error = event.object as? Error else { return }
                    notifications.receiveRegistrationFailure(error)
                }
        }
    }
}
