import UIKit
import UserNotifications

final class AppDelegate: NSObject, UIApplicationDelegate, UNUserNotificationCenterDelegate {
    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil
    ) -> Bool {
        UNUserNotificationCenter.current().delegate = self
        return true
    }

    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification
    ) async -> UNNotificationPresentationOptions {
        let userInfo = notification.request.content.userInfo
        guard
            let occurrenceID = userInfo["occurrenceID"] as? String,
            let cueValue = userInfo["cue"] as? String,
            let cue = FeedbackCue(rawValue: cueValue)
        else {
            return [.banner, .list]
        }

        let soundEnabled = userInfo["soundEnabled"] as? Bool ?? true
        let hapticsEnabled = userInfo["hapticsEnabled"] as? Bool ?? true
        let audibleInSilentMode = userInfo["audibleInSilentMode"] as? Bool ?? false

        await ForegroundFeedbackController.shared.playIfNeeded(
            occurrenceID: occurrenceID,
            cue: cue,
            soundEnabled: soundEnabled,
            hapticsEnabled: hapticsEnabled,
            audibleInSilentMode: audibleInSilentMode
        )

        return [.banner, .list]
    }
}
