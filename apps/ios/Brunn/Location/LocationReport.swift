import CoreLocation
import Foundation

enum LocationPermissionPromptDecision: Equatable {
    case present
    case markHandled
    case none
}

enum LocationPermissionPromptAction: Equatable {
    case beginEnable
    case openSettings
    case unavailable
}

enum LocationPermissionState: Equatable {
    case notDetermined
    case restricted
    case denied
    case whenInUse
    case always
    case unknown

    init(_ authorizationStatus: CLAuthorizationStatus) {
        switch authorizationStatus {
        case .notDetermined:
            self = .notDetermined
        case .restricted:
            self = .restricted
        case .denied:
            self = .denied
        case .authorizedWhenInUse:
            self = .whenInUse
        case .authorizedAlways:
            self = .always
        @unknown default:
            self = .unknown
        }
    }
}

enum LocationPermissionPromptPolicy {
    static let storageKey = "brunn.location.permission-prompt-revision"
    static let userStorageKey = "brunn.location.permission-prompt-user-id"
    static let currentRevision = 1

    static func decision(
        isReady: Bool,
        connectionValidated: Bool,
        isDemo: Bool,
        userID: String?,
        sceneIsActive: Bool,
        reportingEnabled: Bool,
        credentialBoundToUser: Bool,
        storedRevision: Int,
        storedUserID: String,
        permissionState: LocationPermissionState
    ) -> LocationPermissionPromptDecision {
        guard let userID, !userID.isEmpty,
              isReady,
              connectionValidated,
              !isDemo,
              sceneIsActive
        else { return .none }
        guard storedRevision < currentRevision || storedUserID != userID else {
            return .none
        }

        switch permissionState {
        case .notDetermined, .whenInUse, .denied, .restricted:
            return .present
        case .always:
            if reportingEnabled {
                return credentialBoundToUser ? .markHandled : .none
            }
            return .present
        case .unknown:
            return .none
        }
    }

    static func primaryAction(
        for permissionState: LocationPermissionState
    ) -> LocationPermissionPromptAction {
        switch permissionState {
        case .notDetermined, .whenInUse, .always:
            .beginEnable
        case .denied, .restricted:
            .openSettings
        case .unknown:
            .unavailable
        }
    }

    static func handledRevision(storedRevision: Int) -> Int {
        max(storedRevision, currentRevision)
    }

    static func reset(defaults: UserDefaults = .standard) {
        defaults.removeObject(forKey: storageKey)
        defaults.removeObject(forKey: userStorageKey)
    }
}

enum LocationReportType: String, Codable, Sendable {
    case ping
    case visitArrival = "visit_arrival"
    case visitDeparture = "visit_departure"
}

struct LocationGeocode: Codable, Sendable, Equatable {
    let city: String?
    let region: String?
    let country: String?
    let name: String?
}

struct LocationPOI: Codable, Sendable, Equatable {
    let name: String
    let category: String?
    let distanceM: Double

    enum CodingKeys: String, CodingKey {
        case name
        case category
        case distanceM = "distance_m"
    }
}

enum LocationPOICategory {
    private static let mapKitPrefix = "MKPOICategory"

    static func normalizedKind(from rawValue: String?) -> String? {
        guard var source = rawValue?.trimmingCharacters(in: .whitespacesAndNewlines),
              !source.isEmpty
        else { return nil }
        if source.hasPrefix(mapKitPrefix) {
            source.removeFirst(mapKitPrefix.count)
        }
        guard !source.isEmpty else { return nil }

        let scalars = Array(source.unicodeScalars)
        var result = ""
        for index in scalars.indices {
            let scalar = scalars[index]
            let isUppercase = asciiUppercase(scalar)
            let isLowercase = asciiLowercase(scalar)
            let isDigit = asciiDigit(scalar)
            guard isUppercase || isLowercase || isDigit else {
                appendSeparator(to: &result)
                continue
            }

            if isUppercase, index > scalars.startIndex {
                let previous = scalars[scalars.index(before: index)]
                let next = index < scalars.index(before: scalars.endIndex)
                    ? scalars[scalars.index(after: index)]
                    : nil
                let beginsWord = asciiLowercase(previous)
                    || asciiDigit(previous)
                    || (asciiUppercase(previous) && next.map(asciiLowercase) == true)
                if beginsWord {
                    appendSeparator(to: &result)
                }
            }

            if isUppercase {
                result.unicodeScalars.append(UnicodeScalar(scalar.value + 32)!)
            } else {
                result.unicodeScalars.append(scalar)
            }
        }
        while result.last == "_" {
            result.removeLast()
        }
        return result.isEmpty ? nil : result
    }

    private static func appendSeparator(to result: inout String) {
        guard !result.isEmpty, result.last != "_" else { return }
        result.append("_")
    }

    private static func asciiUppercase(_ scalar: UnicodeScalar) -> Bool {
        (65 ... 90).contains(scalar.value)
    }

    private static func asciiLowercase(_ scalar: UnicodeScalar) -> Bool {
        (97 ... 122).contains(scalar.value)
    }

    private static func asciiDigit(_ scalar: UnicodeScalar) -> Bool {
        (48 ... 57).contains(scalar.value)
    }
}

struct LocationReport: Codable, Sendable, Equatable {
    let type: LocationReportType
    let at: String
    let lat: Double
    let lon: Double
    let accuracyM: Double
    let arrivedAt: String?
    let departedAt: String?
    let geocode: LocationGeocode?
    let poi: [LocationPOI]

    init(
        type: LocationReportType,
        at: String,
        lat: Double,
        lon: Double,
        accuracyM: Double,
        arrivedAt: String? = nil,
        departedAt: String? = nil,
        geocode: LocationGeocode? = nil,
        poi: [LocationPOI] = []
    ) {
        self.type = type
        self.at = at
        self.lat = lat
        self.lon = lon
        self.accuracyM = accuracyM
        self.arrivedAt = arrivedAt
        self.departedAt = departedAt
        self.geocode = geocode
        self.poi = Array(poi.prefix(5))
    }

    func enriched(geocode: LocationGeocode?, poi: [LocationPOI]) -> LocationReport {
        LocationReport(
            type: type,
            at: at,
            lat: lat,
            lon: lon,
            accuracyM: accuracyM,
            arrivedAt: arrivedAt,
            departedAt: departedAt,
            geocode: geocode,
            poi: poi
        )
    }

    enum CodingKeys: String, CodingKey {
        case type
        case at
        case lat
        case lon
        case accuracyM = "accuracy_m"
        case arrivedAt = "arrived_at"
        case departedAt = "departed_at"
        case geocode
        case poi
    }
}

struct LocationReportBatchRequest: Codable, Sendable, Equatable {
    let timezone: String
    let reports: [LocationReport]

    init(timezone: String, reports: [LocationReport]) {
        self.timezone = timezone
        self.reports = Array(reports.prefix(200))
    }
}

enum LocationTimestamp {
    static func string(from date: Date, timeZone: TimeZone = .current) -> String {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.calendar = Calendar(identifier: .gregorian)
        formatter.timeZone = timeZone
        formatter.dateFormat = "yyyy-MM-dd'T'HH:mm:ss.SSSXXX"
        return formatter.string(from: date)
    }
}
