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
    @Published private(set) var setupPending: Bool
    @Published private(set) var validatedCredentialUserID: String?

    private let manager: CLLocationManager
    private let api: BrunnAPI
    private let credentialStore: LocationKeychainCredentialStore
    private let queue: LocationDiskQueue
    private let statusStore: LocationStatusStore
    private let enricher: LocationReportEnricher
    private var pendingEnable: Bool
    private var deliveryTail: Task<Void, Never>?
    private var activeUploadTask: Task<LocationReportUploadResponse, Error>?
    private var isFlushing = false
    private var credentialOperationHeld = false
    private var credentialOperationWaiters: [CheckedContinuation<Void, Never>] = []

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
        pendingEnable = statusStore.setupPending
        setupPending = statusStore.setupPending
        validatedCredentialUserID = nil
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
        await acquireCredentialOperation()
        await validateStoredCredential(expectedUserID: expectedUserID)
        releaseCredentialOperation()
        if pendingEnable {
            requestNextAuthorizationStep()
        }
        if reportingEnabled {
            startMonitoring()
            await drainQueue()
            await refreshPresence()
        }
    }

    @discardableResult
    func beginEnable(
        ownerAPI: BrunnAPI,
        expectedUserID: String,
        onReadyToRequest: () -> Void = {}
    ) async -> Bool {
        guard !isWorking else { return false }
        isWorking = true
        await acquireCredentialOperation()
        defer {
            releaseCredentialOperation()
            isWorking = false
        }
        guard !expectedUserID.isEmpty else {
            lastError = "Connect this iPhone to Brunn before setting up location."
            return false
        }
        do {
            try await ensureCredential(ownerAPI: ownerAPI, expectedUserID: expectedUserID)
            setPendingEnable(true)
            onReadyToRequest()
            await Task.yield()
            requestNextAuthorizationStep()
            return true
        } catch {
            setPendingEnable(false)
            lastError = error.localizedDescription
            return false
        }
    }

    @discardableResult
    func prepareEnableFromSettings(ownerAPI: BrunnAPI, expectedUserID: String) async -> Bool {
        guard !isWorking else { return false }
        isWorking = true
        await acquireCredentialOperation()
        defer {
            releaseCredentialOperation()
            isWorking = false
        }
        guard !expectedUserID.isEmpty else {
            lastError = "Connect this iPhone to Brunn before setting up location."
            return false
        }
        do {
            try await ensureCredential(ownerAPI: ownerAPI, expectedUserID: expectedUserID)
            setPendingEnable(true)
            lastError = nil
            return true
        } catch {
            setPendingEnable(false)
            lastError = error.localizedDescription
            return false
        }
    }

    func disableReporting() async {
        guard !isWorking else { return }
        isWorking = true
        await acquireCredentialOperation()
        defer {
            releaseCredentialOperation()
            isWorking = false
        }
        do {
            try await stopReportingLocally()
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

    func disconnectFromAccount(ownerAPI: BrunnAPI) async -> Bool {
        guard !isWorking else { return false }
        isWorking = true
        await acquireCredentialOperation()
        defer {
            releaseCredentialOperation()
            isWorking = false
        }
        do {
            try await stopReportingLocally()
        } catch {
            lastError = "Disconnect stopped because queued location data could not be removed. \(error.localizedDescription)"
            return false
        }

        let credential: LocationDeviceCredential?
        do {
            credential = try credentialStore.load()
        } catch {
            hasCredential = true
            lastError = "Disconnect stopped because the protected location credential could not be read. Unlock this iPhone and retry."
            return false
        }

        guard let credential else {
            finishLocationDisconnect()
            return true
        }

        var credentialAlreadyInvalid = false
        do {
            try await api.deleteLiveLocation(bearerToken: credential.token)
        } catch let error as BrunnAPIError where error.isUnauthorized {
            // A previous disconnect attempt may already have revoked this token.
            // It is safe to finish removing the now-unusable local credential.
            credentialAlreadyInvalid = true
        } catch {
            lastError = "Disconnect stopped because live location data could not be deleted from Brunn. Retry while online."
            return false
        }

        if !credentialAlreadyInvalid {
            do {
                _ = try await ownerAPI.revokeCredential(reference: credential.credentialRef)
            } catch {
                lastError = "Disconnect stopped because iPhone location access could not be revoked on Brunn. Retry while online."
                return false
            }
        }

        do {
            if let current = try credentialStore.load(), current != credential {
                lastError = "Disconnect stopped because iPhone location access changed during cleanup. Retry."
                return false
            }
            try credentialStore.delete()
        } catch {
            hasCredential = true
            lastError = "Disconnect stopped because the revoked location credential could not be removed from this iPhone. Unlock it and retry."
            return false
        }
        finishLocationDisconnect()
        return true
    }

    func deleteLiveData() async {
        guard !isWorking else { return }
        isWorking = true
        await acquireCredentialOperation()
        defer {
            releaseCredentialOperation()
            isWorking = false
        }
        guard let credential = try? credentialStore.load() else {
            statusStore.clearLiveStatus()
            presence = nil
            lastUploadAt = nil
            return
        }
        do {
            try await api.deleteLiveLocation(bearerToken: credential.token)
            guard credentialIsCurrent(credential) else { return }
            statusStore.clearLiveStatus()
            presence = nil
            lastUploadAt = nil
            lastError = nil
        } catch {
            lastError = error.localizedDescription
        }
    }

    func refreshPresence() async {
        guard !isWorking else { return }
        isWorking = true
        defer { isWorking = false }
        guard let credential = validStoredCredential() else { return }
        do {
            let current = try await api.locationPresence(bearerToken: credential.token)
            guard credentialIsCurrent(credential) else { return }
            presence = current
            lastError = nil
        } catch let error as BrunnAPIError {
            guard credentialIsCurrent(credential) else { return }
            if case let .server(status, code, _) = error,
               status == 404,
               code == "location_presence_not_found"
            {
                presence = nil
                return
            }
            if !handleCredentialFailureIfNeeded(error, credential: credential) {
                lastError = error.localizedDescription
            }
        } catch {
            if credentialIsCurrent(credential) {
                lastError = error.localizedDescription
            }
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

    /// A significant-change location older than this at capture is a cached
    /// fix, not a movement, and is dropped. Visit reports are unaffected.
    static let maximumSignificantChangeAge: TimeInterval = 15 * 60

    func handle(location: CLLocation, now: Date = Date()) {
        guard reportingEnabled,
              CLLocationCoordinate2DIsValid(location.coordinate),
              location.horizontalAccuracy >= 0,
              now.timeIntervalSince(location.timestamp) <= Self.maximumSignificantChangeAge
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
            setPendingEnable(false)
            lastError = "Always Location Access is required to report visits in the background."
        case .notDetermined:
            break
        @unknown default:
            setPendingEnable(false)
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
            setPendingEnable(false)
            lastError = "Location permission is off. Open Settings to allow Always access."
        @unknown default:
            setPendingEnable(false)
        }
    }

    private func finishEnable() {
        guard validStoredCredential() != nil else {
            setPendingEnable(false)
            lastError = "The protected location credential is unavailable."
            return
        }
        setPendingEnable(false)
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

    private func setPendingEnable(_ pending: Bool) {
        pendingEnable = pending
        setupPending = pending
        statusStore.setupPending = pending
    }

    private func stopReportingLocally() async throws {
        setPendingEnable(false)
        reportingEnabled = false
        statusStore.reportingEnabled = false
        stopMonitoring()
        let pendingDelivery = deliveryTail
        deliveryTail = nil
        pendingDelivery?.cancel()
        let uploadTask = activeUploadTask
        uploadTask?.cancel()
        if let uploadTask {
            _ = try? await uploadTask.value
        }
        _ = await pendingDelivery?.value
        try queue.clear()
        queuedReportCount = 0
    }

    private func finishLocationDisconnect() {
        hasCredential = false
        validatedCredentialUserID = nil
        statusStore.clearForDisconnect()
        presence = nil
        lastUploadAt = nil
        lastError = nil
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

        while reportingEnabled, !Task.isCancelled, credentialIsCurrent(credential) {
            let batch: [LocationQueuedReport]
            let pending: LocationQueuedReport?
            do {
                batch = try queue.batch()
                pending = batch.count < LocationDiskQueue.maximumBatchCount
                    ? try queue.nextPending()
                    : nil
            } catch {
                if credentialIsCurrent(credential) {
                    lastError = error.localizedDescription
                }
                return
            }

            if let pending {
                let enriched = await enricher.enrich(pending.report)
                guard reportingEnabled, !Task.isCancelled,
                      credentialIsCurrent(credential)
                else { return }
                do {
                    try queue.replace(id: pending.id, with: enriched)
                } catch {
                    if credentialIsCurrent(credential) {
                        lastError = error.localizedDescription
                    }
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
                guard reportingEnabled, !Task.isCancelled,
                      credentialIsCurrent(credential)
                else { return }
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
                guard reportingEnabled, !Task.isCancelled,
                      credentialIsCurrent(credential)
                else { return }
                if !handleCredentialFailureIfNeeded(error, credential: credential) {
                    lastError = error.localizedDescription
                }
                return
            } catch {
                activeUploadTask = nil
                guard reportingEnabled, !Task.isCancelled,
                      credentialIsCurrent(credential)
                else { return }
                lastError = error.localizedDescription
                return
            }
        }
    }

    private func ensureCredential(ownerAPI: BrunnAPI, expectedUserID: String) async throws {
        if let stored = validStoredCredential() {
            if stored.userID != expectedUserID {
                try await clearPreviousAccountCredential(stored)
            } else {
                do {
                    let identity = try await api.deviceCredentialIdentity(
                        bearerToken: stored.token
                    )
                    if identity.user.id == expectedUserID,
                       identity.credentialID == stored.credentialRef,
                       LocationCredentialCapabilities.isExactAcceptedSet(identity.capabilities),
                       Set(identity.capabilities) == Set(stored.capabilities)
                    {
                        hasCredential = true
                        validatedCredentialUserID = identity.user.id
                        return
                    }
                    _ = try? await ownerAPI.revokeCredential(reference: stored.credentialRef)
                    guard invalidateCredential() else {
                        throw LocationReporterError.credentialCleanupFailed
                    }
                } catch let error as BrunnAPIError where error.isUnauthorized {
                    guard invalidateCredential() else {
                        throw LocationReporterError.credentialCleanupFailed
                    }
                } catch {
                    throw error
                }
            }
        }

        guard invalidateCredential() else {
            throw LocationReporterError.credentialCleanupFailed
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
            validatedCredentialUserID = identity.user.id
            lastError = nil
        } catch {
            _ = try? await ownerAPI.revokeCredential(reference: issued.id)
            throw error
        }
    }

    private func validateStoredCredential(expectedUserID: String?) async {
        guard let stored = validStoredCredential() else {
            if reportingEnabled {
                try? await stopReportingLocally()
                statusStore.clearForDisconnect()
                presence = nil
                lastUploadAt = nil
                invalidateCredential()
            }
            return
        }
        if let expectedUserID, stored.userID != expectedUserID {
            do {
                try await clearPreviousAccountCredential(stored)
            } catch {
                lastError = error.localizedDescription
            }
            return
        }
        do {
            let identity = try await api.deviceCredentialIdentity(bearerToken: stored.token)
            guard expectedUserID == nil || identity.user.id == expectedUserID,
                  identity.user.id == stored.userID,
                  identity.credentialID == stored.credentialRef,
                  LocationCredentialCapabilities.isExactAcceptedSet(identity.capabilities),
                  Set(identity.capabilities) == Set(stored.capabilities)
            else {
                if let expectedUserID,
                   identity.user.id != expectedUserID || identity.user.id != stored.userID
                {
                    try await clearPreviousAccountCredential(stored)
                    return
                }
                invalidateCredential()
                return
            }
            hasCredential = true
            validatedCredentialUserID = identity.user.id
        } catch let error as BrunnAPIError {
            _ = handleCredentialFailureIfNeeded(error, credential: stored)
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

    private func credentialIsCurrent(_ credential: LocationDeviceCredential) -> Bool {
        do {
            return try credentialStore.load() == credential
        } catch {
            return false
        }
    }

    @discardableResult
    private func handleCredentialFailureIfNeeded(
        _ error: BrunnAPIError,
        credential: LocationDeviceCredential
    ) -> Bool {
        guard error.isUnauthorized else { return false }
        if let current = try? credentialStore.load(), current != credential {
            return true
        }
        if invalidateCredential() {
            lastError = error.localizedDescription
        }
        return true
    }

    @discardableResult
    private func invalidateCredential() -> Bool {
        validatedCredentialUserID = nil
        setPendingEnable(false)
        reportingEnabled = false
        statusStore.reportingEnabled = false
        stopMonitoring()
        deliveryTail?.cancel()
        deliveryTail = nil
        activeUploadTask?.cancel()
        activeUploadTask = nil
        do {
            try queue.clear()
            queuedReportCount = 0
            try credentialStore.delete()
        } catch {
            hasCredential = (try? credentialStore.load()) != nil
            lastError = "Location access was disabled, but protected queued data could not be cleared. Unlock this iPhone and retry before reconnecting location."
            return false
        }
        hasCredential = false
        statusStore.clearForDisconnect()
        presence = nil
        lastUploadAt = nil
        return true
    }

    private func clearPreviousAccountCredential(
        _ credential: LocationDeviceCredential
    ) async throws {
        try await stopReportingLocally()
        do {
            try await api.deleteLiveLocation(bearerToken: credential.token)
        } catch let error as BrunnAPIError where error.isUnauthorized {
            // Revoked credentials cannot upload; continue clearing local state.
        }
        guard try credentialStore.load() == credential else { return }
        try credentialStore.delete()
        finishLocationDisconnect()
    }

    private func acquireCredentialOperation() async {
        if !credentialOperationHeld {
            credentialOperationHeld = true
            return
        }
        await withCheckedContinuation { continuation in
            credentialOperationWaiters.append(continuation)
        }
    }

    private func releaseCredentialOperation() {
        guard !credentialOperationWaiters.isEmpty else {
            credentialOperationHeld = false
            return
        }
        credentialOperationWaiters.removeFirst().resume()
    }
}

private enum LocationReporterError: Error, LocalizedError {
    case invalidCredential
    case credentialCleanupFailed

    var errorDescription: String? {
        switch self {
        case .invalidCredential:
            "Brunn issued a location credential outside the exact approved capability set."
        case .credentialCleanupFailed:
            "Protected location data could not be cleared before creating replacement access. Unlock this iPhone and retry."
        }
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
