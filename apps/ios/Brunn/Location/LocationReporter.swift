import Combine
import CoreLocation
import Foundation
import UIKit

@MainActor
final class LocationReporter: NSObject, ObservableObject, @preconcurrency CLLocationManagerDelegate {
    @Published private(set) var authorizationStatus: CLAuthorizationStatus
    @Published private(set) var reportingEnabled: Bool
    @Published private(set) var hasCredential = false
    @Published private(set) var queuedReportCount = 0
    @Published private(set) var lastUploadAt: Date?
    @Published private(set) var presence: LocationPresence?
    @Published private(set) var lastError: String?
    @Published private(set) var isWorking = false
    @Published private(set) var wasRelaunchedForLocation = false

    private let manager: CLLocationManager
    private let api: BrunnAPI
    private let credentialStore: LocationKeychainCredentialStore
    private let queue: LocationDiskQueue
    private let statusStore: LocationStatusStore
    private let enricher: LocationReportEnricher
    private var pendingEnable = false
    private var deliveryTail: Task<Void, Never>?
    private var activeUploadTask: Task<LocationReportUploadResponse, Error>?
    private var isFlushing = false

    override convenience init() {
        let statusStore = LocationStatusStore()
        self.init(
            manager: CLLocationManager(),
            api: BrunnAPI(),
            credentialStore: LocationKeychainCredentialStore(),
            queue: LocationDiskQueue(),
            statusStore: statusStore,
            enricher: LocationReportEnricher(statusStore: statusStore)
        )
    }

    init(
        manager: CLLocationManager,
        api: BrunnAPI,
        credentialStore: LocationKeychainCredentialStore,
        queue: LocationDiskQueue,
        statusStore: LocationStatusStore,
        enricher: LocationReportEnricher
    ) {
        self.manager = manager
        self.api = api
        self.credentialStore = credentialStore
        self.queue = queue
        self.statusStore = statusStore
        self.enricher = enricher
        authorizationStatus = manager.authorizationStatus
        reportingEnabled = statusStore.reportingEnabled
        lastUploadAt = statusStore.lastUploadAt
        super.init()
        manager.delegate = self
        hasCredential = (try? credentialStore.load()) != nil
        queuedReportCount = (try? queue.count()) ?? 0
    }

    func applicationDidFinishLaunching(relaunchedForLocation: Bool) {
        wasRelaunchedForLocation = relaunchedForLocation
        authorizationStatus = manager.authorizationStatus
        if reportingEnabled {
            startMonitoring()
        }
    }

    func applicationDidBecomeActive(expectedUserID: String?) async {
        authorizationStatus = manager.authorizationStatus
        await validateStoredCredential(expectedUserID: expectedUserID)
        if reportingEnabled {
            startMonitoring()
            await drainQueue()
            await refreshPresence()
        }
    }

    func beginEnable(ownerAPI: BrunnAPI, expectedUserID: String) async {
        guard !expectedUserID.isEmpty else {
            lastError = "Connect this iPhone to Brunn before setting up location."
            return
        }
        isWorking = true
        defer { isWorking = false }
        do {
            try await ensureCredential(ownerAPI: ownerAPI, expectedUserID: expectedUserID)
            pendingEnable = true
            requestNextAuthorizationStep()
        } catch {
            pendingEnable = false
            lastError = error.localizedDescription
        }
    }

    func disableReporting() async {
        pendingEnable = false
        reportingEnabled = false
        statusStore.reportingEnabled = false
        stopMonitoring()
        let pendingDelivery = deliveryTail
        deliveryTail = nil
        pendingDelivery?.cancel()
        activeUploadTask?.cancel()
        if let activeUploadTask {
            _ = try? await activeUploadTask.value
        }
        do {
            try queue.clear()
            queuedReportCount = 0
        } catch {
            lastError = error.localizedDescription
        }
        guard let credential = try? credentialStore.load() else {
            statusStore.clearLiveStatus()
            presence = nil
            lastUploadAt = nil
            return
        }
        isWorking = true
        defer { isWorking = false }
        do {
            try await api.deleteLiveLocation(bearerToken: credential.token)
            statusStore.clearLiveStatus()
            presence = nil
            lastUploadAt = nil
            lastError = nil
        } catch {
            lastError = "Reporting stopped, but Brunn could not delete live location: \(error.localizedDescription)"
        }
    }

