import Foundation

struct LocationQueuedReport: Codable, Sendable, Equatable, Identifiable {
    let id: UUID
    var report: LocationReport
    var isEnriched: Bool

    init(id: UUID = UUID(), report: LocationReport, isEnriched: Bool = false) {
        self.id = id
        self.report = report
        self.isEnriched = isEnriched
    }

    enum CodingKeys: String, CodingKey {
        case id
        case report
        case isEnriched = "enriched"
    }
}

enum LocationDiskQueueError: Error, LocalizedError {
    case invalidQueue

    var errorDescription: String? {
        "The saved location report queue is invalid."
    }
}

final class LocationDiskQueue {
    static let maximumCount = 2000
    static let maximumBatchCount = 200

    private let fileURL: URL
    private let fileManager: FileManager

    init(fileURL: URL? = nil, fileManager: FileManager = .default) {
        self.fileManager = fileManager
        self.fileURL = fileURL ?? Self.defaultFileURL(fileManager: fileManager)
    }

    @discardableResult
    func append(_ report: LocationReport) throws -> UUID {
        var entries = try load()
        let entry = LocationQueuedReport(report: report)
        entries.append(entry)
        if entries.count > Self.maximumCount {
            entries.removeFirst(entries.count - Self.maximumCount)
        }
        try persist(entries)
        return entry.id
    }

    func replace(id: UUID, with report: LocationReport) throws {
        var entries = try load()
        guard let index = entries.firstIndex(where: { $0.id == id }) else { return }
        entries[index].report = report
        entries[index].isEnriched = true
        try persist(entries)
    }

    func batch() throws -> [LocationQueuedReport] {
        let readyPrefix = try load().prefix(while: \.isEnriched)
        return Array(readyPrefix.prefix(Self.maximumBatchCount))
    }

    func nextPending() throws -> LocationQueuedReport? {
        try load().first { !$0.isEnriched }
    }

    func count() throws -> Int {
        try load().count
    }

    func remove(ids: [UUID]) throws {
        guard !ids.isEmpty else { return }
        let sent = Set(ids)
        let retained = try load().filter { !sent.contains($0.id) }
        try persist(retained)
    }

    func clear() throws {
        if fileManager.fileExists(atPath: fileURL.path) {
            try fileManager.removeItem(at: fileURL)
        }
    }

    private func load() throws -> [LocationQueuedReport] {
        guard fileManager.fileExists(atPath: fileURL.path) else { return [] }
        do {
            return try JSONDecoder().decode(
                [LocationQueuedReport].self,
                from: Data(contentsOf: fileURL)
            )
        } catch {
            throw LocationDiskQueueError.invalidQueue
        }
    }

    private func persist(_ entries: [LocationQueuedReport]) throws {
        let directory = fileURL.deletingLastPathComponent()
        try fileManager.createDirectory(
            at: directory,
            withIntermediateDirectories: true,
            attributes: nil
        )
        var directoryValues = URLResourceValues()
        directoryValues.isExcludedFromBackup = true
        var mutableDirectory = directory
        try? mutableDirectory.setResourceValues(directoryValues)

        if entries.isEmpty {
            if fileManager.fileExists(atPath: fileURL.path) {
                try fileManager.removeItem(at: fileURL)
            }
            return
        }
        let data = try JSONEncoder().encode(entries)
        try data.write(to: fileURL, options: .atomic)
        var fileValues = URLResourceValues()
        fileValues.isExcludedFromBackup = true
        var mutableFile = fileURL
        try? mutableFile.setResourceValues(fileValues)
    }

    private static func defaultFileURL(fileManager: FileManager) -> URL {
        let roots = fileManager.urls(for: .applicationSupportDirectory, in: .userDomainMask)
        var directory = roots[0].appendingPathComponent("Brunn", isDirectory: true)
#if DEBUG
        if let namespace = ProcessInfo.processInfo.environment["BRUNN_CREDENTIAL_NAMESPACE"],
           isValidNamespace(namespace)
        {
            directory.appendPathComponent(namespace, isDirectory: true)
        }
#endif
        return directory.appendingPathComponent("location-queue.json", isDirectory: false)
    }

    private static func isValidNamespace(_ value: String) -> Bool {
        let allowed = CharacterSet(
            charactersIn: "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_"
        )
        return (1 ... 64).contains(value.utf8.count)
            && value.unicodeScalars.allSatisfy(allowed.contains)
    }
}
