@testable import Brunn
import CoreLocation
import Foundation
import MapKit
import XCTest

final class BrunnTests: XCTestCase {
    @MainActor
    func testLegacyKeychainMigrationReconstructsExactServiceIdentifier() {
        let retiredProductSegment = ["stray", "light"].joined()
        let expectedService = ["com", "rourkem", retiredProductSegment, "api"]
            .joined(separator: ".")

        XCTAssertEqual(
            KeychainCredentialStore.legacyServiceForMigration,
            expectedService
        )
    }

    func testMapKitRestaurantCategoryNormalizesToHistoryKind() {
        XCTAssertEqual(
            LocationPOICategory.normalizedKind(
                from: MKPointOfInterestCategory.restaurant.rawValue
            ),
            "restaurant"
        )
    }

    @MainActor
    func testLocationVisitHandlerPersistsRawReportBeforeAsyncWork() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        let suite = "BrunnTests.location-visit.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        let credentialStore = LocationKeychainCredentialStore(
            account: "ios-location-test-\(UUID().uuidString)"
        )
        defer {
            try? credentialStore.delete()
            defaults.removePersistentDomain(forName: suite)
            try? FileManager.default.removeItem(at: directory)
        }
        let queue = LocationDiskQueue(
            fileURL: directory.appendingPathComponent("location-queue.json")
        )
        let statusStore = LocationStatusStore(defaults: defaults)
        statusStore.reportingEnabled = true
        let reporter = LocationReporter(
            manager: CLLocationManager(),
            api: BrunnAPI(),
            credentialStore: credentialStore,
            queue: queue,
            statusStore: statusStore,
            enricher: LocationReportEnricher(statusStore: statusStore)
        )
        let arrival = Date(timeIntervalSince1970: 1_788_300_000)
        let departure = Date(timeIntervalSince1970: 1_788_303_600)

        reporter.handle(visit: TestLocationVisit(
            coordinate: CLLocationCoordinate2D(latitude: 47.6156, longitude: -122.2035),
            horizontalAccuracy: 30,
            arrivalDate: arrival,
            departureDate: departure
        ))

        let persisted = try XCTUnwrap(queue.nextPending())
        XCTAssertFalse(persisted.isEnriched)
        XCTAssertEqual(persisted.report.type, .visitDeparture)
        XCTAssertEqual(persisted.report.lat, 47.6156)
        XCTAssertEqual(persisted.report.lon, -122.2035)
        XCTAssertEqual(persisted.report.arrivedAt, LocationTimestamp.string(from: arrival))
        XCTAssertEqual(persisted.report.departedAt, LocationTimestamp.string(from: departure))
        XCTAssertNil(persisted.report.geocode)
        XCTAssertTrue(persisted.report.poi.isEmpty)
        withExtendedLifetime(reporter) {}
    }

    @MainActor
    func testSignificantLocationHandlerPersistsRawReportBeforeAsyncWork() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        let suite = "BrunnTests.location-ping.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        let credentialStore = LocationKeychainCredentialStore(
            account: "ios-location-test-\(UUID().uuidString)"
        )
        defer {
            try? credentialStore.delete()
            defaults.removePersistentDomain(forName: suite)
            try? FileManager.default.removeItem(at: directory)
        }
        let queue = LocationDiskQueue(
            fileURL: directory.appendingPathComponent("location-queue.json")
        )
        let statusStore = LocationStatusStore(defaults: defaults)
        statusStore.reportingEnabled = true
        let reporter = LocationReporter(
            manager: CLLocationManager(),
            api: BrunnAPI(),
            credentialStore: credentialStore,
            queue: queue,
            statusStore: statusStore,
            enricher: LocationReportEnricher(statusStore: statusStore)
        )
        let timestamp = Date(timeIntervalSince1970: 1_788_400_000)

        reporter.handle(location: CLLocation(
            coordinate: CLLocationCoordinate2D(latitude: 46.9965, longitude: -120.5478),
            altitude: 0,
            horizontalAccuracy: 65,
            verticalAccuracy: -1,
            timestamp: timestamp
        ))

        let persisted = try XCTUnwrap(queue.nextPending())
        XCTAssertFalse(persisted.isEnriched)
        XCTAssertEqual(persisted.report.type, .ping)
        XCTAssertEqual(persisted.report.at, LocationTimestamp.string(from: timestamp))
        XCTAssertEqual(persisted.report.lat, 46.9965)
        XCTAssertEqual(persisted.report.lon, -120.5478)
        XCTAssertNil(persisted.report.geocode)
        XCTAssertTrue(persisted.report.poi.isEmpty)
        withExtendedLifetime(reporter) {}
    }

    @MainActor
    func testForegroundTriggerEnrichesAndDrainsMoreThanTwoHundredReports() async throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let queueURL = directory.appendingPathComponent("location-queue.json")
        let suite = "BrunnTests.location-drain.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        let credentialStore = LocationKeychainCredentialStore(
            account: "ios-location-test-\(UUID().uuidString)"
        )
        defer {
            NotificationRequestURLProtocol.handler = nil
            try? credentialStore.delete()
            defaults.removePersistentDomain(forName: suite)
            try? FileManager.default.removeItem(at: directory)
        }

        let capabilities = LocationCredentialCapabilities.canonicalReadOnly
            + [LocationCredentialCapabilities.locationWrite]
        let credentialRef = "credential:33333333-3333-4333-8333-333333333333"
        try credentialStore.save(LocationDeviceCredential(
            credentialRef: credentialRef,
            token: "location-only-token",
            userID: "user:location-test",
            capabilities: capabilities
        ))

        let report = LocationReport(
            type: .ping,
            at: "2026-09-02T09:00:00.000-07:00",
            lat: 47.6156,
            lon: -122.2035,
            accuracyM: 25
        )
        let queued = (0 ..< 401).map { index in
            LocationQueuedReport(report: report, isEnriched: index != 199)
        }
        try JSONEncoder().encode(queued).write(to: queueURL, options: .atomic)
        let queue = LocationDiskQueue(fileURL: queueURL)
        XCTAssertEqual(try queue.batch().count, 199)

        let recorder = NotificationRequestRecorder()
        NotificationRequestURLProtocol.handler = { request in
            recorder.append(request)
            switch (request.httpMethod, request.url?.path) {
            case ("GET", "/api/v1/me"):
                return StubbedHTTPResponse(json: #"{"user":{"id":"user:location-test","display_name":"Owner"},"credential_id":"credential:33333333-3333-4333-8333-333333333333","capabilities":["open","query","read","compute","verify","status","task.read","location.write"],"read_only":false}"#)
            case ("POST", "/api/v1/location/reports"):
                let count = request.httpBody.flatMap { data in
                    (try? JSONSerialization.jsonObject(with: data) as? [String: Any])
                        .flatMap { $0["reports"] as? [[String: Any]] }?.count
                } ?? 0
                return StubbedHTTPResponse(
                    json: "{\"accepted\":\(count),\"ignored\":{},\"presence\":null}"
                )
            case ("GET", "/api/v1/location/presence"):
                return StubbedHTTPResponse(
                    statusCode: 404,
                    json: #"{"error":{"code":"location_presence_not_found","message":"none"}}"#
                )
            default:
                return StubbedHTTPResponse(
                    statusCode: 503,
                    json: #"{"error":{"code":"unexpected_request","message":"unexpected"}}"#
                )
            }
        }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [NotificationRequestURLProtocol.self]
        let api = BrunnAPI(
            configuration: .init(
                baseURL: try XCTUnwrap(URL(string: "https://location-drain.brunn.test/api/v1"))
            ),
            session: URLSession(configuration: configuration)
        )
        let statusStore = LocationStatusStore(defaults: defaults)
        statusStore.reportingEnabled = true
        statusStore.lastGeocodedCoordinate = LocationStoredCoordinate(
            latitude: report.lat,
            longitude: report.lon
        )
        let reporter = LocationReporter(
            manager: CLLocationManager(),
            api: api,
            credentialStore: credentialStore,
            queue: queue,
            statusStore: statusStore,
            enricher: LocationReportEnricher(statusStore: statusStore)
        )

        await reporter.applicationDidBecomeActive(expectedUserID: "user:location-test")

        let uploads = recorder.snapshot().filter {
            $0.httpMethod == "POST" && $0.url?.path == "/api/v1/location/reports"
        }
        let batchCounts = try uploads.map { request in
            let body = try XCTUnwrap(request.httpBody)
            let object = try XCTUnwrap(
                JSONSerialization.jsonObject(with: body) as? [String: Any]
            )
            return try XCTUnwrap(object["reports"] as? [[String: Any]]).count
        }
        XCTAssertEqual(batchCounts, [200, 200, 1])
        XCTAssertTrue(uploads.allSatisfy {
            $0.value(forHTTPHeaderField: "Authorization") == "Bearer location-only-token"
        })
        XCTAssertTrue(uploads.allSatisfy {
            $0.value(forHTTPHeaderField: "Cookie") == nil
        })
        XCTAssertEqual(try queue.count(), 0)
        XCTAssertEqual(reporter.queuedReportCount, 0)
    }

    @MainActor
    func testLocationDisconnectStopsReportingDeletesLiveDataAndRevokesCredential() async throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        let suite = "BrunnTests.location-disconnect.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        let credentialStore = LocationKeychainCredentialStore(
            account: "ios-location-test-\(UUID().uuidString)"
        )
        defer {
            NotificationRequestURLProtocol.handler = nil
            try? credentialStore.delete()
            defaults.removePersistentDomain(forName: suite)
            try? FileManager.default.removeItem(at: directory)
        }

        let credentialRef = "credential:55555555-5555-4555-8555-555555555555"
        let capabilities = LocationCredentialCapabilities.canonicalReadOnly
            + [LocationCredentialCapabilities.locationWrite]
        try credentialStore.save(LocationDeviceCredential(
            credentialRef: credentialRef,
            token: "location-disconnect-token",
            userID: "user:location-test",
            capabilities: capabilities
        ))
        let queue = LocationDiskQueue(
            fileURL: directory.appendingPathComponent("location-queue.json")
        )
        _ = try queue.append(LocationReport(
            type: .ping,
            at: "2026-09-02T10:00:00.000-07:00",
            lat: 47.6156,
            lon: -122.2035,
            accuracyM: 25
        ))
        let statusStore = LocationStatusStore(defaults: defaults)
        statusStore.reportingEnabled = true
        statusStore.setupPending = true
        statusStore.lastGeocodedCoordinate = LocationStoredCoordinate(
            latitude: 47.6156,
            longitude: -122.2035
        )
        statusStore.lastUploadAt = Date(timeIntervalSince1970: 1_788_400_000)

        let recorder = NotificationRequestRecorder()
        NotificationRequestURLProtocol.handler = { request in
            recorder.append(request)
            switch (request.httpMethod, request.url?.path) {
            case ("DELETE", "/api/v1/location/live"):
                return StubbedHTTPResponse(json: "{}")
            case ("DELETE", "/api/v1/credentials/\(credentialRef)"):
                return StubbedHTTPResponse(json: #"{"id":"credential:55555555-5555-4555-8555-555555555555","status":"revoked","revoked_at":"2026-09-02T17:00:00Z"}"#)
            default:
                return StubbedHTTPResponse(
                    statusCode: 503,
                    json: #"{"error":{"code":"unexpected_request","message":"unexpected"}}"#
                )
            }
        }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [NotificationRequestURLProtocol.self]
        let api = BrunnAPI(
            configuration: .init(
                baseURL: try XCTUnwrap(URL(string: "https://location-disconnect.brunn.test/api/v1"))
            ),
            session: URLSession(configuration: configuration)
        )
        let reporter = LocationReporter(
            manager: CLLocationManager(),
            api: api,
            credentialStore: credentialStore,
            queue: queue,
            statusStore: statusStore,
            enricher: LocationReportEnricher(statusStore: statusStore)
        )

        let disconnected = await reporter.disconnectFromAccount(ownerAPI: api)
        XCTAssertTrue(disconnected)

        let deletes = recorder.snapshot().filter { $0.httpMethod == "DELETE" }
        XCTAssertEqual(deletes.map { $0.url?.path }, [
            "/api/v1/location/live",
            "/api/v1/credentials/\(credentialRef)",
        ])
        XCTAssertEqual(
            deletes.first?.value(forHTTPHeaderField: "Authorization"),
            "Bearer location-disconnect-token"
        )
        XCTAssertNil(deletes.last?.value(forHTTPHeaderField: "Authorization"))
        XCTAssertFalse(reporter.reportingEnabled)
        XCTAssertFalse(reporter.hasCredential)
        XCTAssertFalse(reporter.setupPending)
        XCTAssertEqual(try queue.count(), 0)
        XCTAssertNil(try credentialStore.load())
        XCTAssertFalse(statusStore.reportingEnabled)
        XCTAssertNil(statusStore.lastGeocodedCoordinate)
        XCTAssertNil(statusStore.lastUploadAt)
    }

    @MainActor
    func testUnvalidatedOwnerSessionPreservesIndependentLocationQueue() async throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        let suite = "BrunnTests.location-owner-offline.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        let credentialStore = LocationKeychainCredentialStore(
            account: "ios-location-test-\(UUID().uuidString)"
        )
        defer {
            NotificationRequestURLProtocol.handler = nil
            try? credentialStore.delete()
            defaults.removePersistentDomain(forName: suite)
            try? FileManager.default.removeItem(at: directory)
        }
        try credentialStore.save(LocationDeviceCredential(
            credentialRef: "credential:99999999-9999-4999-8999-999999999999",
            token: "offline-location-token",
            userID: "user:location-test",
            capabilities: LocationCredentialCapabilities.canonicalReadOnly
                + [LocationCredentialCapabilities.locationWrite]
        ))
        let queue = LocationDiskQueue(
            fileURL: directory.appendingPathComponent("location-queue.json")
        )
        _ = try queue.append(LocationReport(
            type: .ping,
            at: "2026-09-02T10:00:00.000-07:00",
            lat: 47.6156,
            lon: -122.2035,
            accuracyM: 25
        ))
        let statusStore = LocationStatusStore(defaults: defaults)
        statusStore.reportingEnabled = true
        NotificationRequestURLProtocol.handler = { _ in
            StubbedHTTPResponse(
                statusCode: 503,
                json: #"{"error":{"code":"temporarily_unavailable","message":"offline"}}"#
            )
        }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [NotificationRequestURLProtocol.self]
        let api = BrunnAPI(
            configuration: .init(
                baseURL: try XCTUnwrap(URL(string: "https://location-offline.brunn.test/api/v1"))
            ),
            session: URLSession(configuration: configuration)
        )
        let reporter = LocationReporter(
            manager: CLLocationManager(),
            api: api,
            credentialStore: credentialStore,
            queue: queue,
            statusStore: statusStore,
            enricher: LocationReportEnricher(statusStore: statusStore)
        )

        await reporter.applicationDidBecomeActive(expectedUserID: nil)

        XCTAssertTrue(reporter.reportingEnabled)
        XCTAssertEqual(try queue.count(), 1)
        XCTAssertTrue(statusStore.reportingEnabled)
        XCTAssertNotNil(try credentialStore.load())
    }

    @MainActor
    func testLocationDisconnectRetryRemovesAnAlreadyRevokedCredential() async throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        let suite = "BrunnTests.location-disconnect-retry.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        let credentialStore = LocationKeychainCredentialStore(
            account: "ios-location-test-\(UUID().uuidString)"
        )
        defer {
            NotificationRequestURLProtocol.handler = nil
            try? credentialStore.delete()
            defaults.removePersistentDomain(forName: suite)
            try? FileManager.default.removeItem(at: directory)
        }
        try credentialStore.save(LocationDeviceCredential(
            credentialRef: "credential:77777777-7777-4777-8777-777777777777",
            token: "already-revoked-location-token",
            userID: "user:location-test",
            capabilities: LocationCredentialCapabilities.canonicalReadOnly
                + [LocationCredentialCapabilities.locationWrite]
        ))

        let recorder = NotificationRequestRecorder()
        NotificationRequestURLProtocol.handler = { request in
            recorder.append(request)
            return StubbedHTTPResponse(
                statusCode: 401,
                json: #"{"error":{"code":"credential_revoked","message":"revoked"}}"#
            )
        }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [NotificationRequestURLProtocol.self]
        let api = BrunnAPI(
            configuration: .init(
                baseURL: try XCTUnwrap(URL(string: "https://location-retry.brunn.test/api/v1"))
            ),
            session: URLSession(configuration: configuration)
        )
        let statusStore = LocationStatusStore(defaults: defaults)
        statusStore.reportingEnabled = true
        let reporter = LocationReporter(
            manager: CLLocationManager(),
            api: api,
            credentialStore: credentialStore,
            queue: LocationDiskQueue(
                fileURL: directory.appendingPathComponent("location-queue.json")
            ),
            statusStore: statusStore,
            enricher: LocationReportEnricher(statusStore: statusStore)
        )

        let disconnected = await reporter.disconnectFromAccount(ownerAPI: api)
        XCTAssertTrue(disconnected)
        let requests = recorder.snapshot()
        XCTAssertEqual(requests.map(\.httpMethod), ["DELETE"])
        XCTAssertEqual(requests.map { $0.url?.path }, ["/api/v1/location/live"])
        XCTAssertNil(try credentialStore.load())
        XCTAssertFalse(reporter.hasCredential)
        XCTAssertFalse(reporter.reportingEnabled)
    }

    @MainActor
    func testAccountSwitchClearsOldQueuedLocationsBeforeNewAccountCanReport() async throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        let suite = "BrunnTests.location-account-switch.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        let credentialStore = LocationKeychainCredentialStore(
            account: "ios-location-test-\(UUID().uuidString)"
        )
        defer {
            NotificationRequestURLProtocol.handler = nil
            try? credentialStore.delete()
            defaults.removePersistentDomain(forName: suite)
            try? FileManager.default.removeItem(at: directory)
        }
        try credentialStore.save(LocationDeviceCredential(
            credentialRef: "credential:88888888-8888-4888-8888-888888888888",
            token: "old-account-location-token",
            userID: "user:old-account",
            capabilities: LocationCredentialCapabilities.canonicalReadOnly
                + [LocationCredentialCapabilities.locationWrite]
        ))
        let queue = LocationDiskQueue(
            fileURL: directory.appendingPathComponent("location-queue.json")
        )
        _ = try queue.append(LocationReport(
            type: .ping,
            at: "2026-09-02T10:00:00.000-07:00",
            lat: 47.6156,
            lon: -122.2035,
            accuracyM: 25
        ))
        let statusStore = LocationStatusStore(defaults: defaults)
        statusStore.reportingEnabled = true

        let recorder = NotificationRequestRecorder()
        NotificationRequestURLProtocol.handler = { request in
            recorder.append(request)
            guard request.httpMethod == "DELETE",
                  request.url?.path == "/api/v1/location/live"
            else {
                return StubbedHTTPResponse(
                    statusCode: 503,
                    json: #"{"error":{"code":"unexpected_request","message":"unexpected"}}"#
                )
            }
            return StubbedHTTPResponse(json: "{}")
        }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [NotificationRequestURLProtocol.self]
        let api = BrunnAPI(
            configuration: .init(
                baseURL: try XCTUnwrap(URL(string: "https://location-switch.brunn.test/api/v1"))
            ),
            session: URLSession(configuration: configuration)
        )
        let reporter = LocationReporter(
            manager: CLLocationManager(),
            api: api,
            credentialStore: credentialStore,
            queue: queue,
            statusStore: statusStore,
            enricher: LocationReportEnricher(statusStore: statusStore)
        )

        await reporter.applicationDidBecomeActive(expectedUserID: "user:new-account")

        XCTAssertEqual(try queue.count(), 0)
        XCTAssertNil(try credentialStore.load())
        XCTAssertFalse(reporter.reportingEnabled)
        XCTAssertNil(reporter.validatedCredentialUserID)
        XCTAssertTrue(recorder.snapshot().allSatisfy {
            $0.url?.path != "/api/v1/location/reports"
        })
    }

    @MainActor
    func testAccountReconciliationSerializesWithNewCredentialSetup() async throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        let suite = "BrunnTests.location-account-serialization.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        let credentialStore = LocationKeychainCredentialStore(
            account: "ios-location-test-\(UUID().uuidString)"
        )
        defer {
            NotificationRequestURLProtocol.handler = nil
            try? credentialStore.delete()
            defaults.removePersistentDomain(forName: suite)
            try? FileManager.default.removeItem(at: directory)
        }

        let oldCredential = LocationDeviceCredential(
            credentialRef: "credential:cccccccc-cccc-4ccc-8ccc-cccccccccccc",
            token: "old-account-serialization-token",
            userID: "user:old-account",
            capabilities: LocationCredentialCapabilities.canonicalReadOnly
                + [LocationCredentialCapabilities.locationWrite]
        )
        try credentialStore.save(oldCredential)
        let statusStore = LocationStatusStore(defaults: defaults)
        statusStore.reportingEnabled = true

        let recorder = NotificationRequestRecorder()
        let deleteGate = SynchronousRequestGate()
        NotificationRequestURLProtocol.handler = { request in
            recorder.append(request)
            switch (request.httpMethod, request.url?.path) {
            case ("DELETE", "/api/v1/location/live"):
                deleteGate.enterAndWait()
                return StubbedHTTPResponse(json: "{}")
            case ("POST", "/api/v1/credentials"):
                return StubbedHTTPResponse(json: #"{"id":"credential:dddddddd-dddd-4ddd-8ddd-dddddddddddd","access":"ios_location","capabilities":["open","query","read","compute","verify","status","task.read","location.write"],"token":"new-account-serialization-token"}"#)
            case ("GET", "/api/v1/me"):
                return StubbedHTTPResponse(json: #"{"user":{"id":"user:new-account","display_name":"New owner"},"credential_id":"credential:dddddddd-dddd-4ddd-8ddd-dddddddddddd","capabilities":["open","query","read","compute","verify","status","task.read","location.write"],"read_only":true}"#)
            default:
                return StubbedHTTPResponse(
                    statusCode: 503,
                    json: #"{"error":{"code":"unexpected_request","message":"unexpected"}}"#
                )
            }
        }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [NotificationRequestURLProtocol.self]
        let api = BrunnAPI(
            configuration: .init(
                baseURL: try XCTUnwrap(URL(string: "https://location-serialization.brunn.test/api/v1"))
            ),
            session: URLSession(configuration: configuration)
        )
        let reporter = LocationReporter(
            manager: CLLocationManager(),
            api: api,
            credentialStore: credentialStore,
            queue: LocationDiskQueue(
                fileURL: directory.appendingPathComponent("location-queue.json")
            ),
            statusStore: statusStore,
            enricher: LocationReportEnricher(statusStore: statusStore)
        )

        let reconciliation = Task {
            await reporter.applicationDidBecomeActive(expectedUserID: "user:new-account")
        }
        await Task.yield()
        XCTAssertTrue(deleteGate.waitUntilEntered())
        let setup = Task {
            await reporter.prepareEnableFromSettings(
                ownerAPI: api,
                expectedUserID: "user:new-account"
            )
        }
        try await Task.sleep(for: .milliseconds(50))
        XCTAssertEqual(recorder.snapshot().count, 1)

        deleteGate.release()
        await reconciliation.value
        let setupSucceeded = await setup.value
        XCTAssertTrue(setupSucceeded)

        let requests = recorder.snapshot()
        XCTAssertEqual(requests.map(\.httpMethod), ["DELETE", "POST", "GET"])
        XCTAssertEqual(requests.map { $0.url?.path }, [
            "/api/v1/location/live",
            "/api/v1/credentials",
            "/api/v1/me",
        ])
        XCTAssertEqual(try credentialStore.load()?.userID, "user:new-account")
        XCTAssertEqual(try credentialStore.load()?.token, "new-account-serialization-token")
        XCTAssertEqual(reporter.validatedCredentialUserID, "user:new-account")
        XCTAssertTrue(reporter.setupPending)
    }

    @MainActor
    func testRevokedOldCredentialCannotCarryQueuedLocationsIntoReplacementAccess() async throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        let suite = "BrunnTests.location-revoked-switch.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        let credentialStore = LocationKeychainCredentialStore(
            account: "ios-location-test-\(UUID().uuidString)"
        )
        defer {
            NotificationRequestURLProtocol.handler = nil
            try? credentialStore.delete()
            defaults.removePersistentDomain(forName: suite)
            try? FileManager.default.removeItem(at: directory)
        }
        try credentialStore.save(LocationDeviceCredential(
            credentialRef: "credential:aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
            token: "revoked-old-account-token",
            userID: "user:old-account",
            capabilities: LocationCredentialCapabilities.canonicalReadOnly
                + [LocationCredentialCapabilities.locationWrite]
        ))
        let queue = LocationDiskQueue(
            fileURL: directory.appendingPathComponent("location-queue.json")
        )
        _ = try queue.append(LocationReport(
            type: .ping,
            at: "2026-09-02T10:00:00.000-07:00",
            lat: 47.6156,
            lon: -122.2035,
            accuracyM: 25
        ))
        let statusStore = LocationStatusStore(defaults: defaults)
        statusStore.reportingEnabled = true

        NotificationRequestURLProtocol.handler = { request in
            XCTAssertEqual(request.httpMethod, "GET")
            XCTAssertEqual(request.url?.path, "/api/v1/me")
            XCTAssertEqual(
                request.value(forHTTPHeaderField: "Authorization"),
                "Bearer revoked-old-account-token"
            )
            return StubbedHTTPResponse(
                statusCode: 401,
                json: #"{"error":{"code":"credential_revoked","message":"revoked"}}"#
            )
        }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [NotificationRequestURLProtocol.self]
        let api = BrunnAPI(
            configuration: .init(
                baseURL: try XCTUnwrap(URL(string: "https://location-revoked.brunn.test/api/v1"))
            ),
            session: URLSession(configuration: configuration)
        )
        let reporter = LocationReporter(
            manager: CLLocationManager(),
            api: api,
            credentialStore: credentialStore,
            queue: queue,
            statusStore: statusStore,
            enricher: LocationReportEnricher(statusStore: statusStore)
        )

        await reporter.applicationDidBecomeActive(expectedUserID: "user:old-account")
        XCTAssertEqual(try queue.count(), 0)
        XCTAssertNil(try credentialStore.load())
        XCTAssertFalse(reporter.reportingEnabled)

        let recorder = NotificationRequestRecorder()
        NotificationRequestURLProtocol.handler = { request in
            recorder.append(request)
            switch (request.httpMethod, request.url?.path) {
            case ("POST", "/api/v1/credentials"):
                return StubbedHTTPResponse(json: #"{"id":"credential:bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb","access":"ios_location","capabilities":["open","query","read","compute","verify","status","task.read","location.write"],"token":"new-account-location-token"}"#)
            case ("GET", "/api/v1/me"):
                return StubbedHTTPResponse(json: #"{"user":{"id":"user:new-account","display_name":"New owner"},"credential_id":"credential:bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb","capabilities":["open","query","read","compute","verify","status","task.read","location.write"],"read_only":true}"#)
            default:
                return StubbedHTTPResponse(
                    statusCode: 503,
                    json: #"{"error":{"code":"unexpected_request","message":"unexpected"}}"#
                )
            }
        }

        let prepared = await reporter.prepareEnableFromSettings(
            ownerAPI: api,
            expectedUserID: "user:new-account"
        )

        XCTAssertTrue(prepared)
        XCTAssertEqual(try queue.count(), 0)
        XCTAssertEqual(try credentialStore.load()?.userID, "user:new-account")
        XCTAssertEqual(reporter.validatedCredentialUserID, "user:new-account")
        XCTAssertTrue(recorder.snapshot().allSatisfy {
            $0.url?.path != "/api/v1/location/reports"
        })
    }

    @MainActor
    func testLocationPermissionPrimerStaysOpenWhenCredentialPreparationFails() async throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        let suite = "BrunnTests.location-enable-failure.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        let credentialStore = LocationKeychainCredentialStore(
            account: "ios-location-test-\(UUID().uuidString)"
        )
        defer {
            NotificationRequestURLProtocol.handler = nil
            try? credentialStore.delete()
            defaults.removePersistentDomain(forName: suite)
            try? FileManager.default.removeItem(at: directory)
        }

        NotificationRequestURLProtocol.handler = { request in
            XCTAssertEqual(request.httpMethod, "POST")
            XCTAssertEqual(request.url?.path, "/api/v1/credentials")
            return StubbedHTTPResponse(
                statusCode: 503,
                json: #"{"error":{"code":"temporarily_unavailable","message":"retry"}}"#
            )
        }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [NotificationRequestURLProtocol.self]
        let api = BrunnAPI(
            configuration: .init(
                baseURL: try XCTUnwrap(URL(string: "https://location-enable.brunn.test/api/v1"))
            ),
            session: URLSession(configuration: configuration)
        )
        let statusStore = LocationStatusStore(defaults: defaults)
        let reporter = LocationReporter(
            manager: CLLocationManager(),
            api: api,
            credentialStore: credentialStore,
            queue: LocationDiskQueue(
                fileURL: directory.appendingPathComponent("location-queue.json")
            ),
            statusStore: statusStore,
            enricher: LocationReportEnricher(statusStore: statusStore)
        )
        var readyCallbackCount = 0

        let started = await reporter.beginEnable(
            ownerAPI: api,
            expectedUserID: "user:location-test"
        ) {
            readyCallbackCount += 1
        }

        XCTAssertFalse(started)
        XCTAssertEqual(readyCallbackCount, 0)
        XCTAssertFalse(reporter.setupPending)
        XCTAssertFalse(statusStore.setupPending)
        XCTAssertNotNil(reporter.lastError)
        XCTAssertNil(try credentialStore.load())
    }

    @MainActor
    func testDeniedPermissionSetupPersistsAcrossSettingsRoundTrip() async throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        let suite = "BrunnTests.location-settings-resume.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        let credentialStore = LocationKeychainCredentialStore(
            account: "ios-location-test-\(UUID().uuidString)"
        )
        defer {
            NotificationRequestURLProtocol.handler = nil
            try? credentialStore.delete()
            defaults.removePersistentDomain(forName: suite)
            try? FileManager.default.removeItem(at: directory)
        }

        let credentialRef = "credential:66666666-6666-4666-8666-666666666666"
        let capabilities = LocationCredentialCapabilities.canonicalReadOnly
            + [LocationCredentialCapabilities.locationWrite]
        try credentialStore.save(LocationDeviceCredential(
            credentialRef: credentialRef,
            token: "location-settings-token",
            userID: "user:location-test",
            capabilities: capabilities
        ))
        NotificationRequestURLProtocol.handler = { request in
            XCTAssertEqual(request.httpMethod, "GET")
            XCTAssertEqual(request.url?.path, "/api/v1/me")
            XCTAssertEqual(
                request.value(forHTTPHeaderField: "Authorization"),
                "Bearer location-settings-token"
            )
            XCTAssertNil(request.value(forHTTPHeaderField: "Cookie"))
            return StubbedHTTPResponse(json: #"{"user":{"id":"user:location-test","display_name":"Owner"},"credential_id":"credential:66666666-6666-4666-8666-666666666666","capabilities":["open","query","read","compute","verify","status","task.read","location.write"],"read_only":true}"#)
        }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [NotificationRequestURLProtocol.self]
        let api = BrunnAPI(
            configuration: .init(
                baseURL: try XCTUnwrap(URL(string: "https://location-settings.brunn.test/api/v1"))
            ),
            session: URLSession(configuration: configuration)
        )
        let statusStore = LocationStatusStore(defaults: defaults)
        let reporter = LocationReporter(
            manager: CLLocationManager(),
            api: api,
            credentialStore: credentialStore,
            queue: LocationDiskQueue(
                fileURL: directory.appendingPathComponent("location-queue.json")
            ),
            statusStore: statusStore,
            enricher: LocationReportEnricher(statusStore: statusStore)
        )

        let prepared = await reporter.prepareEnableFromSettings(
            ownerAPI: api,
            expectedUserID: "user:location-test"
        )

        XCTAssertTrue(prepared)
        XCTAssertTrue(reporter.setupPending)
        XCTAssertTrue(statusStore.setupPending)
        XCTAssertTrue(LocationStatusStore(defaults: defaults).setupPending)
        XCTAssertNil(reporter.lastError)
    }

    @MainActor
    func testFirstRunShowsConnectionWithoutCallingTheAPI() async {
        let credentialStore = TestCredentialStore(token: nil)
        let calls = CallCounter()
        let model = AppModel(
            credentialStore: credentialStore,
            bootstrapIdentityLoader: { _ in
                await calls.increment()
                return Self.readOnlyIdentity
            }
        )

        await model.bootstrap()
        let callCount = await calls.value

        XCTAssertEqual(model.phase, .connectionRequired)
        XCTAssertNil(model.locationReportingUserID)
        XCTAssertEqual(callCount, 0)
    }

    @MainActor
    func testStoredSessionValidationStopsWaitingAtStartupBudget() async {
        let credentialStore = TestCredentialStore(token: "sl_stale")
        let model = AppModel(
            credentialStore: credentialStore,
            bootstrapValidationTimeout: .milliseconds(40),
            storedSessionChecker: { _ in true },
            bootstrapIdentityLoader: { _ in
                try await Task.sleep(for: .seconds(30))
                return Self.readOnlyIdentity
            }
        )

        await model.bootstrap()

        XCTAssertEqual(model.phase, .connectionRequired)
        XCTAssertFalse(credentialStore.wasDeleted)
        XCTAssertTrue(model.connectionMessage?.contains("taking too long") == true)
    }

    @MainActor
    func testDemoBootstrapsWithoutCredentialOrNetwork() {
        let model = AppModel()
        model.enterDemo()

        XCTAssertEqual(model.phase, .ready)
        XCTAssertTrue(model.isDemo)
        XCTAssertNil(model.locationReportingUserID)
        XCTAssertEqual(model.selectedTab, .dashboard)
        XCTAssertEqual(model.currentCredentialID, "credential:demo-iphone")
        XCTAssertEqual(model.dashboard?.storage.text.count, 4_926)
        XCTAssertEqual(model.dashboard?.access.count, 4)
        XCTAssertEqual(model.latestBriefing?.briefing?.schema, "briefing.v1")
        XCTAssertFalse(model.tasks.isEmpty)
        XCTAssertFalse(model.alerts.isEmpty)
    }

    func testProtectedTaskSurfaceCacheRoundTripsOnlyBoundedSurfaceData() async throws {
        let cacheURL = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
            .appendingPathComponent("today-tasks.json")
        defer {
            try? FileManager.default.removeItem(at: cacheURL.deletingLastPathComponent())
        }
        let cache = TaskSurfaceCache(fileURL: cacheURL)
        let value = CachedTaskSurface(
            userID: "user:one",
            savedAt: Date(timeIntervalSince1970: 1_788_000_000),
            urgent: SampleData.agentUrgentTasks,
            next: Array(SampleData.agentNextTasks.prefix(7)),
            doneToday: SampleData.agentDoneToday,
            projects: SampleData.agentTaskProjects,
            contexts: SampleData.agentTaskContexts,
            selectedContexts: ["online", "phone"],
            nextRemaining: 7,
            backlogTotal: 18
        )

        let sessionFingerprint = "sha256:" + String(repeating: "a", count: 64)
        try await cache.save(value, sessionFingerprint: sessionFingerprint)
        let restored = try await cache.load()
        XCTAssertEqual(restored, value)
        let boundUserID = try await cache.boundUserID(matching: sessionFingerprint)
        XCTAssertEqual(boundUserID, "user:one")
        try await cache.clear()
        let cleared = try await cache.load()
        XCTAssertNil(cleared)
        let clearedBinding = try await cache.boundUserID(matching: sessionFingerprint)
        XCTAssertNil(clearedBinding)
    }

    @MainActor
    func testColdBootstrapInstantPaintsSameAccountTaskCacheBeforeNetworkAndRetainsItOnTimeout() async throws {
        let cacheURL = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
            .appendingPathComponent("today-tasks.json")
        defer {
            try? FileManager.default.removeItem(at: cacheURL.deletingLastPathComponent())
        }
        let host = "cold-cache-\(UUID().uuidString.lowercased()).brunn.test"
        let baseURL = try XCTUnwrap(URL(string: "https://\(host)/api/v1"))
        let cookieStorage = HTTPCookieStorage.shared
        let sessionCookie = try XCTUnwrap(HTTPCookie(properties: [
            .domain: host,
            .path: "/",
            .name: "brunn_session",
            .value: "same-account-session",
            .secure: "TRUE",
        ]))
        cookieStorage.setCookie(sessionCookie)
        defer { cookieStorage.deleteCookie(sessionCookie) }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.httpCookieStorage = cookieStorage
        let api = BrunnAPI(
            configuration: .init(baseURL: baseURL),
            session: URLSession(configuration: configuration),
            cookieStorage: cookieStorage
        )
        let currentSessionFingerprint = await api.authenticatedSessionFingerprint()
        let sessionFingerprint = try XCTUnwrap(currentSessionFingerprint)
        let cache = TaskSurfaceCache(fileURL: cacheURL)
        try await cache.save(CachedTaskSurface(
            userID: "user:one",
            savedAt: .now,
            urgent: SampleData.agentUrgentTasks,
            next: SampleData.agentNextTasks,
            doneToday: SampleData.agentDoneToday,
            projects: SampleData.agentTaskProjects,
            contexts: SampleData.agentTaskContexts,
            selectedContexts: ["online", "phone"],
            nextRemaining: 7,
            backlogTotal: 18
        ), sessionFingerprint: sessionFingerprint)
        let model = AppModel(
            api: api,
            credentialStore: TestCredentialStore(token: nil),
            taskSurfaceCache: cache,
            bootstrapValidationTimeout: .milliseconds(80),
            storedSessionChecker: { _ in true },
            bootstrapIdentityLoader: { _ in
                try await Task.sleep(for: .seconds(30))
                return Self.readOnlyIdentity
            },
            dashboardLoader: { _, _ in SampleData.dashboard },
            notificationListLoader: { _, _ in
                NotificationListResponse(items: [], nextCursor: nil, unreadCount: 0)
            }
        )

        let bootstrap = Task { await model.bootstrap() }
        for _ in 0 ..< 50 where model.urgentTasks.isEmpty || model.nextTasks.isEmpty {
            try await Task.sleep(for: .milliseconds(2))
        }

        XCTAssertEqual(model.phase, .ready)
        XCTAssertFalse(model.connectionValidated)
        XCTAssertEqual(model.user?.id, "user:one")
        XCTAssertEqual(model.urgentTasks, SampleData.agentUrgentTasks)
        XCTAssertEqual(model.nextTasks, SampleData.agentNextTasks)

        await bootstrap.value

        XCTAssertEqual(model.phase, .ready)
        XCTAssertFalse(model.connectionValidated)
        XCTAssertEqual(model.user?.id, "user:one")
        XCTAssertEqual(model.urgentTasks, SampleData.agentUrgentTasks)
        XCTAssertEqual(model.nextTasks, SampleData.agentNextTasks)
        XCTAssertTrue(model.connectionMessage?.contains("last protected Today view") == true)
    }

    @MainActor
    func testColdBootstrapNeverPaintsPriorAccountCacheAfterSessionCookieChanges() async throws {
        let cacheURL = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
            .appendingPathComponent("today-tasks.json")
        defer {
            try? FileManager.default.removeItem(at: cacheURL.deletingLastPathComponent())
        }
        let host = "cache-switch-\(UUID().uuidString.lowercased()).brunn.test"
        let baseURL = try XCTUnwrap(URL(string: "https://\(host)/api/v1"))
        let cookieStorage = HTTPCookieStorage.shared
        let accountACookie = try XCTUnwrap(HTTPCookie(properties: [
            .domain: host,
            .path: "/",
            .name: "brunn_session",
            .value: "account-a-session",
            .secure: "TRUE",
        ]))
        let accountBCookie = try XCTUnwrap(HTTPCookie(properties: [
            .domain: host,
            .path: "/",
            .name: "brunn_session",
            .value: "account-b-session",
            .secure: "TRUE",
        ]))
        cookieStorage.setCookie(accountACookie)
        defer {
            cookieStorage.deleteCookie(accountACookie)
            cookieStorage.deleteCookie(accountBCookie)
        }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.httpCookieStorage = cookieStorage
        let api = BrunnAPI(
            configuration: .init(baseURL: baseURL),
            session: URLSession(configuration: configuration),
            cookieStorage: cookieStorage
        )
        let currentAccountAFingerprint = await api.authenticatedSessionFingerprint()
        let accountAFingerprint = try XCTUnwrap(currentAccountAFingerprint)
        let cache = TaskSurfaceCache(fileURL: cacheURL)
        try await cache.save(CachedTaskSurface(
            userID: "user:account-a",
            savedAt: .now,
            urgent: SampleData.agentUrgentTasks,
            next: SampleData.agentNextTasks,
            doneToday: SampleData.agentDoneToday,
            projects: SampleData.agentTaskProjects,
            contexts: SampleData.agentTaskContexts,
            selectedContexts: ["online", "phone"],
            nextRemaining: 7,
            backlogTotal: 18
        ), sessionFingerprint: accountAFingerprint)

        // Reproduce termination after login has persisted account B's cookie
        // but before AppModel.accept(B) can clear account A's presentation.
        cookieStorage.deleteCookie(accountACookie)
        cookieStorage.setCookie(accountBCookie)
        let model = AppModel(
            api: api,
            credentialStore: TestCredentialStore(token: nil),
            taskSurfaceCache: cache,
            bootstrapValidationTimeout: .milliseconds(50),
            storedSessionChecker: { _ in true },
            bootstrapIdentityLoader: { _ in
                try await Task.sleep(for: .seconds(30))
                return Self.readOnlyIdentity
            }
        )

        await model.bootstrap()

        XCTAssertEqual(model.phase, .connectionRequired)
        XCTAssertFalse(model.connectionValidated)
        XCTAssertNil(model.user)
        XCTAssertTrue(model.urgentTasks.isEmpty)
        XCTAssertTrue(model.nextTasks.isEmpty)
        let cachedAfterSwitch = try await cache.load()
        XCTAssertNil(cachedAfterSwitch)
        let currentAccountBFingerprint = await api.authenticatedSessionFingerprint()
        let accountBFingerprint = try XCTUnwrap(currentAccountBFingerprint)
        let boundUserAfterSwitch = try await cache.boundUserID(
            matching: accountBFingerprint
        )
        XCTAssertNil(boundUserAfterSwitch)
    }

    @MainActor
    func testTaskCacheFromAnotherAccountIsClearedBeforePresentation() async throws {
        let cacheURL = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
            .appendingPathComponent("today-tasks.json")
        defer {
            try? FileManager.default.removeItem(at: cacheURL.deletingLastPathComponent())
        }
        let cache = TaskSurfaceCache(fileURL: cacheURL)
        try await cache.save(CachedTaskSurface(
            userID: "user:account-a",
            savedAt: .now,
            urgent: SampleData.agentUrgentTasks,
            next: SampleData.agentNextTasks,
            doneToday: SampleData.agentDoneToday,
            projects: SampleData.agentTaskProjects,
            contexts: SampleData.agentTaskContexts,
            selectedContexts: ["online", "phone"],
            nextRemaining: 7,
            backlogTotal: 18
        ), sessionFingerprint: "sha256:" + String(repeating: "a", count: 64))
        let accountB = MeData(
            user: UserSummary(id: "user:account-b", displayName: "Account B"),
            credentialID: "credential:web-account-b",
            capabilities: ["task.read"],
            readOnly: true
        )
        let model = AppModel(
            api: Self.offlineAPI(),
            credentialStore: TestCredentialStore(token: nil),
            taskSurfaceCache: cache,
            loginLoader: { _, _, _ in accountB },
            dashboardLoader: { _, _ in SampleData.dashboard },
            notificationListLoader: { _, _ in
                NotificationListResponse(items: [], nextCursor: nil, unreadCount: 0)
            }
        )

        await model.connect(email: "account-b@example.test", password: "password")

        XCTAssertEqual(model.user?.id, "user:account-b")
        XCTAssertTrue(model.urgentTasks.isEmpty)
        XCTAssertTrue(model.nextTasks.isEmpty)
        XCTAssertTrue(model.taskProjects.isEmpty)
        XCTAssertTrue(model.taskContexts.isEmpty)
        let cachedAfterSwitch = try await cache.load()
        XCTAssertNil(cachedAfterSwitch)
        let bindingAfterSwitch = try await cache.boundUserID(
            matching: "sha256:" + String(repeating: "a", count: 64)
        )
        XCTAssertNil(bindingAfterSwitch)
    }

    @MainActor
    func testCachedBriefingDoesNotLoadDashboardBeforeCredentialValidation() async throws {
        let cacheURL = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
            .appendingPathComponent("latest-briefing.json")
        defer {
            try? FileManager.default.removeItem(
                at: cacheURL.deletingLastPathComponent()
            )
        }
        let cache = BriefingCache(fileURL: cacheURL)
        try await cache.save(SampleData.briefing)
        let identityGate = IdentityGate()
        let dashboardCalls = CallCounter()
        let model = AppModel(
            api: Self.offlineAPI(),
            credentialStore: TestCredentialStore(token: "sl_cached"),
            briefingCache: cache,
            bootstrapValidationTimeout: .seconds(2),
            storedSessionChecker: { _ in true },
            bootstrapIdentityLoader: { _ in
                await identityGate.load()
            },
            dashboardLoader: { _, _ in
                await dashboardCalls.increment()
                return SampleData.dashboard
            }
        )

        let bootstrap = Task { await model.bootstrap() }
        for _ in 0 ..< 100 where !(model.phase == .ready && !model.connectionValidated) {
            try await Task.sleep(for: .milliseconds(10))
        }
        XCTAssertEqual(model.phase, .ready)
        XCTAssertFalse(model.connectionValidated)

        await model.refreshDashboard()
        let callCountBeforeValidation = await dashboardCalls.value
        XCTAssertEqual(callCountBeforeValidation, 0)

        await identityGate.resolve(with: Self.readOnlyIdentity)
        await bootstrap.value
    }

    @MainActor
    func testLateDashboardResponseCannotRepopulateAfterDisconnect() async throws {
        let dashboardGate = DashboardGate()
        let cacheURL = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
            .appendingPathComponent("latest-briefing.json")
        defer {
            try? FileManager.default.removeItem(
                at: cacheURL.deletingLastPathComponent()
            )
        }
        let model = AppModel(
            api: Self.offlineAPI(),
            // This test isolates dashboard generation invalidation. A saved
            // narrow credential would correctly make disconnect stop until
            // server revocation can be confirmed.
            credentialStore: TestCredentialStore(token: nil),
            briefingCache: BriefingCache(fileURL: cacheURL),
            storedSessionChecker: { _ in true },
            bootstrapIdentityLoader: { _ in Self.readOnlyIdentity },
            dashboardLoader: { _, _ in try await dashboardGate.load() }
        )

        await model.bootstrap()
        for _ in 0 ..< 100 where !(await dashboardGate.hasStarted) {
            try await Task.sleep(for: .milliseconds(10))
        }
        let dashboardRequestStarted = await dashboardGate.hasStarted
        XCTAssertTrue(dashboardRequestStarted)

        await model.disconnect()
        await dashboardGate.resolve(with: SampleData.dashboard)
        try await Task.sleep(for: .milliseconds(30))

        XCTAssertNil(model.dashboard)
        XCTAssertNil(model.dashboardMessage)
        XCTAssertFalse(model.isRefreshingDashboard)
        XCTAssertEqual(model.phase, .connectionRequired)
    }

    func testDashboardFreshnessUsesLocalDayAndFallsBackOnInvalidTimestamp() throws {
        let timezone = try XCTUnwrap(TimeZone(identifier: "America/Los_Angeles"))
        let now = try XCTUnwrap(ISO8601DateFormatter().date(from: "2026-08-02T12:00:00Z"))

        XCTAssertFalse(AppModel.dashboardNeedsRefresh(
            Self.dashboard(generatedAt: "2026-08-02T11:59:00Z"),
            now: now,
            timezone: timezone
        ))
        XCTAssertTrue(AppModel.dashboardNeedsRefresh(
            Self.dashboard(generatedAt: "2026-08-02T11:50:00Z"),
            now: now,
            timezone: timezone
        ))
        XCTAssertTrue(AppModel.dashboardNeedsRefresh(
            Self.dashboard(generatedAt: "not-a-timestamp"),
            now: now,
            timezone: timezone
        ))

        let afterLocalMidnight = try XCTUnwrap(
            ISO8601DateFormatter().date(from: "2026-08-02T07:01:00Z")
        )
        XCTAssertTrue(AppModel.dashboardNeedsRefresh(
            Self.dashboard(generatedAt: "2026-08-02T06:59:00Z"),
            now: afterLocalMidnight,
            timezone: timezone
        ))
    }

    func testDashboardWeekdayKeepsCivilDateAcrossTimeZones() throws {
        let losAngeles = try XCTUnwrap(TimeZone(identifier: "America/Los_Angeles"))
        let tokyo = try XCTUnwrap(TimeZone(identifier: "Asia/Tokyo"))

        XCTAssertEqual(DashboardDate.shortDay("2026-08-02", timezone: losAngeles), "Sun")
        XCTAssertEqual(DashboardDate.shortDay("2026-08-02", timezone: tokyo), "Sun")
    }

    @MainActor
    func testPendingRouteSurvivesConnectionBoundary() async {
        let model = AppModel()
        let notificationRef = "notification:11111111111111111111111111111111"

        await model.handle(.notification(notificationRef: notificationRef, deliveryRef: nil))
        model.enterDemo()

        XCTAssertEqual(model.selectedTab, .alerts)
    }

    @MainActor
    func testPendingTaskRouteLoadsDetailAfterColdDemoBootstrap() async {
        let taskRef = "019f8800-0000-7000-8000-000000000002"
        let model = AppModel()

        await model.handle(.task(reference: taskRef))
        model.enterDemo()

        XCTAssertEqual(model.selectedTab, .tasks)
        XCTAssertEqual(model.presentedTask?.taskRef, taskRef)
    }

    @MainActor
    func testPasswordLoginAcceptsOwnerSession() async {
        let owner = MeData(
            user: UserSummary(id: "user:one", displayName: "Owner"),
            credentialID: "credential:web-owner",
            capabilities: ["read", "credential:manage", "admin"],
            readOnly: false
        )
        let model = AppModel(
            api: Self.offlineAPI(),
            credentialStore: TestCredentialStore(token: "legacy-token"),
            loginLoader: { _, email, password in
                XCTAssertEqual(email, "rourkem@rourkem.com")
                XCTAssertEqual(password, "correct horse battery staple")
                return owner
            }
        )

        await model.connect(
            email: " rourkem@rourkem.com ",
            password: "correct horse battery staple"
        )

        XCTAssertEqual(model.phase, .ready)
        XCTAssertEqual(model.user, owner.user)
        XCTAssertEqual(model.locationReportingUserID, "user:one")
        XCTAssertEqual(model.currentCredentialID, owner.credentialID)
        XCTAssertTrue(model.connectionValidated)
        XCTAssertFalse(model.canManageNotifications)
        XCTAssertFalse(model.canWriteTasks)
    }

    func testNotificationInstallationUsesNarrowBearerWhileReceiptsUseCookieSession() async throws {
        let host = "notification-\(UUID().uuidString.lowercased()).brunn.test"
        let baseURL = try XCTUnwrap(URL(string: "https://\(host)/api/v1"))
        let cookieStorage = HTTPCookieStorage.shared

        let sessionCookie = try XCTUnwrap(HTTPCookie(properties: [
            .domain: host,
            .path: "/",
            .name: "brunn_session",
            .value: "session-secret",
            .secure: "TRUE",
        ]))
        let csrfCookie = try XCTUnwrap(HTTPCookie(properties: [
            .domain: host,
            .path: "/",
            .name: "brunn_csrf",
            .value: "csrf-secret",
            .secure: "TRUE",
        ]))
        cookieStorage.setCookie(sessionCookie)
        cookieStorage.setCookie(csrfCookie)
        defer {
            cookieStorage.deleteCookie(sessionCookie)
            cookieStorage.deleteCookie(csrfCookie)
            NotificationRequestURLProtocol.handler = nil
        }

        let recorder = NotificationRequestRecorder()
        NotificationRequestURLProtocol.handler = { request in
            recorder.append(request)
            if request.httpMethod == "GET" {
                return StubbedHTTPResponse(json: #"""
                {"status":"complete","data":{"view":"next","as_of":"2026-08-27T12:00:00Z","contexts_available":["phone"],"items":[],"urgent_total":0,"next_remaining":0,"backlog_total":0,"next_cursor":null}}
                """#)
            }
            if request.httpMethod == "PUT" {
                return StubbedHTTPResponse(json: #"""
                {"installation_ref":"installation:11111111111111111111111111111111","status":"active","updated_at":"2026-08-02T20:00:00Z"}
                """#)
            }
            return StubbedHTTPResponse(json: #"""
            {"notification_ref":"notification:11111111111111111111111111111111","kind":"opened","delivery_ref":"delivery:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","recorded_at":"2026-08-02T20:01:00Z","replayed":false,"opened_at":"2026-08-02T20:01:00Z","acknowledged_at":null}
            """#)
        }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [NotificationRequestURLProtocol.self]
        configuration.httpCookieStorage = cookieStorage
        configuration.httpShouldSetCookies = true
        let api = BrunnAPI(
            configuration: .init(baseURL: baseURL),
            session: URLSession(configuration: configuration),
            cookieStorage: cookieStorage
        )

        let hasSession = await api.hasAuthenticatedSession()
        XCTAssertTrue(hasSession)
        _ = try await api.taskCandidates(
            view: .next,
            limit: 5,
            contextsAvailable: ["phone"]
        )
        _ = try await api.upsertNotificationInstallation(
            installationID: UUID(uuidString: "11111111-1111-1111-1111-111111111111")!,
            request: NotificationInstallationRequest(
                environment: "development",
                appID: "com.rourkem.brunn",
                deviceToken: "00ff"
            ),
            bearerToken: "narrow-device-token"
        )
        _ = try await api.recordNotificationReceipt(
            notificationRef: "notification:11111111111111111111111111111111",
            kind: .opened,
            deliveryRef: "delivery:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        )

        let requests = recorder.snapshot()
        XCTAssertEqual(requests.map(\.httpMethod), ["GET", "PUT", "POST"])
        XCTAssertNil(requests[0].value(forHTTPHeaderField: "Authorization"))
        XCTAssertTrue(requests[0].httpShouldHandleCookies)
        XCTAssertEqual(
            requests[1].value(forHTTPHeaderField: "Authorization"),
            "Bearer narrow-device-token"
        )
        XCTAssertNil(requests[1].value(forHTTPHeaderField: "Cookie"))
        XCTAssertNil(requests[1].value(forHTTPHeaderField: "X-CSRF-Token"))
        XCTAssertNil(requests.last?.value(forHTTPHeaderField: "Authorization"))
        XCTAssertEqual(requests.last?.value(forHTTPHeaderField: "X-CSRF-Token"), "csrf-secret")
        XCTAssertTrue(requests.last?.httpShouldHandleCookies == true)
        XCTAssertEqual(
            requests[1].url?.path,
            "/api/v1/workspace/notification-installations/11111111-1111-1111-1111-111111111111"
        )
        XCTAssertEqual(
            requests.last?.url?.path,
            "/api/v1/workspace/notifications/notification:11111111111111111111111111111111/receipts"
        )
        let receiptBody = try XCTUnwrap(requests.last?.httpBody)
        let receiptObject = try XCTUnwrap(
            JSONSerialization.jsonObject(with: receiptBody) as? [String: Any]
        )
        XCTAssertEqual(receiptObject["kind"] as? String, "opened")
        XCTAssertEqual(
            receiptObject["delivery_ref"] as? String,
            "delivery:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        )
    }

    func testTaskMutationUsesBearerWithoutCookieOrCSRF() async throws {
        let host = "task-auth-\(UUID().uuidString.lowercased()).brunn.test"
        let baseURL = try XCTUnwrap(URL(string: "https://\(host)/api/v1"))
        let cookieStorage = HTTPCookieStorage.shared
        let sessionCookie = try XCTUnwrap(HTTPCookie(properties: [
            .domain: host,
            .path: "/",
            .name: "brunn_session",
            .value: "must-not-leak",
            .secure: "TRUE",
        ]))
        let csrfCookie = try XCTUnwrap(HTTPCookie(properties: [
            .domain: host,
            .path: "/",
            .name: "brunn_csrf",
            .value: "must-not-leak",
            .secure: "TRUE",
        ]))
        cookieStorage.setCookie(sessionCookie)
        cookieStorage.setCookie(csrfCookie)
        defer {
            cookieStorage.deleteCookie(sessionCookie)
            cookieStorage.deleteCookie(csrfCookie)
            NotificationRequestURLProtocol.handler = nil
        }

        let recorder = NotificationRequestRecorder()
        let taskRef = "019f8800-0000-7000-8000-000000000001"
        NotificationRequestURLProtocol.handler = { request in
            recorder.append(request)
            return StubbedHTTPResponse(json: #"""
            {"status":"committed","data":{"task":{"task_ref":"019f8800-0000-7000-8000-000000000001","entry_ref":"entry:one","version":2,"title":"Done","status":"done","task":{"id":"019f8800-0000-7000-8000-000000000001","title":"Done"},"created_at":"2026-08-27T12:00:00Z","updated_at":"2026-08-27T12:01:00Z"},"action":"complete","correction_ref":null,"done_today_count":1,"next_occurrence_task_ref":null,"replayed":false}}
            """#)
        }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [NotificationRequestURLProtocol.self]
        configuration.httpCookieStorage = cookieStorage
        configuration.httpShouldSetCookies = true
        let api = BrunnAPI(
            configuration: .init(baseURL: baseURL),
            session: URLSession(configuration: configuration),
            cookieStorage: cookieStorage
        )

        _ = try await api.updateTask(
            reference: taskRef,
            request: AgentTaskUpdateRequest(
                expectedVersion: 1,
                idempotencyKey: "ios:test:auth",
                operation: .complete
            ),
            bearerToken: "exact-narrow-token"
        )

        let request = try XCTUnwrap(recorder.snapshot().first)
        XCTAssertEqual(request.httpMethod, "PATCH")
        XCTAssertEqual(
            request.value(forHTTPHeaderField: "Authorization"),
            "Bearer exact-narrow-token"
        )
        XCTAssertNil(request.value(forHTTPHeaderField: "Cookie"))
        XCTAssertNil(request.value(forHTTPHeaderField: "X-CSRF-Token"))
    }

    func testTodoistStatusReadIsContentFreeAndUsesTheReadSession() async throws {
        defer { NotificationRequestURLProtocol.handler = nil }
        let recorder = NotificationRequestRecorder()
        NotificationRequestURLProtocol.handler = { request in
            recorder.append(request)
            return StubbedHTTPResponse(json: #"""
            {"status":"complete","data":{"environment_enabled":false,"saved_mode":"off","effective_mode":"off","token_configured":false,"configuration_generation":3,"last_run_at":null,"last_outcome":null,"last_error_code":null,"next_run_at":null}}
            """#)
        }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [NotificationRequestURLProtocol.self]
        let api = BrunnAPI(
            configuration: .init(baseURL: try XCTUnwrap(URL(string: "https://todoist-status.brunn.test/api/v1"))),
            session: URLSession(configuration: configuration)
        )

        let response = try await api.taskTodoistStatus()

        XCTAssertFalse(response.data.environmentEnabled)
        XCTAssertFalse(response.data.tokenConfigured)
        XCTAssertEqual(response.data.savedMode, "off")
        XCTAssertEqual(response.data.effectiveMode, "off")
        XCTAssertEqual(response.data.configurationGeneration, 3)
        let request = try XCTUnwrap(recorder.snapshot().first)
        XCTAssertEqual(request.httpMethod, "GET")
        XCTAssertEqual(request.url?.path, "/api/v1/workspace/integrations/todoist/status")
        XCTAssertNil(request.value(forHTTPHeaderField: "Authorization"))
    }

    @MainActor
    func testTaskActionBootstrapRepairsAStoredCredentialInsteadOfSilentlyReturning() async throws {
        let credentialRef = "credential:12121212-1212-4212-8212-121212121212"
        let baseURL = try XCTUnwrap(URL(string: "https://repair-device.brunn.test/api/v1"))
        let recorder = NotificationRequestRecorder()
        NotificationRequestURLProtocol.handler = { request in
            recorder.append(request)
            if request.httpMethod == "GET", request.url?.path == "/api/v1/me" {
                let identityAttempts = recorder.snapshot().filter {
                    $0.httpMethod == "GET" && $0.url?.path == "/api/v1/me"
                }.count
                if identityAttempts == 1 {
                    return StubbedHTTPResponse(
                        statusCode: 503,
                        json: #"{"error":{"code":"temporarily_unavailable","message":"retry"}}"#
                    )
                }
                return StubbedHTTPResponse(json: #"""
                {"user":{"id":"user:one","display_name":"Owner"},"credential_id":"credential:12121212-1212-4212-8212-121212121212","capabilities":["task.write","notification:manage"],"read_only":false}
                """#)
            }
            return StubbedHTTPResponse(
                statusCode: 503,
                json: #"{"error":{"code":"test_background_request","message":"not part of repair test"}}"#
            )
        }
        defer { NotificationRequestURLProtocol.handler = nil }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [NotificationRequestURLProtocol.self]
        let api = BrunnAPI(
            configuration: .init(baseURL: baseURL),
            session: URLSession(configuration: configuration)
        )
        let store = TestCredentialStore(credential: DeviceTaskCredential(
            credentialRef: credentialRef,
            token: "protected-narrow-token",
            userID: "user:one",
            capabilities: ["notification:manage", "task.write"]
        ))
        let owner = MeData(
            user: UserSummary(id: "user:one", displayName: "Owner"),
            credentialID: "credential:web-owner",
            capabilities: ["credential:manage", "admin", "task.read"],
            readOnly: false
        )
        let model = AppModel(
            api: api,
            credentialStore: store,
            loginLoader: { _, _, _ in owner },
            dashboardLoader: { _, _ in SampleData.dashboard },
            notificationListLoader: { _, _ in
                NotificationListResponse(items: [], nextCursor: nil, unreadCount: 0)
            }
        )

        await model.connect(email: "owner@example.test", password: "password")
        XCTAssertTrue(model.hasStoredDeviceTaskCredential)
        XCTAssertFalse(model.canWriteTasks)

        await model.bootstrapDeviceTaskAccess()

        XCTAssertTrue(model.canWriteTasks)
        XCTAssertTrue(model.canManageNotifications)
        XCTAssertTrue(model.hasStoredDeviceTaskCredential)
        XCTAssertNil(model.deviceTaskAccessMessage)
        let lifecycle = recorder.snapshot().filter {
            $0.url?.path == "/api/v1/me" || $0.url?.path == "/api/v1/credentials"
        }
        XCTAssertEqual(lifecycle.map(\.httpMethod), ["GET", "GET"])
    }

    func testDeviceTaskCredentialBootstrapUsesOwnerCookieThenValidatesBearerAndRevokesWithCookie() async throws {
        let host = "device-bootstrap-\(UUID().uuidString.lowercased()).brunn.test"
        let baseURL = try XCTUnwrap(URL(string: "https://\(host)/api/v1"))
        let cookieStorage = HTTPCookieStorage.shared
        let sessionCookie = try XCTUnwrap(HTTPCookie(properties: [
            .domain: host,
            .path: "/",
            .name: "brunn_session",
            .value: "owner-session",
            .secure: "TRUE",
        ]))
        let csrfCookie = try XCTUnwrap(HTTPCookie(properties: [
            .domain: host,
            .path: "/",
            .name: "brunn_csrf",
            .value: "owner-csrf",
            .secure: "TRUE",
        ]))
        cookieStorage.setCookie(sessionCookie)
        cookieStorage.setCookie(csrfCookie)
        defer {
            cookieStorage.deleteCookie(sessionCookie)
            cookieStorage.deleteCookie(csrfCookie)
            NotificationRequestURLProtocol.handler = nil
        }

        let recorder = NotificationRequestRecorder()
        NotificationRequestURLProtocol.handler = { request in
            recorder.append(request)
            switch request.httpMethod {
            case "POST":
                return StubbedHTTPResponse(json: #"""
                {"id":"credential:11111111-1111-4111-8111-111111111111","access":"ios_tasks","capabilities":["task.write","notification:manage"],"token":"one-time-narrow-token"}
                """#)
            case "GET":
                return StubbedHTTPResponse(json: #"""
                {"user":{"id":"user:one","display_name":"Owner"},"credential_id":"credential:11111111-1111-4111-8111-111111111111","capabilities":["notification:manage","task.write"],"read_only":false}
                """#)
            default:
                return StubbedHTTPResponse(json: #"""
                {"id":"credential:11111111-1111-4111-8111-111111111111","status":"revoked","revoked_at":"2026-08-27T12:00:00Z"}
                """#)
            }
        }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [NotificationRequestURLProtocol.self]
        configuration.httpCookieStorage = cookieStorage
        configuration.httpShouldSetCookies = true
        let api = BrunnAPI(
            configuration: .init(baseURL: baseURL),
            session: URLSession(configuration: configuration),
            cookieStorage: cookieStorage
        )

        let issued = try await api.bootstrapDeviceTaskCredential()
        _ = try await api.deviceCredentialIdentity(bearerToken: issued.token)
        _ = try await api.revokeCredential(reference: issued.id)

        let requests = recorder.snapshot()
        XCTAssertEqual(requests.map(\.httpMethod), ["POST", "GET", "DELETE"])
        let createBody = try XCTUnwrap(requests[0].httpBody)
        let create = try XCTUnwrap(
            JSONSerialization.jsonObject(with: createBody) as? [String: String]
        )
        XCTAssertEqual(create, [
            "name": "iOS task access",
            "access": "ios_tasks",
        ])
        XCTAssertNil(requests[0].value(forHTTPHeaderField: "Authorization"))
        XCTAssertEqual(requests[0].value(forHTTPHeaderField: "X-CSRF-Token"), "owner-csrf")
        XCTAssertTrue(requests[0].httpShouldHandleCookies)

        XCTAssertEqual(
            requests[1].value(forHTTPHeaderField: "Authorization"),
            "Bearer one-time-narrow-token"
        )
        XCTAssertNil(requests[1].value(forHTTPHeaderField: "Cookie"))
        XCTAssertNil(requests[1].value(forHTTPHeaderField: "X-CSRF-Token"))

        XCTAssertNil(requests[2].value(forHTTPHeaderField: "Authorization"))
        XCTAssertEqual(requests[2].value(forHTTPHeaderField: "X-CSRF-Token"), "owner-csrf")
        XCTAssertTrue(requests[2].httpShouldHandleCookies)
        XCTAssertEqual(
            requests[2].url?.path,
            "/api/v1/credentials/credential:11111111-1111-4111-8111-111111111111"
        )
    }

    func testLoginResponseSynchronizesCSRFCookieBeforeImmediateMutation() async throws {
        let host = "login-csrf-\(UUID().uuidString.lowercased()).brunn.test"
        let baseURL = try XCTUnwrap(URL(string: "https://\(host)/api/v1"))
        let fingerprintDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        let fingerprintURL = fingerprintDirectory
            .appendingPathComponent("web-session-fingerprint-v1", isDirectory: false)
        let cookieStorage = HTTPCookieStorage.shared
        for cookie in cookieStorage.cookies(for: baseURL) ?? [] {
            cookieStorage.deleteCookie(cookie)
        }
        defer {
            for cookie in cookieStorage.cookies(for: baseURL) ?? [] {
                cookieStorage.deleteCookie(cookie)
            }
            try? FileManager.default.removeItem(at: fingerprintDirectory)
            NotificationRequestURLProtocol.handler = nil
        }

        let recorder = NotificationRequestRecorder()
        NotificationRequestURLProtocol.handler = { request in
            recorder.append(request)
            switch request.url?.path {
            case "/api/v1/auth/login":
                return StubbedHTTPResponse(
                    json: #"{"status":"complete","data":{"user":{"id":"user:one","display_name":"Owner"},"expires_at":"2026-09-27T12:00:00Z"}}"#,
                    headers: [
                        "Set-Cookie": "brunn_session=sws_test_session; Path=/; HttpOnly; SameSite=Strict, brunn_csrf=csrf-from-login; Path=/; SameSite=Strict",
                    ]
                )
            case "/api/v1/credentials":
                return StubbedHTTPResponse(json: #"{"id":"credential:11111111-1111-4111-8111-111111111111","access":"ios_tasks","capabilities":["task.write","notification:manage"],"token":"one-time-narrow-token"}"#)
            default:
                return StubbedHTTPResponse(statusCode: 404, json: #"{"error":{"code":"unexpected","message":"unexpected"}}"#)
            }
        }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [NotificationRequestURLProtocol.self]
        // Keep the transport cookie jar detached so this test can only pass
        // through BrunnAPI's explicit response-cookie synchronization.
        configuration.httpCookieStorage = nil
        configuration.httpShouldSetCookies = false
        let api = BrunnAPI(
            configuration: .init(baseURL: baseURL),
            session: URLSession(configuration: configuration),
            cookieStorage: cookieStorage,
            sessionFingerprintURL: fingerprintURL
        )

        _ = try await api.login(email: "owner@example.test", password: "not-a-secret")
        let hasAuthenticatedSession = await api.hasAuthenticatedSession()
        XCTAssertTrue(hasAuthenticatedSession)
        let sessionFingerprint = await api.authenticatedSessionFingerprint()
        XCTAssertTrue(sessionFingerprint?.hasPrefix("sha256:") == true)
        XCTAssertEqual(
            try String(contentsOf: fingerprintURL, encoding: .utf8),
            sessionFingerprint
        )
        XCTAssertFalse(
            try String(contentsOf: fingerprintURL, encoding: .utf8)
                .contains("sws_test_session")
        )
        let fingerprintAttributes = try FileManager.default.attributesOfItem(
            atPath: fingerprintURL.path
        )
        let fingerprintProtection = fingerprintAttributes[.protectionKey] as? FileProtectionType
#if targetEnvironment(simulator)
        // Simulator filesystems can omit NSFileProtectionKey entirely even
        // when the complete-protection write option was applied.
        XCTAssertTrue(fingerprintProtection == nil || fingerprintProtection == .complete)
#else
        XCTAssertEqual(fingerprintProtection, .complete)
#endif
        XCTAssertEqual(
            try fingerprintURL.resourceValues(forKeys: [.isExcludedFromBackupKey])
                .isExcludedFromBackup,
            true
        )
        XCTAssertEqual(
            try fingerprintDirectory.resourceValues(forKeys: [.isExcludedFromBackupKey])
                .isExcludedFromBackup,
            true
        )
        _ = try await api.bootstrapDeviceTaskCredential()

        let requests = recorder.snapshot()
        XCTAssertEqual(requests.map(\.url?.path), [
            "/api/v1/auth/login",
            "/api/v1/credentials",
        ])
        XCTAssertTrue(requests[0].httpShouldHandleCookies)
        XCTAssertNil(requests[0].value(forHTTPHeaderField: "X-CSRF-Token"))
        XCTAssertTrue(requests[1].httpShouldHandleCookies)
        XCTAssertEqual(
            requests[1].value(forHTTPHeaderField: "X-CSRF-Token"),
            "csrf-from-login"
        )
        XCTAssertEqual(
            Set((cookieStorage.cookies(for: baseURL) ?? []).map(\.name)),
            Set(["brunn_session", "brunn_csrf"])
        )

        for cookie in cookieStorage.cookies(for: baseURL) ?? []
            where cookie.name == "brunn_session"
        {
            cookieStorage.deleteCookie(cookie)
        }
        let replacementCookie = try XCTUnwrap(HTTPCookie(properties: [
            .domain: host,
            .path: "/",
            .name: "brunn_session",
            .value: "replacement-account-session",
            .secure: "TRUE",
        ]))
        cookieStorage.setCookie(replacementCookie)
        let replacementAPI = BrunnAPI(
            configuration: .init(baseURL: baseURL),
            session: URLSession(configuration: configuration),
            cookieStorage: cookieStorage,
            sessionFingerprintURL: fingerprintURL
        )
        let replacementFingerprint = await replacementAPI.authenticatedSessionFingerprint()
        XCTAssertNotEqual(replacementFingerprint, sessionFingerprint)
        XCTAssertEqual(
            try String(contentsOf: fingerprintURL, encoding: .utf8),
            replacementFingerprint
        )

        cookieStorage.deleteCookie(replacementCookie)
        let restoredAPI = BrunnAPI(
            configuration: .init(baseURL: baseURL),
            session: URLSession(configuration: configuration),
            cookieStorage: cookieStorage,
            sessionFingerprintURL: fingerprintURL
        )
        let restoredFingerprint = await restoredAPI.authenticatedSessionFingerprint()
        XCTAssertEqual(restoredFingerprint, replacementFingerprint)
        cookieStorage.setCookie(replacementCookie)
        await restoredAPI.clearAuthenticatedSession()
        let clearedFingerprint = await restoredAPI.authenticatedSessionFingerprint()
        XCTAssertNil(clearedFingerprint)
        XCTAssertFalse(FileManager.default.fileExists(atPath: fingerprintURL.path))
        XCTAssertFalse(
            (cookieStorage.cookies(for: baseURL) ?? []).contains {
                $0.name == "brunn_session"
            }
        )

        try Data(("sha256:" + String(repeating: "١", count: 64)).utf8)
            .write(to: fingerprintURL, options: .atomic)
        let corruptAPI = BrunnAPI(
            configuration: .init(baseURL: baseURL),
            session: URLSession(configuration: configuration),
            cookieStorage: cookieStorage,
            sessionFingerprintURL: fingerprintURL
        )
        let corruptFingerprint = await corruptAPI.authenticatedSessionFingerprint()
        XCTAssertNil(corruptFingerprint)
    }

    @MainActor
    func testPostIssueKeychainFailureRevokesServerCredentialAndFailsClosed() async throws {
        let baseURL = try XCTUnwrap(URL(string: "https://bootstrap-lifecycle.brunn.test/api/v1"))
        let recorder = NotificationRequestRecorder()
        NotificationRequestURLProtocol.handler = { request in
            recorder.append(request)
            switch (request.httpMethod, request.url?.path) {
            case ("POST", "/api/v1/credentials"):
                return StubbedHTTPResponse(json: #"""
                {"id":"credential:22222222-2222-4222-8222-222222222222","access":"ios_tasks","capabilities":["task.write","notification:manage"],"token":"issued-once"}
                """#)
            case ("GET", "/api/v1/me"):
                return StubbedHTTPResponse(json: #"""
                {"user":{"id":"user:one","display_name":"Owner"},"credential_id":"credential:22222222-2222-4222-8222-222222222222","capabilities":["task.write","notification:manage"],"read_only":false}
                """#)
            case ("DELETE", "/api/v1/credentials/credential:22222222-2222-4222-8222-222222222222"):
                return StubbedHTTPResponse(json: #"""
                {"id":"credential:22222222-2222-4222-8222-222222222222","status":"revoked","revoked_at":"2026-08-27T12:00:00Z"}
                """#)
            default:
                return StubbedHTTPResponse(
                    statusCode: 503,
                    json: #"{"error":{"code":"test_background_request","message":"not part of lifecycle test"}}"#
                )
            }
        }
        defer { NotificationRequestURLProtocol.handler = nil }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [NotificationRequestURLProtocol.self]
        let api = BrunnAPI(
            configuration: .init(baseURL: baseURL),
            session: URLSession(configuration: configuration)
        )
        let store = TestCredentialStore(token: nil, saveError: .forced)
        let owner = MeData(
            user: UserSummary(id: "user:one", displayName: "Owner"),
            credentialID: "credential:web-owner",
            capabilities: ["credential:manage", "admin", "task.read"],
            readOnly: false
        )
        let model = AppModel(
            api: api,
            credentialStore: store,
            loginLoader: { _, _, _ in owner },
            dashboardLoader: { _, _ in SampleData.dashboard },
            notificationListLoader: { _, _ in
                NotificationListResponse(items: [], nextCursor: nil, unreadCount: 0)
            }
        )

        await model.connect(email: "owner@example.test", password: "password")
        await model.bootstrapDeviceTaskAccess()

        let lifecycle = recorder.snapshot().filter {
            $0.url?.path == "/api/v1/credentials"
                || $0.url?.path == "/api/v1/me"
                || $0.url?.path.contains("credential:22222222") == true
        }
        XCTAssertEqual(lifecycle.map(\.httpMethod), ["POST", "GET", "DELETE"])
        XCTAssertFalse(model.canWriteTasks)
        XCTAssertFalse(model.canManageNotifications)
        XCTAssertFalse(model.hasStoredDeviceTaskCredential)
        XCTAssertTrue(store.wasDeleted)
    }

    @MainActor
    func testDisconnectStopsBeforeLogoutOrLocalDeleteWhenNarrowRevocationFails() async throws {
        let baseURL = try XCTUnwrap(URL(string: "https://disconnect-lifecycle.brunn.test/api/v1"))
        let recorder = NotificationRequestRecorder()
        NotificationRequestURLProtocol.handler = { request in
            recorder.append(request)
            if request.httpMethod == "GET", request.url?.path == "/api/v1/me" {
                return StubbedHTTPResponse(json: #"""
                {"user":{"id":"user:one","display_name":"Owner"},"credential_id":"credential:33333333-3333-4333-8333-333333333333","capabilities":["notification:manage","task.write"],"read_only":false}
                """#)
            }
            return StubbedHTTPResponse(
                statusCode: 503,
                json: #"{"error":{"code":"revocation_unavailable","message":"retry online"}}"#
            )
        }
        defer { NotificationRequestURLProtocol.handler = nil }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [NotificationRequestURLProtocol.self]
        let api = BrunnAPI(
            configuration: .init(baseURL: baseURL),
            session: URLSession(configuration: configuration)
        )
        let store = TestCredentialStore(
            credential: DeviceTaskCredential(
                credentialRef: "credential:33333333-3333-4333-8333-333333333333",
                token: "protected-narrow-token"
            )
        )
        let owner = MeData(
            user: UserSummary(id: "user:one", displayName: "Owner"),
            credentialID: "credential:web-owner",
            capabilities: ["credential:manage", "admin", "task.read"],
            readOnly: false
        )
        let model = AppModel(
            api: api,
            credentialStore: store,
            loginLoader: { _, _, _ in owner },
            dashboardLoader: { _, _ in SampleData.dashboard },
            notificationListLoader: { _, _ in
                NotificationListResponse(items: [], nextCursor: nil, unreadCount: 0)
            }
        )

        await model.connect(email: "owner@example.test", password: "password")
        XCTAssertTrue(model.canWriteTasks)
        XCTAssertFalse(model.canWriteMessages)
        await model.disconnect()

        XCTAssertEqual(model.phase, .ready)
        XCTAssertFalse(model.canWriteTasks)
        XCTAssertTrue(model.hasStoredDeviceTaskCredential)
        XCTAssertFalse(store.wasDeleted)
        XCTAssertTrue(model.privacyMessage?.contains("Disconnect stopped") == true)
        XCTAssertFalse(recorder.snapshot().contains { $0.url?.path == "/api/v1/auth/logout" })
    }

    @MainActor
    func testStoredDeviceCredentialFromAnotherAccountNeverEnablesMutations() async throws {
        let credentialRef = "credential:44444444-4444-4444-8444-444444444444"
        let baseURL = try XCTUnwrap(URL(string: "https://account-switch.brunn.test/api/v1"))
        let recorder = NotificationRequestRecorder()
        NotificationRequestURLProtocol.handler = { request in
            recorder.append(request)
            if request.httpMethod == "GET", request.url?.path == "/api/v1/me" {
                return StubbedHTTPResponse(json: #"""
                {"user":{"id":"user:account-a","display_name":"Account A"},"credential_id":"credential:44444444-4444-4444-8444-444444444444","capabilities":["task.write","notification:manage"],"read_only":false}
                """#)
            }
            if request.httpMethod == "DELETE",
               request.url?.path == "/api/v1/credentials/\(credentialRef)"
            {
                return StubbedHTTPResponse(
                    statusCode: 403,
                    json: #"{"error":{"code":"credential_scope_mismatch","message":"credential belongs to another account"}}"#
                )
            }
            return StubbedHTTPResponse(
                statusCode: 503,
                json: #"{"error":{"code":"test_background_request","message":"not part of account-switch test"}}"#
            )
        }
        defer { NotificationRequestURLProtocol.handler = nil }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [NotificationRequestURLProtocol.self]
        let api = BrunnAPI(
            configuration: .init(baseURL: baseURL),
            session: URLSession(configuration: configuration)
        )
        let store = TestCredentialStore(credential: DeviceTaskCredential(
            credentialRef: credentialRef,
            token: "account-a-narrow-token"
        ))
        let accountB = MeData(
            user: UserSummary(id: "user:account-b", displayName: "Account B"),
            credentialID: "credential:web-account-b",
            capabilities: ["credential:manage", "admin", "task.read"],
            readOnly: false
        )
        let model = AppModel(
            api: api,
            credentialStore: store,
            loginLoader: { _, _, _ in accountB },
            dashboardLoader: { _, _ in SampleData.dashboard },
            notificationListLoader: { _, _ in
                NotificationListResponse(items: [], nextCursor: nil, unreadCount: 0)
            }
        )

        await model.connect(email: "account-b@example.test", password: "password")

        XCTAssertEqual(model.user?.id, "user:account-b")
        XCTAssertFalse(model.canWriteTasks)
        XCTAssertFalse(model.canManageNotifications)
        XCTAssertTrue(model.hasStoredDeviceTaskCredential)
        XCTAssertFalse(store.wasDeleted)
        XCTAssertTrue(model.deviceTaskAccessMessage?.contains("did not match this account") == true)
        let lifecycle = recorder.snapshot().filter {
            $0.url?.path == "/api/v1/me"
                || $0.url?.path == "/api/v1/credentials/\(credentialRef)"
        }
        XCTAssertEqual(lifecycle.map(\.httpMethod), ["GET", "DELETE"])
        XCTAssertEqual(
            lifecycle.first?.value(forHTTPHeaderField: "Authorization"),
            "Bearer account-a-narrow-token"
        )
        XCTAssertNil(lifecycle.last?.value(forHTTPHeaderField: "Authorization"))
    }

    @MainActor
    func testExactMessagingDeviceCredentialEnablesOnlyApprovedMutationSurfaces() async throws {
        let credentialRef = "credential:55555555-5555-4555-8555-555555555555"
        let baseURL = try XCTUnwrap(URL(string: "https://messaging-device.brunn.test/api/v1"))
        NotificationRequestURLProtocol.handler = { request in
            if request.httpMethod == "GET", request.url?.path == "/api/v1/me" {
                return StubbedHTTPResponse(json: #"""
                {"user":{"id":"user:one","display_name":"Owner"},"credential_id":"credential:55555555-5555-4555-8555-555555555555","capabilities":["task.write","message.write","notification:manage"],"read_only":false}
                """#)
            }
            return StubbedHTTPResponse(
                statusCode: 503,
                json: #"{"error":{"code":"test_background_request","message":"not part of credential test"}}"#
            )
        }
        defer { NotificationRequestURLProtocol.handler = nil }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [NotificationRequestURLProtocol.self]
        let api = BrunnAPI(
            configuration: .init(baseURL: baseURL),
            session: URLSession(configuration: configuration)
        )
        let credentialStore = TestCredentialStore(credential: DeviceTaskCredential(
            credentialRef: credentialRef,
            token: "protected-messaging-token"
        ))
        let owner = MeData(
            user: UserSummary(id: "user:one", displayName: "Owner"),
            credentialID: "credential:web-owner",
            capabilities: ["credential:manage", "admin", "task.read", "message.read"],
            readOnly: false
        )
        let model = AppModel(
            api: api,
            credentialStore: credentialStore,
            loginLoader: { _, _, _ in owner },
            dashboardLoader: { _, _ in SampleData.dashboard },
            notificationListLoader: { _, _ in
                NotificationListResponse(items: [], nextCursor: nil, unreadCount: 0)
            }
        )

        await model.connect(email: "owner@example.test", password: "password")

        XCTAssertTrue(model.canWriteTasks)
        XCTAssertTrue(model.canWriteMessages)
        XCTAssertTrue(model.canManageNotifications)
        XCTAssertEqual(model.deviceTaskBearer(), "protected-messaging-token")
        XCTAssertEqual(
            try credentialStore.load()?.capabilities,
            ["message.write", "notification:manage", "task.write"]
        )
        XCTAssertEqual(try credentialStore.load()?.userID, "user:one")
    }

    private static let readOnlyIdentity = MeData(
        user: UserSummary(id: "user:one", displayName: "Owner"),
        capabilities: ["open", "query", "read", "compute", "verify", "status"],
        readOnly: true
    )

    private static let notificationIdentity = MeData(
        user: UserSummary(id: "user:one", displayName: "Owner"),
        credentialID: "credential:web-owner",
        capabilities: ["query", "read", "status", "notification:manage"],
        readOnly: false
    )

    private static func testNotification(openedAt: String? = nil) -> BrunnNotification {
        BrunnNotification(
            notificationRef: "notification:11111111111111111111111111111111",
            kind: .briefingReady,
            importance: .important,
            title: "Morning briefing ready",
            body: "Open the durable detail before continuing.",
            source: BrunnNotificationSource(
                type: "entry",
                reference: "entry:morning",
                versionRef: "version:morning-v3"
            ),
            target: BrunnNotificationTarget(
                type: .briefing,
                date: "2026-08-02",
                edition: "morning",
                itemID: "native-ios"
            ),
            occurredAt: "2026-08-02T06:30:00Z",
            openedAt: openedAt,
            deliveries: [
                BrunnNotificationDelivery(
                    deliveryRef: "delivery:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    state: .acceptedByAPNs,
                    acceptedAt: "2026-08-02T06:30:02Z"
                ),
            ]
        )
    }

    private static func offlineAPI() -> BrunnAPI {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.waitsForConnectivity = false
        configuration.timeoutIntervalForRequest = 0.1
        configuration.timeoutIntervalForResource = 0.1
        return BrunnAPI(
            configuration: .init(
                baseURL: URL(string: "http://127.0.0.1:9/api/v1")!
            ),
            session: URLSession(configuration: configuration)
        )
    }

    private static func dashboard(generatedAt: String) -> WorkspaceDashboardData {
        let dashboard = SampleData.dashboard
        return WorkspaceDashboardData(
            generatedAt: generatedAt,
            timezone: dashboard.timezone,
            workspaceGeneration: dashboard.workspaceGeneration,
            activityTrackingStartedAt: dashboard.activityTrackingStartedAt,
            tracking: dashboard.tracking,
            storage: dashboard.storage,
            today: dashboard.today,
            activity: dashboard.activity,
            access: dashboard.access,
            coverage: dashboard.coverage
        )
    }

    func testNotificationParserRejectsArbitraryPayload() {
        XCTAssertNil(NotificationRouteParser.route(from: ["url": "https://example.com"]))
    }

    func testNotificationParserAcceptsTypedRoute() {
        let notificationID = "11111111111111111111111111111111"
        let deliveryID = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        XCTAssertEqual(
            NotificationRouteParser.route(
                from: [
                    "schema": "brunn-push@v1",
                    "notification_ref": "notification:\(notificationID)",
                    "delivery_ref": "delivery:\(deliveryID)",
                    "brunn_route": "brunn://notification/\(notificationID)?delivery=\(deliveryID)",
                ]
            ),
            .notification(
                notificationRef: "notification:\(notificationID)",
                deliveryRef: "delivery:\(deliveryID)"
            )
        )
    }

    func testNotificationParserRejectsMismatchedOrUnversionedPayloads() {
        let notificationID = "11111111111111111111111111111111"
        let deliveryID = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        let route = "brunn://notification/\(notificationID)?delivery=\(deliveryID)"

        XCTAssertNil(NotificationRouteParser.route(from: [
            "notification_ref": "notification:\(notificationID)",
            "delivery_ref": "delivery:\(deliveryID)",
            "brunn_route": route,
        ]))
        XCTAssertNil(NotificationRouteParser.route(from: [
            "schema": "brunn-push@v1",
            "notification_ref": "notification:22222222222222222222222222222222",
            "delivery_ref": "delivery:\(deliveryID)",
            "brunn_route": route,
        ]))
    }

    func testPushRouteBufferRetainsOneColdLaunchRoute() {
        _ = PushRouteBuffer.shared.take()
        let route = AppRoute.notification(
            notificationRef: "notification:11111111111111111111111111111111",
            deliveryRef: "delivery:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        )

        PushRouteBuffer.shared.store(route)

        XCTAssertEqual(PushRouteBuffer.shared.take(), route)
        XCTAssertNil(PushRouteBuffer.shared.take())
    }

    func testNotificationResponseCompletesOnMainBeforePublishingBufferedRoute() async {
        _ = PushRouteBuffer.shared.take()
        let route = AppRoute.notification(
            notificationRef: "notification:11111111111111111111111111111111",
            deliveryRef: "delivery:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        )
        let completed = expectation(description: "notification response completed")
        let published = expectation(description: "push route published")
        let events = NotificationHandoffRecorder()
        let observer = NotificationCenter.default.addObserver(
            forName: .brunnPushRoute,
            object: nil,
            queue: .main
        ) { _ in
            events.append("published")
            published.fulfill()
        }
        defer { NotificationCenter.default.removeObserver(observer) }

        NotificationDelegateHandoff.finishResponse(route: route) {
            XCTAssertTrue(Thread.isMainThread)
            XCTAssertEqual(PushRouteBuffer.shared.take(), route)
            events.append("completed")
            completed.fulfill()
        }

        await fulfillment(of: [completed, published], timeout: 1)
        XCTAssertEqual(events.snapshot(), ["completed", "published"])
    }

    @MainActor
    func testAppDelegateExportsExplicitNotificationCompletionSelectors() {
        let delegate = AppDelegate()

        XCTAssertTrue(delegate.responds(to: NSSelectorFromString(
            "userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:"
        )))
        XCTAssertTrue(delegate.responds(to: NSSelectorFromString(
            "userNotificationCenter:willPresentNotification:withCompletionHandler:"
        )))
    }

    func testPushTokenBufferRetainsOnlyInMemoryUntilConsumed() {
        _ = PushTokenBuffer.shared.take()
        let token = Data([0x00, 0x7f, 0xff])

        PushTokenBuffer.shared.store(token)

        XCTAssertEqual(PushTokenBuffer.shared.take(), token)
        XCTAssertNil(PushTokenBuffer.shared.take())
    }

    @MainActor
    func testColdAndWarmPushTapFetchDetailBeforePresentationAndAttributeDeliveryOpen() async {
        let trace = NotificationTrace()
        let notification = Self.testNotification(openedAt: "2026-08-02T06:40:00Z")
        let deliveryRef = "delivery:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        let model = AppModel(
            api: Self.offlineAPI(),
            credentialStore: TestCredentialStore(token: nil),
            storedSessionChecker: { _ in true },
            bootstrapIdentityLoader: { _ in Self.notificationIdentity },
            dashboardLoader: { _, _ in SampleData.dashboard },
            notificationListLoader: { _, _ in
                NotificationListResponse(items: [notification], unreadCount: 0)
            },
            notificationDetailLoader: { _, reference in
                await trace.append("detail:\(reference)")
                return notification
            },
            notificationReceiptWriter: { _, reference, kind, delivery in
                await trace.append("receipt:\(delivery ?? "none")")
                return NotificationReceiptResponse(
                    notificationRef: reference,
                    kind: kind,
                    deliveryRef: delivery,
                    recordedAt: "2026-08-02T06:41:00Z",
                    replayed: false,
                    openedAt: "2026-08-02T06:40:00Z",
                    acknowledgedAt: nil
                )
            }
        )

        await model.handle(.notification(
            notificationRef: notification.notificationRef,
            deliveryRef: deliveryRef
        ))
        XCTAssertNil(model.presentedNotification)

        await model.bootstrap()

        XCTAssertEqual(model.selectedTab, .alerts)
        XCTAssertEqual(model.presentedNotification?.notificationRef, notification.notificationRef)
        XCTAssertNil(model.focusedBriefingItemID, "A push must not bypass durable alert detail.")
        let coldTrace = await trace.values
        XCTAssertEqual(coldTrace, [
            "detail:\(notification.notificationRef)",
            "receipt:\(deliveryRef)",
        ])

        model.presentedNotification = nil
        await model.handle(.notification(
            notificationRef: notification.notificationRef,
            deliveryRef: nil
        ))
        XCTAssertEqual(model.presentedNotification?.notificationRef, notification.notificationRef)
        let warmTrace = await trace.values
        XCTAssertEqual(warmTrace.last, "detail:\(notification.notificationRef)")
    }

    @MainActor
    func testNotificationTargetIsASecondaryActionToExactBriefingItem() async throws {
        let model = AppModel()
        model.enterDemo()
        let notification = try XCTUnwrap(SampleData.notifications.first)

        await model.openNotification(reference: notification.notificationRef)

        XCTAssertEqual(model.selectedTab, .alerts)
        XCTAssertEqual(model.presentedNotification?.notificationRef, notification.notificationRef)
        XCTAssertNil(model.focusedBriefingItemID)

        await model.openNotificationTarget(notification)

        XCTAssertEqual(model.selectedTab, .today)
        XCTAssertEqual(model.focusedBriefingItemID, "ios-direction")
        XCTAssertNil(model.presentedNotification)
    }

    @MainActor
    func testEntryTargetUsesPinnedVersionReferenceForExactRead() async throws {
        let model = AppModel()
        model.enterDemo()
        let notification = try XCTUnwrap(
            SampleData.notifications.first(where: { $0.target.type == .entry })
        )

        let item = try await model.readNotificationEntry(notification)

        XCTAssertEqual(item.reference, notification.source?.versionRef)
    }

    func testSafeMarkdownAllowsWebLinksAndRemovesOtherSchemes() {
        let value = SafeMarkdown.attributedString(
            "[web](https://brunn.ai) [unsafe](javascript:alert(1))"
        )
        let links = value.runs.compactMap(\.link)

        XCTAssertEqual(links.map(\.scheme), ["https"])
    }

    func testEntryMarkdownKeepsWebLinksAndRoutesRelativeAndWikiLinksInternally() throws {
        let value = SafeMarkdown.entryAttributedString(
            "[web](https://brunn.ai) [relative](../Other.md) [[Topics/Gaming/Gaming|Gaming]]"
        )
        let links = value.runs.compactMap(\.link)

        XCTAssertEqual(links.map(\.scheme), ["https", "brunn-entry", "brunn-entry"])
        let relative = try XCTUnwrap(EntryNavigationURL.link(from: links[1]))
        XCTAssertEqual(relative.target, "../Other.md")
        XCTAssertFalse(relative.isWikiLink)
        let wiki = try XCTUnwrap(EntryNavigationURL.link(from: links[2]))
        XCTAssertEqual(wiki.target, "Topics/Gaming/Gaming")
        XCTAssertEqual(wiki.label, "Gaming")
        XCTAssertTrue(wiki.isWikiLink)
    }

    func testEntryMarkdownDoesNotRewriteWikiSyntaxInsideCode() {
        let markdown = """
        `[[Inline literal]]`

        ```swift
        let example = "[[Fenced literal]]"
        ```

        ~~~text
        [[Tilde fenced literal]]
        ~~~

        > ```text
        > [[Blockquoted fenced literal]]
        > ```

        `[[Multiline
        code literal]]`

            [[Indented literal]]

        [[Real entry]]
        """

        let rewritten = EntryMarkdown.rewritingWikiLinks(in: markdown)

        XCTAssertTrue(rewritten.contains("`[[Inline literal]]`"))
        XCTAssertTrue(rewritten.contains("[[Fenced literal]]"))
        XCTAssertTrue(rewritten.contains("[[Tilde fenced literal]]"))
        XCTAssertTrue(rewritten.contains("[[Blockquoted fenced literal]]"))
        XCTAssertTrue(rewritten.contains("`[[Multiline\ncode literal]]`"))
        XCTAssertTrue(rewritten.contains("    [[Indented literal]]"))
        XCTAssertEqual(
            rewritten.components(separatedBy: "brunn-entry://open").count - 1,
            1
        )
        XCTAssertEqual(
            SafeMarkdown.entryAttributedString(markdown).runs.compactMap(\.link).count,
            1
        )
    }

    func testEntryMarkdownLeavesUnmatchedBackticksLiteralWithoutHidingLaterWikiLinks() {
        let markdown = """
        An unmatched ` delimiter stays literal.
        An unmatched `` delimiter also stays literal.
        [[Still linked]]
        """

        let rewritten = EntryMarkdown.rewritingWikiLinks(in: markdown)

        XCTAssertTrue(rewritten.contains("unmatched ` delimiter"))
        XCTAssertTrue(rewritten.contains("unmatched `` delimiter"))
        XCTAssertEqual(
            rewritten.components(separatedBy: "brunn-entry://open").count - 1,
            1
        )
        let rendered = SafeMarkdown.entryAttributedString(markdown)
        XCTAssertTrue(String(rendered.characters).contains("unmatched ` delimiter"))
        XCTAssertTrue(String(rendered.characters).contains("unmatched `` delimiter"))
        XCTAssertEqual(rendered.runs.compactMap(\.link).count, 1)
    }

    func testEntryMarkdownDoesNotRewriteWikiSyntaxInsideListNestedFences() {
        let markdown = """
        - ```markdown
          [[Bullet literal]]
          ```

        1. ~~~markdown
           [[Ordered literal]]
           ~~~

        + ```markdown
          [[Plus bullet literal]]
          ```

        2) ~~~markdown
           [[Parenthesized ordered literal]]
           ~~~

        -\t```markdown
        \t[[Tabbed bullet literal]]
        \t```

          - ```markdown
            [[Nested bullet literal]]
            ```

        > - ```markdown
        >   [[Blockquote bullet literal]]
        >   ```

        - > ~~~markdown
          > [[Bullet blockquote literal]]
          > ~~~

        [[Real entry]]
        """

        let rewritten = EntryMarkdown.rewritingWikiLinks(in: markdown)

        XCTAssertTrue(rewritten.contains("[[Bullet literal]]"))
        XCTAssertTrue(rewritten.contains("[[Ordered literal]]"))
        XCTAssertTrue(rewritten.contains("[[Plus bullet literal]]"))
        XCTAssertTrue(rewritten.contains("[[Parenthesized ordered literal]]"))
        XCTAssertTrue(rewritten.contains("[[Tabbed bullet literal]]"))
        XCTAssertTrue(rewritten.contains("[[Nested bullet literal]]"))
        XCTAssertTrue(rewritten.contains("[[Blockquote bullet literal]]"))
        XCTAssertTrue(rewritten.contains("[[Bullet blockquote literal]]"))
        XCTAssertEqual(
            rewritten.components(separatedBy: "brunn-entry://open").count - 1,
            1
        )
    }

    func testEntryMarkdownRestartsFenceWhenBlockquoteContainerEnds() {
        let markdown = """
        > ```markdown
        > [[Blockquote literal]]
        ```markdown
        [[Top-level literal]]
        ```
        [[Real entry]]
        """

        let rewritten = EntryMarkdown.rewritingWikiLinks(in: markdown)

        XCTAssertTrue(rewritten.contains("[[Blockquote literal]]"))
        XCTAssertTrue(rewritten.contains("[[Top-level literal]]"))
        XCTAssertEqual(
            rewritten.components(separatedBy: "brunn-entry://open").count - 1,
            1
        )
    }

    func testEntryMarkdownRestartsFenceWhenListContainerEnds() {
        let markdown = """
        - ```markdown
          [[List literal]]
        ```markdown
        [[Top-level literal]]
        ```
        [[Real entry]]
        """

        let rewritten = EntryMarkdown.rewritingWikiLinks(in: markdown)

        XCTAssertTrue(rewritten.contains("[[List literal]]"))
        XCTAssertTrue(rewritten.contains("[[Top-level literal]]"))
        XCTAssertEqual(
            rewritten.components(separatedBy: "brunn-entry://open").count - 1,
            1
        )
    }

    func testEntryMarkdownKeepsNestedContainerFenceMarkersInsideTopLevelFence() {
        let markdown = """
        ```markdown
        [[Top-level literal]]
        > ```markdown
        > [[Blockquote-looking literal]]
        > ```
        - ```markdown
        - [[List-looking literal]]
        - ```
        [[Still top-level literal]]
        ```
        [[Real entry]]
        """

        let rewritten = EntryMarkdown.rewritingWikiLinks(in: markdown)

        XCTAssertTrue(rewritten.contains("[[Blockquote-looking literal]]"))
        XCTAssertTrue(rewritten.contains("[[List-looking literal]]"))
        XCTAssertTrue(rewritten.contains("[[Still top-level literal]]"))
        XCTAssertEqual(
            rewritten.components(separatedBy: "brunn-entry://open").count - 1,
            1
        )
    }

    @MainActor
    func testDemoSearchEntryReadsPinnedMarkdownAndFollowsInternalLink() async throws {
        let model = AppModel()
        model.enterDemo()
        let first = try XCTUnwrap(SampleData.searchResults.first)

        let item = try await model.read(first)
        XCTAssertEqual(item.reference, first.reference)
        XCTAssertEqual(item.version, first.version)
        XCTAssertTrue(item.text?.contains("[[") == true)

        let attributed = SafeMarkdown.entryAttributedString(try XCTUnwrap(item.text))
        let internalURL = try XCTUnwrap(
            attributed.runs.compactMap(\.link).first(where: {
                $0.scheme == "brunn-entry"
            })
        )
        let link = try XCTUnwrap(EntryNavigationURL.link(from: internalURL))
        let linked = try await model.read(WorkspaceEntryRequest(link: link, sourcePath: item.path))

        XCTAssertNotEqual(linked.reference, item.reference)
        XCTAssertEqual(linked.title, SampleData.searchResults.dropFirst().first?.title)
    }

    @MainActor
    func testShortSearchClearsPreviouslyDisplayedResults() async {
        let model = AppModel()
        model.enterDemo()

        await model.performSearch("brunn")
        XCTAssertFalse(model.searchResults.isEmpty)

        await model.performSearch("x")
        XCTAssertTrue(model.searchResults.isEmpty)
        XCTAssertEqual(model.searchMessage, "Enter at least two characters.")
    }

    @MainActor
    func testDemoFixtureCoversTheCompleteReaderAndNewsDeliveryStates() throws {
        let model = AppModel()
        model.enterDemo()

        let briefing = try XCTUnwrap(model.latestBriefing)
        let payload = try XCTUnwrap(briefing.briefing)
        let sections = try XCTUnwrap(payload.sections)
        let items = sections.flatMap(\.items)

        XCTAssertEqual(payload.summaryMD?.count, 7)
        XCTAssertEqual(sections.map(\.topic), ["brunn", "platform", "reading-experience"])
        XCTAssertEqual(
            items.map(\.id),
            ["ios-direction", "existing-contracts", "delivery-correction", "full-width-reader"]
        )
        XCTAssertTrue(items.allSatisfy { !($0.story?.urls ?? []).isEmpty })
        XCTAssertEqual(model.newsItems.count, items.count)
        XCTAssertTrue(model.newsItems.contains { $0.kind == .new })
        XCTAssertTrue(model.newsItems.contains { $0.kind == .update })
        XCTAssertTrue(model.newsItems.contains { $0.kind == .correction })
        XCTAssertTrue(model.newsItems.allSatisfy(\.isPriority))
        XCTAssertEqual(model.briefingHistory.map(\.date), ["2026-08-02", "2026-08-01", "2026-08-01"])
        XCTAssertEqual(model.briefingHistory.map(\.edition), ["morning", "evening", "morning"])
        XCTAssertEqual(model.topicsSnapshot?.topics.count, 2)
        XCTAssertEqual(model.topicsSnapshot?.pendingRequests.count, 1)
        XCTAssertTrue(model.isNewsItemRead("existing-contracts"))
        XCTAssertFalse(model.isNewsItemRead("ios-direction"))
    }

    @MainActor
    func testMarkingNewsReadChangesOnlySessionPresentationState() {
        let model = AppModel()
        model.enterDemo()
        let before = model.newsItems

        model.markNewsItemRead("ios-direction")

        XCTAssertTrue(model.isNewsItemRead("ios-direction"))
        XCTAssertEqual(model.newsItems, before)
        XCTAssertEqual(model.latestBriefing, SampleData.briefing)
    }
}

private final class TestLocationVisit: CLVisit {
    private let testCoordinate: CLLocationCoordinate2D
    private let testHorizontalAccuracy: CLLocationAccuracy
    private let testArrivalDate: Date
    private let testDepartureDate: Date

    override var coordinate: CLLocationCoordinate2D { testCoordinate }
    override var horizontalAccuracy: CLLocationAccuracy { testHorizontalAccuracy }
    override var arrivalDate: Date { testArrivalDate }
    override var departureDate: Date { testDepartureDate }

    init(
        coordinate: CLLocationCoordinate2D,
        horizontalAccuracy: CLLocationAccuracy,
        arrivalDate: Date,
        departureDate: Date
    ) {
        testCoordinate = coordinate
        testHorizontalAccuracy = horizontalAccuracy
        testArrivalDate = arrivalDate
        testDepartureDate = departureDate
        super.init()
    }

    required init?(coder _: NSCoder) {
        fatalError("TestLocationVisit does not support decoding")
    }
}

@MainActor
private final class TestCredentialStore: CredentialStoring {
    private var credential: DeviceTaskCredential?
    private(set) var wasDeleted = false
    private let loadError: TestCredentialStoreError?
    private let saveError: TestCredentialStoreError?
    private let deleteError: TestCredentialStoreError?

    init(
        token: String?,
        loadError: TestCredentialStoreError? = nil,
        saveError: TestCredentialStoreError? = nil,
        deleteError: TestCredentialStoreError? = nil
    ) {
        credential = token.map {
            DeviceTaskCredential(
                credentialRef: "credential:11111111-1111-1111-1111-111111111111",
                token: $0
            )
        }
        self.loadError = loadError
        self.saveError = saveError
        self.deleteError = deleteError
    }

    init(
        credential: DeviceTaskCredential,
        loadError: TestCredentialStoreError? = nil,
        saveError: TestCredentialStoreError? = nil,
        deleteError: TestCredentialStoreError? = nil
    ) {
        self.credential = credential
        self.loadError = loadError
        self.saveError = saveError
        self.deleteError = deleteError
    }

    func load() throws -> DeviceTaskCredential? {
        if let loadError { throw loadError }
        return credential
    }

    func save(_ credential: DeviceTaskCredential) throws {
        if let saveError { throw saveError }
        self.credential = credential
    }

    func delete() throws {
        if let deleteError { throw deleteError }
        wasDeleted = true
        credential = nil
    }
}

private enum TestCredentialStoreError: Error {
    case forced
}

private actor CallCounter {
    private(set) var value = 0

    func increment() {
        value += 1
    }
}

private actor NotificationTrace {
    private(set) var values: [String] = []

    func append(_ value: String) {
        values.append(value)
    }
}

private final class NotificationHandoffRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var events: [String] = []

    func append(_ event: String) {
        lock.lock()
        events.append(event)
        lock.unlock()
    }

    func snapshot() -> [String] {
        lock.lock()
        defer { lock.unlock() }
        return events
    }
}

private struct StubbedHTTPResponse: Sendable {
    let statusCode: Int
    let json: String
    let headers: [String: String]

    init(
        statusCode: Int = 200,
        json: String,
        headers: [String: String] = [:]
    ) {
        self.statusCode = statusCode
        self.json = json
        self.headers = headers
    }
}

private final class NotificationRequestRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var requests: [URLRequest] = []

    func append(_ request: URLRequest) {
        lock.lock()
        requests.append(request)
        lock.unlock()
    }

    func snapshot() -> [URLRequest] {
        lock.lock()
        defer { lock.unlock() }
        return requests
    }
}

