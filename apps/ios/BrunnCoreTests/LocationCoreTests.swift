import Foundation
import XCTest
@testable import BrunnCore

final class LocationCoreTests: XCTestCase {
    func testCredentialCapabilitiesAreOnlyCanonicalReadOnlyUnionLocationWrite() {
        let withoutMessaging = LocationCredentialCapabilities.canonicalReadOnly
            + [LocationCredentialCapabilities.locationWrite]
        XCTAssertTrue(LocationCredentialCapabilities.isExactAcceptedSet(withoutMessaging))
        XCTAssertTrue(LocationCredentialCapabilities.isExactAcceptedSet(
            withoutMessaging + [LocationCredentialCapabilities.conditionalMessageRead]
        ))
        XCTAssertFalse(LocationCredentialCapabilities.isExactAcceptedSet(
            withoutMessaging + ["task.write"]
        ))
        XCTAssertFalse(LocationCredentialCapabilities.isExactAcceptedSet(
            withoutMessaging + [LocationCredentialCapabilities.locationWrite]
        ))
        XCTAssertFalse(LocationCredentialCapabilities.isExactAcceptedSet(
            LocationCredentialCapabilities.canonicalReadOnly
        ))
    }

    func testReportEncodingMatchesStrictWireShape() throws {
        let report = LocationReport(
            type: .visitDeparture,
            at: "2026-09-01T14:10:22.000-07:00",
            lat: 46.9965,
            lon: -120.5478,
            accuracyM: 30,
            arrivedAt: "2026-09-01T13:00:00.000-07:00",
            departedAt: "2026-09-01T14:10:22.000-07:00",
            geocode: LocationGeocode(
                city: "Ellensburg",
                region: "WA",
                country: "US",
                name: "Example"
            ),
            poi: [LocationPOI(name: "Example", category: "cafe", distanceM: 18)]
        )
        let payload = LocationReportBatchRequest(
            timezone: "America/Los_Angeles",
            reports: [report]
        )
        let object = try XCTUnwrap(
            JSONSerialization.jsonObject(with: JSONEncoder().encode(payload)) as? [String: Any]
        )
        let reports = try XCTUnwrap(object["reports"] as? [[String: Any]])
        let wire = try XCTUnwrap(reports.first)
        XCTAssertEqual(Set(wire.keys), [
            "type", "at", "lat", "lon", "accuracy_m", "arrived_at",
            "departed_at", "geocode", "poi",
        ])
        XCTAssertEqual(wire["type"] as? String, "visit_departure")
        XCTAssertNil(wire["id"])
        XCTAssertEqual((wire["poi"] as? [[String: Any]])?.first?["distance_m"] as? Double, 18)
    }

    func testMapKitPOICategoryRawValuesNormalizeToHistoryKinds() {
        XCTAssertEqual(
            LocationPOICategory.normalizedKind(from: "MKPOICategoryRestaurant"),
            "restaurant"
        )
        XCTAssertEqual(
            LocationPOICategory.normalizedKind(from: "MKPOICategoryAmusementPark"),
            "amusement_park"
        )
        XCTAssertEqual(
            LocationPOICategory.normalizedKind(from: "MKPOICategoryEVCharger"),
            "ev_charger"
        )
        XCTAssertEqual(LocationPOICategory.normalizedKind(from: "restaurant"), "restaurant")
        XCTAssertNil(LocationPOICategory.normalizedKind(from: "MKPOICategory"))
    }

    func testDiskQueueIsAtomicFIFOBoundedAndReplacesEnrichment() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let fileURL = directory.appendingPathComponent("location-queue.json")
        let queue = LocationDiskQueue(fileURL: fileURL)
        for index in 0 ... LocationDiskQueue.maximumCount {
            _ = try queue.append(ping(index))
        }
        XCTAssertEqual(try queue.count(), LocationDiskQueue.maximumCount)
        XCTAssertTrue(try queue.batch().isEmpty)
        let first = try XCTUnwrap(queue.nextPending())
        XCTAssertEqual(first.report.lat, 1)
        let enriched = first.report.enriched(
            geocode: LocationGeocode(city: "City", region: nil, country: "US", name: nil),
            poi: []
        )
        try queue.replace(id: first.id, with: enriched)
        let batch = try queue.batch()
        XCTAssertEqual(batch.count, 1)
        XCTAssertEqual(batch.first?.report.geocode?.city, "City")

        try queue.remove(ids: batch.map(\.id))
        XCTAssertEqual(try queue.count(), LocationDiskQueue.maximumCount - 1)
        XCTAssertEqual(try queue.nextPending()?.report.lat, 2)
        try queue.clear()
        XCTAssertEqual(try queue.count(), 0)

        try FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: true
        )
        let ready = (0 ... LocationDiskQueue.maximumBatchCount).map {
            LocationQueuedReport(report: ping($0), isEnriched: true)
        }
        try JSONEncoder().encode(ready).write(to: fileURL, options: .atomic)
        XCTAssertEqual(try queue.batch().count, LocationDiskQueue.maximumBatchCount)
    }

    func testStatusStorePersistsReportingCoordinateAndUpload() throws {
        let suite = "LocationCoreTests.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let store = LocationStatusStore(defaults: defaults)
        let coordinate = LocationStoredCoordinate(latitude: 47.6, longitude: -122.3)
        let date = Date(timeIntervalSince1970: 1_800_000_000)
        store.reportingEnabled = true
        store.lastGeocodedCoordinate = coordinate
        store.lastUploadAt = date

        let reloaded = LocationStatusStore(defaults: defaults)
        XCTAssertTrue(reloaded.reportingEnabled)
        XCTAssertEqual(reloaded.lastGeocodedCoordinate, coordinate)
        XCTAssertEqual(reloaded.lastUploadAt, date)

        reloaded.clearLiveStatus()
        XCTAssertNil(reloaded.lastUploadAt)
    }

    private func ping(_ index: Int) -> LocationReport {
        LocationReport(
            type: .ping,
            at: "2026-09-01T14:10:22.000-07:00",
            lat: Double(index),
            lon: 0,
            accuracyM: 10
        )
    }
}