    func deleteLiveData() async {
        guard let credential = try? credentialStore.load() else {
            statusStore.clearLiveStatus()
            presence = nil
            lastUploadAt = nil
            return
        }
        isWorking = true
        defer { isWorking = false }
        do {
            try await api.deleteLiveLocation(bearerToken: credential.token)
            statusStore.clearLiveStatus()
            presence = nil
            lastUploadAt = nil
            lastError = nil
        } catch {
            lastError = error.localizedDescription
        }
    }

    func refreshPresence() async {
        guard let credential = validStoredCredential() else { return }
        do {
            let current = try await api.locationPresence(bearerToken: credential.token)
            presence = current
            lastError = nil
        } catch let error as BrunnAPIError {
            if case let .server(status, code, _) = error,
               status == 404,
               code == "location_presence_not_found"
            {
                presence = nil
                return
            }
            handleCredentialFailureIfNeeded(error)
            lastError = error.localizedDescription
        } catch {
            lastError = error.localizedDescription
        }
    }

    func handle(visit: CLVisit) {
        guard reportingEnabled,
              CLLocationCoordinate2DIsValid(visit.coordinate),
              visit.horizontalAccuracy >= 0
        else { return }

        let report: LocationReport
        if visit.departureDate == .distantFuture {
            let arrivedAt = visit.arrivalDate == .distantPast
                ? nil
                : LocationTimestamp.string(from: visit.arrivalDate)
            report = LocationReport(
                type: .visitArrival,
                at: LocationTimestamp.string(from: Date()),
                lat: visit.coordinate.latitude,
                lon: visit.coordinate.longitude,
                accuracyM: visit.horizontalAccuracy,
                arrivedAt: arrivedAt
            )
        } else {
            guard visit.arrivalDate != .distantPast else { return }
            report = LocationReport(
                type: .visitDeparture,
                at: LocationTimestamp.string(from: Date()),
                lat: visit.coordinate.latitude,
                lon: visit.coordinate.longitude,
                accuracyM: visit.horizontalAccuracy,
                arrivedAt: LocationTimestamp.string(from: visit.arrivalDate),
                departedAt: LocationTimestamp.string(from: visit.departureDate)
            )
        }
        enqueue(report)
    }

    func handle(location: CLLocation) {
        guard reportingEnabled,
              CLLocationCoordinate2DIsValid(location.coordinate),
              location.horizontalAccuracy >= 0
        else { return }
        enqueue(LocationReport(
            type: .ping,
            at: LocationTimestamp.string(from: location.timestamp),
            lat: location.coordinate.latitude,
            lon: location.coordinate.longitude,
            accuracyM: location.horizontalAccuracy
        ))
    }

    func locationManagerDidChangeAuthorization(_ manager: CLLocationManager) {
        authorizationStatus = manager.authorizationStatus
        guard pendingEnable else { return }
        switch manager.authorizationStatus {
        case .authorizedWhenInUse:
            manager.requestAlwaysAuthorization()
        case .authorizedAlways:
            finishEnable()
        case .denied, .restricted:
            pendingEnable = false
            lastError = "Always Location Access is required to report visits in the background."
        case .notDetermined:
            break
        @unknown default:
            pendingEnable = false
        }
    }

    func locationManager(_: CLLocationManager, didVisit visit: CLVisit) {
        handle(visit: visit)
    }

    func locationManager(_: CLLocationManager, didUpdateLocations locations: [CLLocation]) {
        guard let location = locations.last else { return }
        handle(location: location)
    }

    func locationManager(_: CLLocationManager, didFailWithError error: Error) {
        lastError = error.localizedDescription
    }