private final class SynchronousRequestGate: @unchecked Sendable {
    private let entered = DispatchSemaphore(value: 0)
    private let releaseRequest = DispatchSemaphore(value: 0)

    func enterAndWait() {
        entered.signal()
        _ = releaseRequest.wait(timeout: .now() + 5)
    }

    func waitUntilEntered() -> Bool {
        entered.wait(timeout: .now() + 5) == .success
    }

    func release() {
        releaseRequest.signal()
    }
}

private final class NotificationRequestURLProtocol: URLProtocol, @unchecked Sendable {
    nonisolated(unsafe) static var handler: (@Sendable (URLRequest) -> StubbedHTTPResponse)?

    override class func canInit(with _: URLRequest) -> Bool { true }

    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        guard let handler = Self.handler, let url = request.url else {
            client?.urlProtocol(self, didFailWithError: URLError(.badServerResponse))
            return
        }
        var capturedRequest = request
        if capturedRequest.httpBody == nil,
           let bodyStream = capturedRequest.httpBodyStream
        {
            bodyStream.open()
            defer { bodyStream.close() }
            var body = Data()
            let buffer = UnsafeMutablePointer<UInt8>.allocate(capacity: 4096)
            defer { buffer.deallocate() }
            while bodyStream.hasBytesAvailable {
                let count = bodyStream.read(buffer, maxLength: 4096)
                guard count > 0 else { break }
                body.append(buffer, count: count)
            }
            capturedRequest.httpBody = body
        }
        let stub = handler(capturedRequest)
        guard let response = HTTPURLResponse(
            url: url,
            statusCode: stub.statusCode,
            httpVersion: "HTTP/1.1",
            headerFields: ["Content-Type": "application/json"].merging(
                stub.headers,
                uniquingKeysWith: { _, replacement in replacement }
            )
        ) else {
            client?.urlProtocol(self, didFailWithError: URLError(.badServerResponse))
            return
        }
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: Data(stub.json.utf8))
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}
}

private actor IdentityGate {
    private var continuation: CheckedContinuation<MeData, Never>?

    func load() async -> MeData {
        await withCheckedContinuation { continuation in
            self.continuation = continuation
        }
    }

    func resolve(with identity: MeData) {
        continuation?.resume(returning: identity)
        continuation = nil
    }
}

private actor DashboardGate {
    private var continuation: CheckedContinuation<WorkspaceDashboardData, any Error>?
    private(set) var hasStarted = false

    func load() async throws -> WorkspaceDashboardData {
        hasStarted = true
        return try await withCheckedThrowingContinuation { continuation in
            self.continuation = continuation
        }
    }

    func resolve(with dashboard: WorkspaceDashboardData) {
        continuation?.resume(returning: dashboard)
        continuation = nil
    }
}
