import Foundation

struct LocationPresencePlace: Codable, Sendable, Equatable {
    let label: String?
    let kind: String
    let confidence: String
    let since: String
}

struct LocationPresencePosition: Codable, Sendable, Equatable {
    let lat: Double
    let lon: Double
    let accuracyM: Double
    let observedAt: String
    let ageSeconds: Int
    let approximate: Bool

    enum CodingKeys: String, CodingKey {
        case lat, lon, approximate
        case accuracyM = "accuracy_m"
        case observedAt = "observed_at"
        case ageSeconds = "age_seconds"
    }

    var observedDate: Date? {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return formatter.date(from: observedAt)
            ?? ISO8601DateFormatter().date(from: observedAt)
    }

    var mapURL: URL? {
        guard lat.isFinite, lon.isFinite, (-90...90).contains(lat),
              (-180...180).contains(lon) else { return nil }
        var components = URLComponents(string: "https://maps.apple.com/")
        components?.queryItems = [
            URLQueryItem(name: "ll", value: "\(lat),\(lon)"),
            URLQueryItem(name: "q", value: "Reported location"),
        ]
        return components?.url
    }
}

struct LocationPresence: Codable, Sendable, Equatable {
    let status: String
    let place: LocationPresencePlace?
    let atHome: Bool
    let city: String?
    let region: String?
    let country: String?
    let timezone: String
    let lastSeen: String
    let position: LocationPresencePosition?
    let lastContact: String?
    let visit: LocationPresencePlace?

    enum CodingKeys: String, CodingKey {
        case status
        case place
        case atHome = "at_home"
        case city
        case region
        case country
        case timezone
        case lastSeen = "last_seen"
        case position, visit
        case lastContact = "last_contact"
    }

    func isLastKnown(at now: Date = .now) -> Bool {
        guard let position, let observedAt = position.observedDate else { return true }
        return status == "stale" || now.timeIntervalSince(observedAt) > 15 * 60
    }

    func displayLabel(at now: Date = .now) -> String {
        let label = place?.label?.trimmingCharacters(in: .whitespacesAndNewlines)
        let locality = [city, region, country].compactMap { $0 }.filter { !$0.isEmpty }
            .joined(separator: ", ")
        let description = label.flatMap { $0.isEmpty ? nil : $0 }
            ?? (locality.isEmpty ? "Map position" : locality)
        if isLastKnown(at: now) {
            return "Last known: \(description)"
        }
        if position?.approximate == true {
            return "Approximate: \(description)"
        }
        return description
    }
}

struct LocationReportUploadResponse: Codable, Sendable, Equatable {
    let accepted: Int
    let ignored: [String: Int]
    let presence: LocationPresence?
}
