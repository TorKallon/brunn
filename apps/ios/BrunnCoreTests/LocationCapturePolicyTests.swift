import Foundation
import XCTest
@testable import BrunnCore

final class LocationCapturePolicyTests: XCTestCase {
    func testLiveCaptureKeepsArrivalAndResumeButBoundsOrdinarySamples() {
        func accepted(_ seconds: TimeInterval, _ distance: Double, transition: Bool = false) -> Bool {
            LocationCapturePolicy.shouldForwardLiveFix(
                elapsed: seconds, distance: distance, accuracy: 20,
                previousAccuracy: 20, stationaryTransition: transition
            )
        }
        XCTAssertFalse(accepted(1, 2))
        XCTAssertFalse(accepted(14, 500))
        XCTAssertTrue(accepted(15, 100))
        XCTAssertTrue(accepted(60, 0))
        XCTAssertTrue(accepted(0, 0, transition: true), "Keep the final stationary sample even if its timestamp repeats.")
        XCTAssertTrue(accepted(1, 1, transition: true), "Resume cannot wait behind the ordinary upload throttle.")
        XCTAssertFalse(accepted(-1, 500, transition: true))
        XCTAssertTrue(LocationCapturePolicy.shouldForwardLiveFix(
            elapsed: 1, distance: 0, accuracy: 20, previousAccuracy: 5_000, stationaryTransition: false
        ))
    }

    func testAccuracyCategoriesDoNotClaimCoarseContactIsUsable() {
        XCTAssertEqual(LocationCapturePolicy.accuracyCategory(20), "precise")
        XCTAssertEqual(LocationCapturePolicy.accuracyCategory(500), "approximate")
        XCTAssertEqual(LocationCapturePolicy.accuracyCategory(5_485), "coarse")
        XCTAssertEqual(LocationCapturePolicy.maximumUsableAccuracy, 1_000)
    }

    func testCaptureHealthAndSessionIntentSurviveRestartAndClearOnDisconnect() throws {
        let suite = "LocationCapturePolicyTests.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let store = LocationStatusStore(defaults: defaults)
        store.backgroundSessionActive = true
        var health = LocationCaptureHealth()
        health.captureState = "stationary"
        health.lastUsableFixAt = Date(timeIntervalSince1970: 1_700_000_000)
        health.lastCallbackSource = "live"
        store.captureHealth = health
        let reopened = LocationStatusStore(defaults: defaults)
        XCTAssertTrue(reopened.backgroundSessionActive)
        XCTAssertEqual(reopened.captureHealth, health)
        let encoded = try JSONEncoder().encode(health)
        let keys = try XCTUnwrap(JSONSerialization.jsonObject(with: encoded) as? [String: Any]).keys
        XCTAssertFalse(keys.contains("latitude"))
        XCTAssertFalse(keys.contains("longitude"))
        XCTAssertFalse(keys.contains("label"))
        reopened.clearForDisconnect()
        XCTAssertFalse(reopened.backgroundSessionActive)
        XCTAssertNil(reopened.captureHealth.lastUsableFixAt)
    }
}
