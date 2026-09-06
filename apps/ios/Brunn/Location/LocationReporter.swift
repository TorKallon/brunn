import Combine
import CoreLocation
import Foundation
import Network
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
    @Published private(set) var health: LocationCaptureHealth
    @Published private(set) var isRefreshingLocation = false

    private let manager: CLLocationManager
    private let api: BrunnAPI
    private let credentialStore: LocationKeychainCredentialStore
    private let queue: LocationDiskQueue
    private let statusStore: LocationStatusStore
    private let enricher: LocationReportEnricher
    private let liveSession: any LocationLiveSession
    private var deliveryTail: Task<Void, Never>?
    private var refreshRequest: LocationFixRequest?
    private var lastForwardedLiveFix: CLLocation?
    private var wasStationary = false
    private var lastHealthPersistedAt = Date.distantPast
    private var networkMonitor: NWPathMonitor?
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
        enricher: LocationReportEnricher,
        liveSession: any LocationLiveSession = SystemLocationLiveSession()
    ) {
        self.manager = manager
        self.api = api
        self.credentialStore = credentialStore
        self.queue = queue
        self.statusStore = statusStore
        self.enricher = enricher
        self.liveSession = liveSession
        authorizationStatus = manager.authorizationStatus
        reportingEnabled = statusStore.reportingEnabled
        lastUploadAt = statusStore.lastUploadAt
        setupPending = statusStore.setupPending
        validatedCredentialUserID = nil
        health = statusStore.captureHealth
        super.init()
        manager.delegate = self
        hasCredential = (try? credentialStore.load()) != nil
        queuedReportCount = (try? queue.count()) ?? 0
    }

    func applicationDidFinishLaunching(relaunchedForLocation: Bool) {
        wasRelaunchedForLocation = relaunchedForLocation
        authorizationStatus = manager.authorizationStatus
        if reportingEnabled {
            startMonitoring(allowNewSession: UIApplication.shared.applicationState != .background)
        }
    }

    func applicationDidBecomeActive(expectedUserID: String?) async {
        authorizationStatus = manager.authorizationStatus
        await acquireCredentialOperation()
        await validateStoredCredential(expectedUserID: expectedUserID)
        releaseCredentialOperation()
        if setupPending {
            requestNextAuthorizationStep()
        }
        if reportingEnabled {
            startMonitoring()
            await refreshLocation()
        }
    }

    /// Acquire evidence on this phone before fetching the server's current answer.
    func refreshLocation() async {
        // Flush retained evidence independently: Core Location can time out or be unavailable,
        // and that must never prevent an existing offline backlog from reaching Brunn.
        async let queuedDelivery: Void = drainQueue()
        await withCheckedContinuation { continuation in
            requestFreshLocation { _ in continuation.resume() }
        }
        await queuedDelivery
        await refreshPresence()
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

    /// Passive recovery can deliver a batch of historical movement samples.
    static let maximumSignificantChangeAge = LocationCapturePolicy.maximumSampleAge

    func handleHeartbeat(completionHandler: @escaping (UIBackgroundFetchResult) -> Void) {
        requestFreshLocation(completion: completionHandler)
    }

    private func requestFreshLocation(completion: @escaping (UIBackgroundFetchResult) -> Void) {
        guard reportingEnabled,
              manager.authorizationStatus == .authorizedAlways,
              validStoredCredential() != nil
        else {
            updateHealth { $0.lastRefreshResult = "unavailable" }
            completion(.noData)
            return
        }
        if let refreshRequest {
            refreshRequest.completions.append(completion)
            return
        }
        let request = LocationFixRequest(completion: completion)
        refreshRequest = request
        isRefreshingLocation = true
        updateHealth { $0.lastRefreshResult = "pending" }
        DispatchQueue.main.asyncAfter(deadline: .now() + 25) { [weak self, weak request] in
            guard let self, let request, self.refreshRequest === request else { return }
            self.finishRefresh(request, result: .failed, status: "timed_out")
        }
        manager.desiredAccuracy = kCLLocationAccuracyHundredMeters
        manager.requestLocation()
    }

    private func finishRefresh(_ request: LocationFixRequest, result: UIBackgroundFetchResult, status: String) {
        guard refreshRequest === request else { return }
        refreshRequest = nil
        isRefreshingLocation = false
        updateHealth { $0.lastRefreshResult = status }
        let completions = request.completions
        request.completions.removeAll()
        for completion in completions { completion(result) }
    }

    @discardableResult
    func handle(location: CLLocation, now: Date = Date(), source: String = "significant_change") -> Bool {
        updateHealth {
            $0.lastCallbackAt = now
            $0.lastCallbackSource = source
            $0.accuracyCategory = LocationCapturePolicy.accuracyCategory(location.horizontalAccuracy)
        }
        let rejection: String?
        if !reportingEnabled { rejection = "reporting_disabled" }
        else if !CLLocationCoordinate2DIsValid(location.coordinate)
            || !location.horizontalAccuracy.isFinite || location.horizontalAccuracy < 0 {
            rejection = "invalid_fix"
        } else if now.timeIntervalSince(location.timestamp) > Self.maximumSignificantChangeAge {
            rejection = "cached_fix"
        } else if location.timestamp.timeIntervalSince(now) > 60 {
            rejection = "future_fix"
        } else { rejection = nil }
        guard rejection == nil else {
            updateHealth { $0.lastRejection = rejection }
            return false
        }
        updateHealth {
            $0.lastRejection = nil
            if location.horizontalAccuracy <= LocationCapturePolicy.maximumUsableAccuracy,
               location.timestamp > ($0.lastUsableFixAt ?? .distantPast) {
                $0.lastUsableFixAt = location.timestamp
            }
        }
        let reportID = enqueue(LocationReport(
            type: .ping,
            at: LocationTimestamp.string(from: location.timestamp),
            lat: location.coordinate.latitude,
            lon: location.coordinate.longitude,
            accuracyM: location.horizontalAccuracy
        ))
        if let reportID, let request = refreshRequest,
           request.startedAt.timeIntervalSince(location.timestamp) <= LocationCapturePolicy.maximumRefreshAge,
           location.horizontalAccuracy <= LocationCapturePolicy.maximumUsableAccuracy {
            request.candidateReportAccuracies[reportID] = location.horizontalAccuracy
        }
        return reportID != nil
    }

    func handleLiveUpdate(location: CLLocation?, stationary: Bool, now: Date = Date()) {
        guard reportingEnabled else { return }
        let transitioned = wasStationary != stationary
        wasStationary = stationary
        updateHealth {
            $0.captureState = stationary ? "stationary" : "tracking"
            $0.lastCallbackAt = now
            $0.lastCallbackSource = "live"
        }
        guard let location else {
            updateHealth { $0.lastRejection = "location_unavailable" }
            return
        }
        let needsFreshSample = refreshRequest.map { $0.candidateReportAccuracies.isEmpty } ?? false
        if let previous = lastForwardedLiveFix, !needsFreshSample,
           !LocationCapturePolicy.shouldForwardLiveFix(
               elapsed: location.timestamp.timeIntervalSince(previous.timestamp),
               distance: location.distance(from: previous),
               accuracy: location.horizontalAccuracy,
               previousAccuracy: previous.horizontalAccuracy,
               stationaryTransition: transitioned
           ) {
            return
        }
        let accepted = handle(location: location, now: now, source: "live")
        if accepted { lastForwardedLiveFix = location }
        // Keep the live stream and background session alive on the final stationary fix.
        // Core Location suspends sampling and automatically resumes it when movement returns.
    }

    func locationManagerDidChangeAuthorization(_ manager: CLLocationManager) {
        authorizationStatus = manager.authorizationStatus
        if manager.authorizationStatus != .authorizedAlways {
            stopMonitoring()
            if let request = refreshRequest {
                finishRefresh(request, result: .noData, status: "permission_required")
            }
        } else if reportingEnabled {
            startMonitoring(allowNewSession: UIApplication.shared.applicationState != .background)
        }
        guard setupPending else { return }
        switch manager.authorizationStatus {
        case .authorizedWhenInUse:
            manager.requestAlwaysAuthorization()
        case .authorizedAlways:
            finishEnable()
        case .denied, .restricted:
            setPendingEnable(false)
            lastError = "Always Location Access is required to report location in the background."
        case .notDetermined:
            break
        @unknown default:
            setPendingEnable(false)
        }
    }

    func locationManager(_: CLLocationManager, didVisit visit: CLVisit) {
        updateHealth {
            $0.lastCallbackAt = Date()
            $0.lastCallbackSource = "visit"
        }
        handle(visit: visit)
    }

    func locationManager(_: CLLocationManager, didUpdateLocations locations: [CLLocation]) {
        guard !locations.isEmpty else {
            updateHealth { $0.lastRejection = "empty_callback" }
            return
        }
        // Core Location delivers fixes oldest first; retain every valid fix in the batch.
        let accepted = locations.filter {
            handle(location: $0, source: refreshRequest == nil ? "significant_change" : "refresh")
        }
        if accepted.isEmpty, let request = refreshRequest {
            finishRefresh(request, result: .noData, status: "cached_or_invalid_fix")
        }
        if reportingEnabled, !liveSession.isRunning {
            startMonitoring(allowNewSession: UIApplication.shared.applicationState != .background)
        }
    }

    func locationManager(_: CLLocationManager, didFailWithError error: Error) {
        lastError = "Location could not obtain a position."
        updateHealth { $0.lastRejection = "location_error" }
        if let request = refreshRequest { finishRefresh(request, result: .failed, status: "location_error") }
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
        requestFreshLocation { _ in }
        Task { await drainQueue() }
    }

    private func startMonitoring(allowNewSession: Bool? = nil) {
        guard reportingEnabled, manager.authorizationStatus == .authorizedAlways,
              validStoredCredential() != nil else { return }
        manager.startMonitoringVisits()
        manager.startMonitoringSignificantLocationChanges()
        if networkMonitor == nil {
            let monitor = NWPathMonitor()
            monitor.pathUpdateHandler = { [weak self] path in
                guard path.status == .satisfied else { return }
                Task { @MainActor in await self?.networkBecameAvailable() }
            }
            monitor.start(queue: DispatchQueue(label: "com.rourkem.brunn.location-connectivity"))
            networkMonitor = monitor
        }
        guard !liveSession.isRunning else { return }
        let canBegin = allowNewSession ?? (UIApplication.shared.applicationState != .background)
        guard canBegin || statusStore.backgroundSessionActive else {
            updateHealth { $0.captureState = "awaiting_foreground" }
            return
        }
        statusStore.backgroundSessionActive = true
        updateHealth { $0.captureState = "starting" }
        liveSession.start { [weak self] location, stationary in
            self?.handleLiveUpdate(location: location, stationary: stationary)
        } failure: { [weak self] in
            self?.updateHealth {
                $0.captureState = "recovering"
                $0.lastRejection = "live_stream_error"
            }
        }
    }

    private func stopMonitoring() {
        liveSession.stop()
        networkMonitor?.cancel()
        networkMonitor = nil
        statusStore.backgroundSessionActive = false
        lastForwardedLiveFix = nil
        wasStationary = false
        manager.stopMonitoringVisits()
        manager.stopMonitoringSignificantLocationChanges()
        updateHealth { $0.captureState = "stopped" }
    }

    func networkBecameAvailable() async {
        guard reportingEnabled, queuedReportCount > 0 else { return }
        await drainQueue()
    }

    private func updateHealth(_ update: (inout LocationCaptureHealth) -> Void) {
        let previous = health
        update(&health)
        let now = Date()
        if now.timeIntervalSince(lastHealthPersistedAt) >= 30
            || previous.captureState != health.captureState
            || previous.lastRejection != health.lastRejection
            || previous.lastRefreshResult != health.lastRefreshResult
            || previous.lastUsableFixAt != health.lastUsableFixAt
            || previous.lastUploadFailureAt != health.lastUploadFailureAt {
            statusStore.captureHealth = health
            lastHealthPersistedAt = now
        }
    }

    private func setPendingEnable(_ pending: Bool) {
        setupPending = pending
        statusStore.setupPending = pending
    }

    private func stopReportingLocally() async throws {
        setPendingEnable(false)
        reportingEnabled = false
        statusStore.reportingEnabled = false
        stopMonitoring()
        if let request = refreshRequest { finishRefresh(request, result: .noData, status: "stopped") }
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
        health = statusStore.captureHealth
    }

    @discardableResult
    private func enqueue(_ report: LocationReport) -> UUID? {
        let lease = LocationBackgroundTaskLease()
        lease.begin()
        let reportID: UUID
        do {
            reportID = try queue.append(report)
            queuedReportCount = (try? queue.count()) ?? queuedReportCount
        } catch {
            lease.end()
            lastError = error.localizedDescription
            updateHealth { $0.lastRejection = "queue_write_failed" }
            return nil
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
        return reportID
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
                updateHealth { $0.lastUploadFailureAt = nil }
                if let request = refreshRequest,
                   let accuracy = batch.compactMap({ request.candidateReportAccuracies[$0.id] }).min() {
                    finishRefresh(request, result: .newData, status:
                        accuracy <= LocationCapturePolicy.preciseAccuracy ? "updated" : "approximate")
                }
            } catch let error as BrunnAPIError {
                activeUploadTask = nil
                guard reportingEnabled, !Task.isCancelled,
                      credentialIsCurrent(credential)
                else { return }
                if !handleCredentialFailureIfNeeded(error, credential: credential) {
                    lastError = error.localizedDescription
                    updateHealth { $0.lastUploadFailureAt = Date() }
                    finishRefreshAfterUploadFailure()
                }
                return
            } catch {
                activeUploadTask = nil
                guard reportingEnabled, !Task.isCancelled,
                      credentialIsCurrent(credential)
                else { return }
                lastError = error.localizedDescription
                updateHealth { $0.lastUploadFailureAt = Date() }
                finishRefreshAfterUploadFailure()
                return
            }
        }
    }

    private func finishRefreshAfterUploadFailure() {
        if let request = refreshRequest, !request.candidateReportAccuracies.isEmpty {
            finishRefresh(request, result: .failed, status: "upload_failed")
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
        if let request = refreshRequest { finishRefresh(request, result: .noData, status: "access_revoked") }
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
        health = statusStore.captureHealth
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

@MainActor
protocol LocationLiveSession: AnyObject {
    var isRunning: Bool { get }
    func start(
        update: @escaping (CLLocation?, Bool) -> Void,
        failure: @escaping () -> Void
    )
    func stop()
}

/// Holding both objects preserves Core Location's native stationary pause/resume behavior.
@MainActor
final class SystemLocationLiveSession: LocationLiveSession {
    private var backgroundSession: CLBackgroundActivitySession?
    private var task: Task<Void, Never>?
    var isRunning: Bool { task != nil }

    func start(update: @escaping (CLLocation?, Bool) -> Void, failure: @escaping () -> Void) {
        guard task == nil else { return }
        backgroundSession = CLBackgroundActivitySession()
        task = Task {
            var retrySeconds: UInt64 = 30
            while !Task.isCancelled {
                do {
                    for try await value in CLLocationUpdate.liveUpdates(.default) {
                        guard !Task.isCancelled else { return }
                        update(value.location, value.isStationary)
                        retrySeconds = 30
                    }
                } catch {
                    if Task.isCancelled { return }
                }
                guard !Task.isCancelled else { return }
                failure()
                // Recover a terminated stream without spinning; permission/disable cancels this task.
                do { try await Task.sleep(nanoseconds: retrySeconds * 1_000_000_000) }
                catch { return }
                retrySeconds = min(retrySeconds * 2, 300)
            }
        }
    }

    func stop() {
        task?.cancel()
        task = nil
        backgroundSession?.invalidate()
        backgroundSession = nil
    }

    deinit { task?.cancel() }
}

@MainActor
private final class LocationFixRequest {
    let startedAt = Date()
    var candidateReportAccuracies: [UUID: Double] = [:]
    var completions: [(UIBackgroundFetchResult) -> Void]

    init(completion: @escaping (UIBackgroundFetchResult) -> Void) {
        completions = [completion]
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
