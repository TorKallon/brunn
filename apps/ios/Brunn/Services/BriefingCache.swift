import Foundation

struct CachedBriefing: Codable, Equatable {
    let savedAt: Date
    let edition: BriefingEditionData
}

actor BriefingCache {
    private let fileManager: FileManager
    private let fileURL: URL

    init(fileManager: FileManager = .default, fileURL: URL? = nil) {
        self.fileManager = fileManager
        if let fileURL {
            self.fileURL = fileURL
        } else {
            let root = fileManager.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            self.fileURL = root
                .appendingPathComponent("Brunn", isDirectory: true)
                .appendingPathComponent("latest-briefing.json", isDirectory: false)
        }
    }

    func load() throws -> CachedBriefing? {
        guard fileManager.fileExists(atPath: fileURL.path) else { return nil }
        let data = try Data(contentsOf: fileURL)
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        return try decoder.decode(CachedBriefing.self, from: data)
    }

    func save(_ edition: BriefingEditionData, at date: Date = .now) throws {
        let directory = fileURL.deletingLastPathComponent()
        try fileManager.createDirectory(
            at: directory,
            withIntermediateDirectories: true,
            attributes: [.protectionKey: FileProtectionType.complete]
        )
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        encoder.dateEncodingStrategy = .iso8601
        let data = try encoder.encode(CachedBriefing(savedAt: date, edition: edition))
        try data.write(to: fileURL, options: [.atomic, .completeFileProtection])
    }

    func clear() throws {
        guard fileManager.fileExists(atPath: fileURL.path) else { return }
        try fileManager.removeItem(at: fileURL)
    }
}

struct CachedTaskSurface: Codable, Equatable {
    let userID: String?
    let savedAt: Date
    let urgent: [AgentTaskCandidate]
    let next: [AgentTaskCandidate]
    let doneToday: AgentTaskDoneSummaryData?
    let projects: [AgentTaskProject]
    let contexts: [AgentTaskContext]
    let selectedContexts: [String]
    let nextRemaining: Int
    let backlogTotal: Int
}

actor TaskSurfaceCache {
    private struct AccountBinding: Codable {
        static let schema = "brunn.task-surface-account@v2"

        let schema: String
        let userID: String
        let sessionFingerprint: String
    }

    private let fileManager: FileManager
    private let fileURL: URL
    private let accountBindingURL: URL

    init(fileManager: FileManager = .default, fileURL: URL? = nil) {
        self.fileManager = fileManager
        if let fileURL {
            self.fileURL = fileURL
        } else {
            let root = fileManager.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            self.fileURL = root
                .appendingPathComponent("Brunn", isDirectory: true)
                .appendingPathComponent("today-tasks.json", isDirectory: false)
        }
        accountBindingURL = self.fileURL.deletingLastPathComponent()
            .appendingPathComponent("today-tasks-account.json", isDirectory: false)
    }

    func load() throws -> CachedTaskSurface? {
        guard fileManager.fileExists(atPath: fileURL.path) else { return nil }
        let data = try Data(contentsOf: fileURL)
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        return try decoder.decode(CachedTaskSurface.self, from: data)
    }

    func save(_ value: CachedTaskSurface, sessionFingerprint: String) throws {
        let directory = fileURL.deletingLastPathComponent()
        try fileManager.createDirectory(
            at: directory,
            withIntermediateDirectories: true,
            attributes: [.protectionKey: FileProtectionType.complete]
        )
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        encoder.dateEncodingStrategy = .iso8601
        try encoder.encode(value).write(
            to: fileURL,
            options: [.atomic, .completeFileProtection]
        )
        if let userID = value.userID {
            try bind(to: userID, sessionFingerprint: sessionFingerprint)
        }
    }

    func boundUserID(matching sessionFingerprint: String) throws -> String? {
        guard fileManager.fileExists(atPath: accountBindingURL.path) else { return nil }
        let binding = try JSONDecoder().decode(
            AccountBinding.self,
            from: Data(contentsOf: accountBindingURL)
        )
        guard binding.schema == AccountBinding.schema,
              !binding.userID.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              binding.sessionFingerprint == sessionFingerprint
        else {
            throw CocoaError(.fileReadCorruptFile)
        }
        return binding.userID
    }

    func bind(to userID: String, sessionFingerprint: String) throws {
        guard !userID.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              sessionFingerprint.hasPrefix("sha256:"),
              sessionFingerprint.count == 71
        else {
            throw CocoaError(.fileWriteInvalidFileName)
        }
        let directory = accountBindingURL.deletingLastPathComponent()
        try fileManager.createDirectory(
            at: directory,
            withIntermediateDirectories: true,
            attributes: [.protectionKey: FileProtectionType.complete]
        )
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        try encoder.encode(AccountBinding(
            schema: AccountBinding.schema,
            userID: userID,
            sessionFingerprint: sessionFingerprint
        )).write(
            to: accountBindingURL,
            options: [.atomic, .completeFileProtection]
        )
    }

    func clear() throws {
        var firstError: (any Error)?
        for url in [fileURL, accountBindingURL] where fileManager.fileExists(atPath: url.path) {
            do {
                try fileManager.removeItem(at: url)
            } catch {
                if firstError == nil {
                    firstError = error
                }
            }
        }
        if let firstError {
            throw firstError
        }
    }
}