    private func requestNextAuthorizationStep() {
        authorizationStatus = manager.authorizationStatus
        switch manager.authorizationStatus {
        case .notDetermined:
            manager.requestWhenInUseAuthorization()
        case .authorizedWhenInUse:
            manager.requestAlwaysAuthorization()
        case .authorizedAlways:
            finishEnable()
        case .denied, .restricted:
            pendingEnable = false
            lastError = "Location permission is off. Open Settings to allow Always access."
        @unknown default:
            pendingEnable = false
        }
    }

    private func finishEnable() {
        guard validStoredCredential() != nil else {
            pendingEnable = false
            lastError = "The protected location credential is unavailable."
            return
        }
        pendingEnable = false
        reportingEnabled = true
        statusStore.reportingEnabled = true
        lastError = nil
        startMonitoring()
        Task { await drainQueue() }
    }

    private func startMonitoring() {
        manager.startMonitoringVisits()
        manager.startMonitoringSignificantLocationChanges()
    }

    private func stopMonitoring() {
        manager.stopMonitoringVisits()
        manager.stopMonitoringSignificantLocationChanges()
    }

    private func enqueue(_ report: LocationReport) {
        let lease = LocationBackgroundTaskLease()
        lease.begin()
        do {
            _ = try queue.append(report)
            queuedReportCount = (try? queue.count()) ?? queuedReportCount
        } catch {
            lease.end()
            lastError = error.localizedDescription
            return
        }
        let previous = deliveryTail
        deliveryTail = Task { [weak self] in
            _ = await previous?.value
            guard let self else {
                lease.end()
                return
            }
            await self.drainQueue()
            lease.end()
        }
    }

    private func drainQueue() async {
        guard !isFlushing, reportingEnabled,
              let credential = validStoredCredential()
        else { return }
        isFlushing = true
        defer {
            activeUploadTask = nil
            isFlushing = false
        }

        while reportingEnabled, !Task.isCancelled {
            let batch: [LocationQueuedReport]
            let pending: LocationQueuedReport?
            do {
                batch = try queue.batch()
                pending = batch.count < LocationDiskQueue.maximumBatchCount
                    ? try queue.nextPending()
                    : nil
            } catch {
                lastError = error.localizedDescription
                return
            }

            if let pending {
                let enriched = await enricher.enrich(pending.report)
                guard reportingEnabled, !Task.isCancelled else { return }
                do {
                    try queue.replace(id: pending.id, with: enriched)
                } catch {
                    lastError = error.localizedDescription
                    return
                }
                continue
            }

            guard !batch.isEmpty else {
                queuedReportCount = (try? queue.count()) ?? queuedReportCount
                return
            }

            let uploadTask = Task {
                try await api.uploadLocationReports(
                    LocationReportBatchRequest(
                        timezone: TimeZone.current.identifier,
                        reports: batch.map(\.report)
                    ),
                    bearerToken: credential.token
                )
            }
            activeUploadTask = uploadTask
            do {
                let response = try await uploadTask.value
                activeUploadTask = nil
                guard reportingEnabled, !Task.isCancelled else { return }
                try queue.remove(ids: batch.map(\.id))
                queuedReportCount = try queue.count()
                let uploadedAt = Date()
                lastUploadAt = uploadedAt
                statusStore.lastUploadAt = uploadedAt
                if let current = response.presence {
                    presence = current
                }
                lastError = nil
            } catch let error as BrunnAPIError {
                activeUploadTask = nil
                guard reportingEnabled, !Task.isCancelled else { return }
                handleCredentialFailureIfNeeded(error)
                lastError = error.localizedDescription
                return
            } catch {
                activeUploadTask = nil
                guard reportingEnabled, !Task.isCancelled else { return }
                lastError = error.localizedDescription
                return
            }
        }
    }

