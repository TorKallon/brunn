import Foundation

struct LocationStoredCoordinate: Codable, Sendable, Equatable {
    let latitude: Double
    let longitude: Double
}

final class LocationStatusStore {
    private enum Key {
        static let reportingEnabled = "brunn.location.reporting-enabled.v1"
        static let lastGeocodedCoordinate = "brunn.location.last-geocoded-coordinate.v1"
        static let lastUploadAt = "brunn.location.last-upload-at.v1"
        static let setupPending = "brunn.location.setup-pending.v1"
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

    func clearLiveStatus() {
        lastUploadAt = nil
    }

    func clearForDisconnect() {
        reportingEnabled = false
        lastGeocodedCoordinate = nil
        lastUploadAt = nil
        setupPending = false
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
