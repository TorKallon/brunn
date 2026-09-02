import Foundation

struct LocationPresencePlace: Codable, Sendable, Equatable {
    let label: String?
    let kind: String
    let confidence: String
    let since: String
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

    enum CodingKeys: String, CodingKey {
        case status
        case place
        case atHome = "at_home"
        case city
        case region
        case country
        case timezone
        case lastSeen = "last_seen"
    }
}

struct LocationReportUploadResponse: Codable, Sendable, Equatable {
    let accepted: Int
    let ignored: [String: Int]
    let presence: LocationPresence?
}
