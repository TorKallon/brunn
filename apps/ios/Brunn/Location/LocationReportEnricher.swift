import CoreLocation
import Foundation
import MapKit

@MainActor
final class LocationReportEnricher {
    nonisolated private static let geocodeDistanceThreshold: CLLocationDistance = 75
    nonisolated private static let poiRadius: CLLocationDistance = 150

    private let statusStore: LocationStatusStore
    private let geocodeOverride: (@MainActor (LocationReport) async -> LocationGeocode?)?
    private var lastGeocode: LocationGeocode?
    private var lastAttemptAt: Date?
    private var lastResolvedAt: Date?

    init(
        statusStore: LocationStatusStore,
        geocode: (@MainActor (LocationReport) async -> LocationGeocode?)? = nil
    ) {
        self.statusStore = statusStore
        geocodeOverride = geocode
    }

    func enrich(_ report: LocationReport, now: Date = Date()) async -> LocationReport {
        let shouldGeocode = shouldGeocode(report, now: now)
        if shouldGeocode { lastAttemptAt = now }
        let isVisit = report.type != .ping
        async let geocode = shouldGeocode ? reverseGeocode(report) : nil
        // Nearby businesses are ambiguous while moving; map/address information is the
        // current-position fallback, and POI evidence remains reserved for accurate visits.
        async let poi = isVisit && report.accuracyM <= 200 ? nearbyPointsOfInterest(report) : []
        let (resolvedGeocode, resolvedPOI) = await (geocode, poi)
        if resolvedGeocode != nil {
            lastGeocode = resolvedGeocode
            lastResolvedAt = now
            statusStore.lastGeocodedCoordinate = LocationStoredCoordinate(
                latitude: report.lat,
                longitude: report.lon
            )
        }
        let reusable = distanceFromLastGeocode(report).map { $0 < Self.geocodeDistanceThreshold } == true
            && lastResolvedAt.map { now.timeIntervalSince($0) < 15 * 60 } == true
        return report.enriched(geocode: resolvedGeocode ?? (reusable ? lastGeocode : nil), poi: resolvedPOI)
    }

    private func shouldGeocode(_ report: LocationReport, now: Date) -> Bool {
        guard report.accuracyM <= LocationCapturePolicy.maximumUsableAccuracy else { return false }
        // Limit external work independently of the native capture stream. A changed position
        // can still be uploaded immediately with coordinates while enrichment is throttled.
        if let lastAttemptAt, now.timeIntervalSince(lastAttemptAt) < 60 { return false }
        guard report.type == .ping else { return true }
        guard lastGeocode != nil, let lastResolvedAt,
              now.timeIntervalSince(lastResolvedAt) < 15 * 60,
              let distance = distanceFromLastGeocode(report) else { return true }
        return distance >= Self.geocodeDistanceThreshold
    }

    private func distanceFromLastGeocode(_ report: LocationReport) -> CLLocationDistance? {
        guard let last = statusStore.lastGeocodedCoordinate else { return nil }
        let current = CLLocation(latitude: report.lat, longitude: report.lon)
        let previous = CLLocation(latitude: last.latitude, longitude: last.longitude)
        return current.distance(from: previous)
    }

    private func reverseGeocode(_ report: LocationReport) async -> LocationGeocode? {
        if let geocodeOverride { return await geocodeOverride(report) }
        let geocoder = CLGeocoder()
        let deadline = LocationEnrichmentDeadline<LocationGeocode> {
            geocoder.cancelGeocode()
        }
        return await deadline.value { completion in
            geocoder.reverseGeocodeLocation(CLLocation(latitude: report.lat, longitude: report.lon)) { placemarks, _ in
                let result = placemarks?.first.map { placemark in
                    let street = [placemark.subThoroughfare, placemark.thoroughfare]
                        .compactMap { $0 }.joined(separator: " ")
                    return LocationGeocode(
                        city: placemark.locality,
                        region: placemark.administrativeArea,
                        country: placemark.isoCountryCode,
                        name: street.isEmpty
                            ? (placemark.subLocality ?? placemark.locality ?? placemark.name)
                            : street
                    )
                }
                Task { @MainActor in completion(result) }
            }
        }
    }

    private func nearbyPointsOfInterest(_ report: LocationReport) async -> [LocationPOI] {
        let center = CLLocationCoordinate2D(latitude: report.lat, longitude: report.lon)
        let request = MKLocalPointsOfInterestRequest(center: center, radius: Self.poiRadius)
        let search = MKLocalSearch(request: request)
        let deadline = LocationEnrichmentDeadline<[LocationPOI]> { search.cancel() }
        return await deadline.value { completion in
            search.start { response, _ in
                let origin = CLLocation(latitude: report.lat, longitude: report.lon)
                let matches = response?.mapItems.compactMap { item -> LocationPOI? in
                    let name = item.name?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
                    guard !name.isEmpty else { return nil }
                    let coordinate = item.placemark.coordinate
                    return LocationPOI(
                        name: name,
                        category: LocationPOICategory.normalizedKind(from: item.pointOfInterestCategory?.rawValue),
                        distanceM: origin.distance(from: CLLocation(
                            latitude: coordinate.latitude, longitude: coordinate.longitude
                        ))
                    )
                }
                .sorted { $0.distanceM < $1.distanceM }
                .prefix(5)
                .map { $0 }
                completion(matches)
            }
        } ?? []
    }
}

/// Timeout and task cancellation resume the caller immediately and cancel the underlying
/// Apple request. Late callbacks are ignored, so an uncooperative service cannot hold uploads.
@MainActor
final class LocationEnrichmentDeadline<Value: Sendable> {
    private let cancel: () -> Void
    private var continuation: CheckedContinuation<Value?, Never>?
    private var finished = false

    init(cancel: @escaping () -> Void) { self.cancel = cancel }

    func value(
        timeout: TimeInterval = 4,
        start: (@escaping @MainActor @Sendable (Value?) -> Void) -> Void
    ) async -> Value? {
        await withTaskCancellationHandler {
            await withCheckedContinuation { continuation in
                self.continuation = continuation
                if Task.isCancelled || finished {
                    finish(nil, cancelRequest: true)
                    return
                }
                start { [weak self] value in self?.finish(value, cancelRequest: false) }
                DispatchQueue.main.asyncAfter(deadline: .now() + timeout) { [weak self] in
                    self?.finish(nil, cancelRequest: true)
                }
            }
        } onCancel: {
            Task { @MainActor in self.finish(nil, cancelRequest: true) }
        }
    }

    private func finish(_ value: Value?, cancelRequest: Bool) {
        guard let continuation else {
            if cancelRequest { finished = true }
            return
        }
        self.continuation = nil
        finished = true
        continuation.resume(returning: value)
        if cancelRequest { cancel() }
    }
}
