import Foundation
import XCTest
@testable import BrunnCore

final class LocationPresenceTests: XCTestCase {
    func testCurrentPositionDoesNotDisplayHistoricalVisit() throws {
        let presence = try decode(position: #"{"lat":47.6,"lon":-122.3,"accuracy_m":18,"observed_at":"2026-09-06T12:00:00Z","age_seconds":60,"approximate":false}"#)
        let now = try XCTUnwrap(ISO8601DateFormatter().date(from: "2026-09-06T12:01:00Z"))
        XCTAssertEqual(presence.displayLabel(at: now), "Example Street")
        XCTAssertEqual(presence.visit?.label, "Previous stop")
        XCTAssertNotNil(presence.position?.mapURL)
    }

    func testCachedPositionBecomesLastKnownAsTimePasses() throws {
        let presence = try decode(position: #"{"lat":47.6,"lon":-122.3,"accuracy_m":18,"observed_at":"2026-09-06T12:00:00.000Z","age_seconds":0,"approximate":false}"#)
        let now = try XCTUnwrap(ISO8601DateFormatter().date(from: "2026-09-06T12:16:00Z"))
        XCTAssertEqual(presence.displayLabel(at: now), "Last known: Example Street")
    }

    func testApproximateAndLegacyPayloadsNeverLookPreciselyCurrent() throws {
        let approximate = try decode(position: #"{"lat":47.6,"lon":-122.3,"accuracy_m":800,"observed_at":"2026-09-06T12:00:00Z","age_seconds":0,"approximate":true}"#)
        let now = try XCTUnwrap(ISO8601DateFormatter().date(from: "2026-09-06T12:01:00Z"))
        XCTAssertEqual(approximate.displayLabel(at: now), "Approximate: Example Street")
        let legacy = try decode(position: nil)
        XCTAssertNil(legacy.position)
        XCTAssertEqual(legacy.displayLabel(at: now), "Last known: Example Street")
    }

    private func decode(position: String?) throws -> LocationPresence {
        let added = position.map { ",\"position\":\($0)" } ?? ""
        let json = #"{"status":"at_place","place":{"label":"Example Street","kind":"unknown","confidence":"medium","since":"2026-09-06T12:00:00Z"},"at_home":false,"city":"Example City","region":null,"country":null,"timezone":"UTC","last_seen":"2026-09-06T12:00:00Z","visit":{"label":"Previous stop","kind":"unknown","confidence":"low","since":"2026-09-06T09:00:00Z"}"# + added + "}"
        return try JSONDecoder().decode(LocationPresence.self, from: Data(json.utf8))
    }
}
