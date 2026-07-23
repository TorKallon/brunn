import UIKit

@MainActor
protocol ScreenAwakeControlling: AnyObject {
    var isEnabled: Bool { get }
    func setEnabled(_ enabled: Bool)
}

@MainActor
final class ScreenAwakeController: ScreenAwakeControlling {
    private(set) var isEnabled = false

    func setEnabled(_ enabled: Bool) {
        guard enabled != isEnabled else { return }
        isEnabled = enabled
        UIApplication.shared.isIdleTimerDisabled = enabled
    }

    deinit {
        if isEnabled {
            UIApplication.shared.isIdleTimerDisabled = false
        }
    }
}

@MainActor
final class NoopScreenAwakeController: ScreenAwakeControlling {
    private(set) var isEnabled = false

    func setEnabled(_ enabled: Bool) {
        isEnabled = enabled
    }
}