    private func ensureCredential(ownerAPI: BrunnAPI, expectedUserID: String) async throws {
        if let stored = validStoredCredential() {
            do {
                let identity = try await api.deviceCredentialIdentity(bearerToken: stored.token)
                if identity.user.id == expectedUserID,
                   identity.credentialID == stored.credentialRef,
                   LocationCredentialCapabilities.isExactAcceptedSet(identity.capabilities),
                   Set(identity.capabilities) == Set(stored.capabilities)
                {
                    hasCredential = true
                    return
                }
                _ = try? await ownerAPI.revokeCredential(reference: stored.credentialRef)
                try credentialStore.delete()
                hasCredential = false
            } catch let error as BrunnAPIError where error.isUnauthorized {
                try credentialStore.delete()
                hasCredential = false
            } catch {
                throw error
            }
        }

        let issued = try await ownerAPI.bootstrapDeviceLocationCredential()
        guard issued.access == "ios_location",
              issued.id.hasPrefix("credential:"),
              !issued.token.isEmpty,
              LocationCredentialCapabilities.isExactAcceptedSet(issued.capabilities)
        else {
            if !issued.id.isEmpty {
                _ = try? await ownerAPI.revokeCredential(reference: issued.id)
            }
            throw LocationReporterError.invalidCredential
        }
        do {
            let identity = try await api.deviceCredentialIdentity(bearerToken: issued.token)
            guard identity.user.id == expectedUserID,
                  identity.credentialID == issued.id,
                  LocationCredentialCapabilities.isExactAcceptedSet(identity.capabilities),
                  Set(identity.capabilities) == Set(issued.capabilities)
            else {
                throw LocationReporterError.invalidCredential
            }
            try credentialStore.save(LocationDeviceCredential(
                credentialRef: issued.id,
                token: issued.token,
                userID: identity.user.id,
                capabilities: issued.capabilities
            ))
            hasCredential = true
            lastError = nil
        } catch {
            _ = try? await ownerAPI.revokeCredential(reference: issued.id)
            throw error
        }
    }

    private func validateStoredCredential(expectedUserID: String?) async {
        guard let stored = validStoredCredential() else { return }
        do {
            let identity = try await api.deviceCredentialIdentity(bearerToken: stored.token)
            guard expectedUserID == nil || identity.user.id == expectedUserID,
                  identity.user.id == stored.userID,
                  identity.credentialID == stored.credentialRef,
                  LocationCredentialCapabilities.isExactAcceptedSet(identity.capabilities),
                  Set(identity.capabilities) == Set(stored.capabilities)
            else {
                invalidateCredential()
                return
            }
            hasCredential = true
        } catch let error as BrunnAPIError {
            handleCredentialFailureIfNeeded(error)
        } catch {
            // Transient network failures do not discard a valid local bearer.
        }
    }

    private func validStoredCredential() -> LocationDeviceCredential? {
        do {
            guard let credential = try credentialStore.load(),
                  LocationCredentialCapabilities.isExactAcceptedSet(credential.capabilities)
            else {
                hasCredential = false
                return nil
            }
            hasCredential = true
            return credential
        } catch {
            hasCredential = false
            return nil
        }
    }

    private func handleCredentialFailureIfNeeded(_ error: BrunnAPIError) {
        if error.isUnauthorized {
            invalidateCredential()
        }
    }

    private func invalidateCredential() {
        try? credentialStore.delete()
        hasCredential = false
        reportingEnabled = false
        statusStore.reportingEnabled = false
        stopMonitoring()
    }
}

private enum LocationReporterError: Error, LocalizedError {
    case invalidCredential

    var errorDescription: String? {
        "Brunn issued a location credential outside the exact approved capability set."
    }
}

@MainActor
private final class LocationBackgroundTaskLease {
    private var identifier: UIBackgroundTaskIdentifier = .invalid

    func begin() {
        guard identifier == .invalid else { return }
        identifier = UIApplication.shared.beginBackgroundTask(
            withName: "Brunn location delivery"
        ) { [weak self] in
            Task { @MainActor in self?.end() }
        }
    }

    func end() {
        guard identifier != .invalid else { return }
        let current = identifier
        identifier = .invalid
        UIApplication.shared.endBackgroundTask(current)
    }
}
