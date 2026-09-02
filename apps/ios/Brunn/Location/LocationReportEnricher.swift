import CoreLocation
import Foundation
import MapKit

@MainActor
final class LocationReportEnricher {
    nonisolated private static let geocodeDistanceThreshold: CLLocationDistance = 5_000
    nonisolated private static let poiRadius: CLLocationDistance = 150
    nonisolated private static let timeoutNanoseconds: UInt64 = 4_000_000_000

    private let statusStore: LocationStatusStore

    init(statusStore: LocationStatusStore) {
        self.statusStore = statusStore
    }

    func enrich(_ report: LocationReport) async -> LocationReport {
        let shouldGeocode = shouldGeocode(report)
        let isVisit = report.type != .ping
        async let geocode = shouldGeocode ? reverseGeocode(report) : nil
        async let poi = isVisit ? nearbyPointsOfInterest(report) : []
        let (resolvedGeocode, resolvedPOI) = await (geocode, poi)
        if resolvedGeocode != nil {
            statusStore.lastGeocodedCoordinate = LocationStoredCoordinate(
                latitude: report.lat,
                longitude: report.lon
            )
        }
        return report.enriched(geocode: resolvedGeocode, poi: resolvedPOI)
    }

    private func shouldGeocode(_ report: LocationReport) -> Bool {
        guard report.type == .ping else { return true }
        guard let last = statusStore.lastGeocodedCoordinate else { return true }
        let current = CLLocation(latitude: report.lat, longitude: report.lon)
        let previous = CLLocation(latitude: last.latitude, longitude: last.longitude)
        return current.distance(from: previous) > Self.geocodeDistanceThreshold
    }

    private func reverseGeocode(_ report: LocationReport) async -> LocationGeocode? {
        let latitude = report.lat
        let longitude = report.lon
        return await Self.withTimeout {
            let location = CLLocation(latitude: latitude, longitude: longitude)
            let placemark = try await CLGeocoder()
                .reverseGeocodeLocation(location)
                .first
            guard let placemark else { return nil }
            let result = LocationGeocode(
                city: placemark.locality,
                region: placemark.administrativeArea,
                country: placemark.isoCountryCode,
                name: placemark.name
            )
            if result.city == nil,
               result.region == nil,
               result.country == nil,
               result.name == nil
            {
                return nil
            }
            return result
        } ?? nil
    }

    private func nearbyPointsOfInterest(_ report: LocationReport) async -> [LocationPOI] {
        let latitude = report.lat
        let longitude = report.lon
        return await Self.withTimeout {
            let center = CLLocationCoordinate2D(latitude: latitude, longitude: longitude)
            let request = MKLocalPointsOfInterestRequest(
                center: center,
                radius: Self.poiRadius
            )
            let response = try await MKLocalSearch(request: request).start()
            let origin = CLLocation(latitude: latitude, longitude: longitude)
            return response.mapItems.compactMap { item -> LocationPOI? in
                let name = item.name?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
                guard !name.isEmpty else { return nil }
                let coordinate = item.placemark.coordinate
                let distance = origin.distance(from: CLLocation(
                    latitude: coordinate.latitude,
                    longitude: coordinate.longitude
                ))
                return LocationPOI(
                    name: name,
                    category: LocationPOICategory.normalizedKind(
                        from: item.pointOfInterestCategory?.rawValue
                    ),
                    distanceM: distance
                )
            }
            .sorted { $0.distanceM < $1.distanceM }
            .prefix(5)
            .map { $0 }
        } ?? []
    }

    nonisolated private static func withTimeout<Value: Sendable>(
        _ operation: @escaping @Sendable () async throws -> Value?
    ) async -> Value? {
        await withTaskGroup(of: Value?.self) { group in
            group.addTask {
                try? await operation()
            }
            group.addTask {
                try? await Task.sleep(nanoseconds: timeoutNanoseconds)
                return nil
            }
            let first = await group.next() ?? nil
            group.cancelAll()
            return first
        }
    }
}
