import Foundation

struct LocationStoredCoordinate: Codable, Sendable, Equatable {
    let latitude: Double
    let longitude: Double
}

/// Persist only operational facts here, never coordinates, labels, or tokens.
struct LocationCaptureHealth: Codable, Sendable, Equatable {
    var captureState = "stopped"
    var lastCallbackAt: Date?
    var lastCallbackSource: String?
    var lastUsableFixAt: Date?
    var accuracyCategory: String?
    var lastRejection: String?
    var lastRefreshResult: String?
    var lastUploadFailureAt: Date?
}

enum LocationCapturePolicy {
    static let maximumSampleAge: TimeInterval = 15 * 60
    static let maximumRefreshAge: TimeInterval = 60
    static let maximumUsableAccuracy = 1_000.0
    static let preciseAccuracy = 200.0

    static func accuracyCategory(_ accuracy: Double) -> String {
        if accuracy <= preciseAccuracy { return "precise" }
        if accuracy <= maximumUsableAccuracy { return "approximate" }
        return "coarse"
    }

    static func shouldForwardLiveFix(
        elapsed: TimeInterval,
        distance: Double,
        accuracy: Double,
        previousAccuracy: Double,
        stationaryTransition: Bool
    ) -> Bool {
        guard elapsed >= 0 else { return false }
        // Preserve arrival/resumption and accuracy recovery without waiting for a timer.
        if stationaryTransition { return true }
        guard elapsed > 0 else { return false }
        if previousAccuracy > preciseAccuracy, accuracy <= preciseAccuracy { return true }
        if elapsed >= 60 { return true }
        return elapsed >= 15 && distance >= 100
    }
}

final class LocationStatusStore {
    private enum Key {
        static let reportingEnabled = "brunn.location.reporting-enabled.v1"
        static let lastGeocodedCoordinate = "brunn.location.last-geocoded-coordinate.v1"
        static let lastUploadAt = "brunn.location.last-upload-at.v1"
        static let setupPending = "brunn.location.setup-pending.v1"
        static let backgroundSessionActive = "brunn.location.background-session-active.v1"
        static let captureHealth = "brunn.location.capture-health.v1"
    }

    private let defaults: UserDefaults

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    var reportingEnabled: Bool {
        get { defaults.bool(forKey: Key.reportingEnabled) }
        set { defaults.set(newValue, forKey: Key.reportingEnabled) }
    }

    var lastGeocodedCoordinate: LocationStoredCoordinate? {
        get { decode(LocationStoredCoordinate.self, key: Key.lastGeocodedCoordinate) }
        set { encode(newValue, key: Key.lastGeocodedCoordinate) }
    }

    var lastUploadAt: Date? {
        get { defaults.object(forKey: Key.lastUploadAt) as? Date }
        set { defaults.set(newValue, forKey: Key.lastUploadAt) }
    }

    var setupPending: Bool {
        get { defaults.bool(forKey: Key.setupPending) }
        set { defaults.set(newValue, forKey: Key.setupPending) }
    }

    var backgroundSessionActive: Bool {
        get { defaults.bool(forKey: Key.backgroundSessionActive) }
        set { defaults.set(newValue, forKey: Key.backgroundSessionActive) }
    }

    var captureHealth: LocationCaptureHealth {
        get { decode(LocationCaptureHealth.self, key: Key.captureHealth) ?? LocationCaptureHealth() }
        set { encode(newValue, key: Key.captureHealth) }
    }

    func clearLiveStatus() {
        lastUploadAt = nil
    }

    func clearForDisconnect() {
        reportingEnabled = false
        lastGeocodedCoordinate = nil
        lastUploadAt = nil
        setupPending = false
        backgroundSessionActive = false
        captureHealth = LocationCaptureHealth()
    }

    private func decode<Value: Decodable>(_ type: Value.Type, key: String) -> Value? {
        guard let data = defaults.data(forKey: key) else { return nil }
        return try? JSONDecoder().decode(type, from: data)
    }

    private func encode<Value: Encodable>(_ value: Value?, key: String) {
        guard let value else {
            defaults.removeObject(forKey: key)
            return
        }
        defaults.set(try? JSONEncoder().encode(value), forKey: key)
    }
}
