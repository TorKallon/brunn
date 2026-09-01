import CryptoKit
import Foundation

struct MessagingBackgroundUploadResponse: @unchecked Sendable {
    let data: Data
    let response: HTTPURLResponse
}

enum MessagingBackgroundTransportError: Error, LocalizedError, Equatable {
    case invalidRequest
    case persistedBodyMismatch
    case responseTooLarge
    case invalidResponse
    case transferContinuesInBackground
    case artifactPurgeFailed

    var errorDescription: String? {
        switch self {
        case .invalidRequest:
            "The queued message request is invalid."
        case .persistedBodyMismatch:
            "The protected queued message bytes did not match."
        case .responseTooLarge:
            "Brunn returned an unexpectedly large message receipt."
        case .invalidResponse:
            "Brunn returned an invalid message receipt."
        case .transferContinuesInBackground:
            "Message delivery is continuing safely in the background."
        case .artifactPurgeFailed:
            "Protected message upload data could not be removed."
        }
    }
}

private final class MessagingBackgroundTransportRegistry: @unchecked Sendable {
    static let shared = MessagingBackgroundTransportRegistry()

    private let lock = NSLock()
    private var instance: MessagingBackgroundTransport?

    func resolve() -> MessagingBackgroundTransport {
        lock.lock()
        defer { lock.unlock() }
        if let instance { return instance }
        let created = MessagingBackgroundTransport()
        instance = created
        return created
    }
}

private final class MessagingBackgroundEventCompletionBroker: @unchecked Sendable {
    static let shared = MessagingBackgroundEventCompletionBroker()

    private let lock = NSLock()
    private var handlers: [() -> Void] = []

    func append(_ handler: @escaping () -> Void) {
        lock.lock()
        handlers.append(handler)
        lock.unlock()
    }

    func drain() -> [() -> Void] {
        lock.lock()
        defer { lock.unlock() }
        let pending = handlers
        handlers.removeAll()
        return pending
    }
}

/// A single delegate-backed background session for durable message uploads.
///
/// Background URLSession convenience APIs are intentionally unavailable. This
/// adapter therefore persists the already-canonical JSON bytes to a protected,
/// backup-excluded file and submits only a file-backed upload task. The opaque
/// task description is stable across relaunches, so a foreground retry joins an
/// existing system task instead of creating another transfer. Server-side
/// client-key idempotency remains the final authority after an ambiguous result.
final class MessagingBackgroundTransport: NSObject, @unchecked Sendable {
    static var shared: MessagingBackgroundTransport {
        MessagingBackgroundTransportRegistry.shared.resolve()
    }
    static let sessionIdentifier = "com.rourkem.brunn.messaging-outbox.v1"
    static let fileProtection: FileProtectionType = .completeUntilFirstUserAuthentication

    private static let taskDescriptionPrefix = "brunn.messaging-upload.v1."
    private static let maximumResponseBytes = 1_048_576
    private static let maximumBufferedResults = 64
    private static let defaultAcknowledgementTimeout: TimeInterval = 5

    private typealias UploadContinuation = CheckedContinuation<
        MessagingBackgroundUploadResponse,
        Error
    >

    private struct Waiter {
        let id: UUID
        let continuation: UploadContinuation
    }

    private let lock = NSLock()
    private let fileManager: FileManager
    private let uploadDirectory: URL
    private let artifactRemover: (URL) throws -> Void
    private let delegateQueue: OperationQueue
    private var session: URLSession!
    private var responseDataByTaskID: [Int: Data] = [:]
    private var oversizedResponseTaskIDs = Set<Int>()
    private var taskDescriptionByTaskID: [Int: String] = [:]
    private var activeTaskIDByDescription: [String: Int] = [:]
    private var waitersByTaskID: [Int: [Waiter]] = [:]
    private var completedResultsByDescription: [
        String: Result<MessagingBackgroundUploadResponse, Error>
    ] = [:]
    private var completedResultOrder: [String] = []
    private var taskGenerationByTaskID: [Int: UInt64] = [:]
    private var cleanupGeneration: UInt64 = 0
    private var isPurgingArtifacts = false
    private var artifactPurgeFailed = false
    private var purgeContinuations: [CheckedContinuation<Bool, Never>] = []

    fileprivate override convenience init() {
        let fileManager = FileManager.default
        let uploadDirectory = (try? Self.defaultUploadDirectory(fileManager: fileManager))
            ?? fileManager.temporaryDirectory.appendingPathComponent(
                "BrunnMessagingUploads",
                isDirectory: true
            )
        let configuration = URLSessionConfiguration.background(
            withIdentifier: Self.sessionIdentifier
        )
        configuration.sessionSendsLaunchEvents = true
        configuration.isDiscretionary = false
        configuration.timeoutIntervalForResource = 24 * 60 * 60
        self.init(
            fileManager: fileManager,
            uploadDirectory: uploadDirectory,
            configuration: configuration
        )
    }

    init(
        fileManager: FileManager,
        uploadDirectory: URL,
        configuration: URLSessionConfiguration,
        artifactRemover: ((URL) throws -> Void)? = nil
    ) {
        self.fileManager = fileManager
        self.uploadDirectory = uploadDirectory
        self.artifactRemover = artifactRemover ?? { url in
            try fileManager.removeItem(at: url)
        }
        let queue = OperationQueue()
        queue.name = "com.rourkem.brunn.messaging-background-session"
        queue.maxConcurrentOperationCount = 1
        queue.qualityOfService = .utility
        delegateQueue = queue
        super.init()

        configuration.requestCachePolicy = .reloadIgnoringLocalCacheData
        configuration.urlCache = nil
        configuration.httpCookieStorage = nil
        configuration.httpShouldSetCookies = false
        configuration.urlCredentialStorage = nil
        session = URLSession(
            configuration: configuration,
            delegate: self,
            delegateQueue: delegateQueue
        )
    }

    static func handlesBackgroundSession(identifier: String) -> Bool {
        identifier == sessionIdentifier
    }

    static func handleBackgroundEvents(
        identifier: String,
        completionHandler: @escaping () -> Void
    ) -> Bool {
        guard handlesBackgroundSession(identifier: identifier) else { return false }
        // UIKit can relaunch the app solely for these callbacks. Persist the
        // completion before constructing the session, matching Apple's required
        // restoration order even when no normal AppModel bootstrap runs.
        MessagingBackgroundEventCompletionBroker.shared.append(completionHandler)
        _ = shared
        return true
    }

    static func clearAuthenticatedSessionArtifacts() async -> Bool {
        // Resolve even when this process has not sent a message: iOS may own a
        // restored task, and a prior launch may have left a body file before
        // URLSession task registration.
        return await shared.cancelAllAndPurgeArtifacts()
    }

    static func stableTaskDescription(
        requestURL: URL,
        exactRequestData: Data
    ) -> String {
        var material = Data(requestURL.absoluteString.utf8)
        material.append(0)
        material.append(exactRequestData)
        let digest = SHA256.hash(data: material)
        return taskDescriptionPrefix + digest.map {
            String(format: "%02x", $0)
        }.joined()
    }

    static func defaultUploadDirectory(
        fileManager: FileManager = .default
    ) throws -> URL {
        guard let root = fileManager.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        ).first else {
            throw CocoaError(.fileNoSuchFile)
        }
        return root
            .appendingPathComponent("Brunn", isDirectory: true)
            .appendingPathComponent("AgentMessaging", isDirectory: true)
            .appendingPathComponent("BackgroundUploads", isDirectory: true)
    }

    @discardableResult
    static func prepareUploadFile(
        exactRequestData: Data,
        taskDescription: String,
        directory: URL,
        fileManager: FileManager = .default
    ) throws -> URL {
        guard !exactRequestData.isEmpty,
              taskDescription.hasPrefix(taskDescriptionPrefix),
              taskDescription.dropFirst(taskDescriptionPrefix.count).allSatisfy({
                  $0.isHexDigit
              })
        else {
            throw MessagingBackgroundTransportError.invalidRequest
        }

        try fileManager.createDirectory(
            at: directory,
            withIntermediateDirectories: true,
            attributes: [.protectionKey: fileProtection]
        )
        try fileManager.setAttributes(
            [.protectionKey: fileProtection],
            ofItemAtPath: directory.path
        )
        var protectedDirectory = directory
        var directoryValues = URLResourceValues()
        directoryValues.isExcludedFromBackup = true
        try protectedDirectory.setResourceValues(directoryValues)

        let digest = String(taskDescription.dropFirst(taskDescriptionPrefix.count))
        let fileURL = directory.appendingPathComponent("\(digest).json")
        if fileManager.fileExists(atPath: fileURL.path) {
            guard try Data(contentsOf: fileURL) == exactRequestData else {
                throw MessagingBackgroundTransportError.persistedBodyMismatch
            }
        } else {
            try exactRequestData.write(
                to: fileURL,
                options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication]
            )
        }
        try fileManager.setAttributes(
            [.protectionKey: fileProtection],
            ofItemAtPath: fileURL.path
        )
        var protectedFile = fileURL
        var fileValues = URLResourceValues()
        fileValues.isExcludedFromBackup = true
        try protectedFile.setResourceValues(fileValues)
        return fileURL
    }

    func upload(
        request: URLRequest,
        exactRequestData: Data,
        acknowledgementTimeout: TimeInterval = defaultAcknowledgementTimeout
    ) async throws -> MessagingBackgroundUploadResponse {
        guard let requestURL = request.url,
              ["http", "https"].contains(requestURL.scheme?.lowercased() ?? ""),
              requestURL.user == nil,
              requestURL.password == nil,
              request.httpMethod?.uppercased() == "POST",
              request.httpBody == nil,
              !exactRequestData.isEmpty
        else {
            throw MessagingBackgroundTransportError.invalidRequest
        }

        let description = Self.stableTaskDescription(
            requestURL: requestURL,
            exactRequestData: exactRequestData
        )
        let preparation = try lock.withLock { () -> (
            completed: Result<MessagingBackgroundUploadResponse, Error>?,
            generation: UInt64,
            bodyFile: URL?
        ) in
            if let completed = completedResultsByDescription.removeValue(
                forKey: description
            ) {
                completedResultOrder.removeAll { $0 == description }
                return (completed, cleanupGeneration, nil)
            }
            guard !artifactPurgeFailed else {
                throw MessagingBackgroundTransportError.artifactPurgeFailed
            }
            guard !isPurgingArtifacts else {
                throw CancellationError()
            }
            let bodyFile = try Self.prepareUploadFile(
                exactRequestData: exactRequestData,
                taskDescription: description,
                directory: uploadDirectory,
                fileManager: fileManager
            )
            return (nil, cleanupGeneration, bodyFile)
        }
        if let completed = preparation.completed {
            return try completed.get()
        }
        guard let bodyFile = preparation.bodyFile else {
            throw MessagingBackgroundTransportError.invalidRequest
        }
        let uploadGeneration = preparation.generation
        let existingTasks = await session.allTasks.filter {
            $0.taskDescription == description && $0.state != .completed
        }
        let waiterID = UUID()

        return try await withCheckedThrowingContinuation { continuation in
            var taskToResume: URLSessionTask?
            var resultToResume: Result<MessagingBackgroundUploadResponse, Error>?

            lock.lock()
            guard !artifactPurgeFailed else {
                lock.unlock()
                continuation.resume(
                    throwing: MessagingBackgroundTransportError.artifactPurgeFailed
                )
                return
            }
            guard !isPurgingArtifacts, uploadGeneration == cleanupGeneration else {
                lock.unlock()
                continuation.resume(throwing: CancellationError())
                return
            }
            if let completed = completedResultsByDescription.removeValue(
                forKey: description
            ) {
                completedResultOrder.removeAll { $0 == description }
                resultToResume = completed
            } else {
                let task: URLSessionTask
                if let activeTaskID = activeTaskIDByDescription[description] {
                    taskDescriptionByTaskID[activeTaskID] = description
                    taskGenerationByTaskID[activeTaskID] = uploadGeneration
                    waitersByTaskID[activeTaskID, default: []].append(Waiter(
                        id: waiterID,
                        continuation: continuation
                    ))
                    lock.unlock()
                    scheduleAcknowledgementTimeout(
                        taskID: activeTaskID,
                        waiterID: waiterID,
                        after: acknowledgementTimeout
                    )
                    return
                } else if let existingTask = existingTasks.first {
                    task = existingTask
                } else {
                    let uploadTask = session.uploadTask(with: request, fromFile: bodyFile)
                    uploadTask.taskDescription = description
                    uploadTask.countOfBytesClientExpectsToSend = Int64(exactRequestData.count)
                    uploadTask.countOfBytesClientExpectsToReceive = 4_096
                    task = uploadTask
                    taskToResume = uploadTask
                }

                activeTaskIDByDescription[description] = task.taskIdentifier
                taskDescriptionByTaskID[task.taskIdentifier] = description
                taskGenerationByTaskID[task.taskIdentifier] = uploadGeneration
                waitersByTaskID[task.taskIdentifier, default: []].append(Waiter(
                    id: waiterID,
                    continuation: continuation
                ))
                if task.state == .suspended {
                    taskToResume = task
                }
            }
            lock.unlock()

            if let resultToResume {
                continuation.resume(with: resultToResume)
                return
            }
            taskToResume?.resume()
            if let taskID = taskToResume?.taskIdentifier
                ?? existingTasks.first?.taskIdentifier
            {
                scheduleAcknowledgementTimeout(
                    taskID: taskID,
                    waiterID: waiterID,
                    after: acknowledgementTimeout
                )
            }
        }
    }

    func cancelAllAndPurgeArtifacts() async -> Bool {
        await withCheckedContinuation {
            (continuation: CheckedContinuation<Bool, Never>) in
            var abandonedWaiters: [Waiter] = []
            var shouldStartPurge = false

            lock.lock()
            purgeContinuations.append(continuation)
            if !isPurgingArtifacts {
                isPurgingArtifacts = true
                cleanupGeneration &+= 1
                abandonedWaiters = waitersByTaskID.values.flatMap { $0 }
                waitersByTaskID.removeAll()
                responseDataByTaskID.removeAll()
                oversizedResponseTaskIDs.removeAll()
                taskDescriptionByTaskID.removeAll()
                activeTaskIDByDescription.removeAll()
                taskGenerationByTaskID.removeAll()
                completedResultsByDescription.removeAll()
                completedResultOrder.removeAll()
                shouldStartPurge = true
            }
            lock.unlock()

            for waiter in abandonedWaiters {
                waiter.continuation.resume(throwing: CancellationError())
            }
            if shouldStartPurge {
                Task { [self] in
                    await cancelRegisteredTasksAndPurgeArtifacts()
                }
            }
        }
    }

    private func cancelRegisteredTasksAndPurgeArtifacts() async {
        let tasks = await session.allTasks.filter { $0.state != .completed }
        tasks.forEach { $0.cancel() }

        let (continuations, purgeSucceeded) = lock.withLock {
            // This directory is owned exclusively by the messaging background
            // transport. Removing it under the same lock used to prepare upload
            // bodies prevents a late completion callback from racing a new
            // login that recreates the same opaque body path.
            var purgeSucceeded = true
            if fileManager.fileExists(atPath: uploadDirectory.path) {
                do {
                    try artifactRemover(uploadDirectory)
                } catch {
                    purgeSucceeded = false
                }
            }
            // A completion delegate can race cancellation. Drop anything it
            // may have buffered before making uploads available to a later
            // login.
            responseDataByTaskID.removeAll()
            oversizedResponseTaskIDs.removeAll()
            completedResultsByDescription.removeAll()
            completedResultOrder.removeAll()
            artifactPurgeFailed = !purgeSucceeded
            isPurgingArtifacts = false
            let pending = purgeContinuations
            purgeContinuations.removeAll()
            return (pending, purgeSucceeded)
        }

        continuations.forEach { $0.resume(returning: purgeSucceeded) }
    }

    private func scheduleAcknowledgementTimeout(
        taskID: Int,
        waiterID: UUID,
        after interval: TimeInterval
    ) {
        DispatchQueue.global(qos: .utility).asyncAfter(
            deadline: .now() + max(0.1, interval)
        ) { [weak self] in
            self?.expireWaiter(taskID: taskID, waiterID: waiterID)
        }
    }

    private func expireWaiter(taskID: Int, waiterID: UUID) {
        var continuation: UploadContinuation?
        lock.lock()
        if var waiters = waitersByTaskID[taskID],
           let index = waiters.firstIndex(where: { $0.id == waiterID })
        {
            continuation = waiters.remove(at: index).continuation
            if waiters.isEmpty {
                waitersByTaskID.removeValue(forKey: taskID)
            } else {
                waitersByTaskID[taskID] = waiters
            }
        }
        lock.unlock()
        continuation?.resume(throwing: MessagingBackgroundTransportError
            .transferContinuesInBackground)
    }

    private func storeCompletedResult(
        _ result: Result<MessagingBackgroundUploadResponse, Error>,
        description: String
    ) {
        completedResultsByDescription[description] = result
        completedResultOrder.removeAll { $0 == description }
        completedResultOrder.append(description)
        while completedResultOrder.count > Self.maximumBufferedResults {
            let discarded = completedResultOrder.removeFirst()
            completedResultsByDescription.removeValue(forKey: discarded)
        }
    }

    private func uploadFileURL(description: String) -> URL? {
        guard description.hasPrefix(Self.taskDescriptionPrefix) else { return nil }
        let digest = description.dropFirst(Self.taskDescriptionPrefix.count)
        guard !digest.isEmpty, digest.allSatisfy({ $0.isHexDigit }) else { return nil }
        return uploadDirectory.appendingPathComponent("\(digest).json")
    }
}

extension MessagingBackgroundTransport: URLSessionDataDelegate {
    func urlSession(
        _: URLSession,
        dataTask: URLSessionDataTask,
        didReceive data: Data
    ) {
        lock.lock()
        defer { lock.unlock() }
        let taskID = dataTask.taskIdentifier
        guard !oversizedResponseTaskIDs.contains(taskID) else { return }
        var buffered = responseDataByTaskID[taskID, default: Data()]
        guard buffered.count + data.count <= Self.maximumResponseBytes else {
            responseDataByTaskID.removeValue(forKey: taskID)
            oversizedResponseTaskIDs.insert(taskID)
            return
        }
        buffered.append(data)
        responseDataByTaskID[taskID] = buffered
    }

    func urlSession(
        _: URLSession,
        task: URLSessionTask,
        didCompleteWithError error: Error?
    ) {
        let taskID = task.taskIdentifier
        var waiters: [Waiter] = []
        var description: String?
        var result: Result<MessagingBackgroundUploadResponse, Error>

        lock.lock()
        description = task.taskDescription ?? taskDescriptionByTaskID[taskID]
        let taskGeneration = taskGenerationByTaskID.removeValue(forKey: taskID) ?? 0
        let data = responseDataByTaskID.removeValue(forKey: taskID) ?? Data()
        let responseWasOversized = oversizedResponseTaskIDs.remove(taskID) != nil
        taskDescriptionByTaskID.removeValue(forKey: taskID)
        if let description,
           activeTaskIDByDescription[description] == taskID
        {
            activeTaskIDByDescription.removeValue(forKey: description)
        }
        waiters = waitersByTaskID.removeValue(forKey: taskID) ?? []

        if let error {
            result = .failure(error)
        } else if responseWasOversized {
            result = .failure(MessagingBackgroundTransportError.responseTooLarge)
        } else if let response = task.response as? HTTPURLResponse {
            result = .success(MessagingBackgroundUploadResponse(
                data: data,
                response: response
            ))
        } else {
            result = .failure(MessagingBackgroundTransportError.invalidResponse)
        }
        if waiters.isEmpty,
           let description,
           !isPurgingArtifacts,
           taskGeneration == cleanupGeneration
        {
            storeCompletedResult(result, description: description)
        }
        if taskGeneration == cleanupGeneration,
           let description,
           let fileURL = uploadFileURL(description: description)
        {
            // Upload preparation uses this same lock. Keep the generation check
            // and deletion in one artifact-domain critical section so an old
            // callback cannot delete a post-login body with the same digest.
            do {
                try artifactRemover(fileURL)
            } catch where !fileManager.fileExists(atPath: fileURL.path) {
                // A missing file is already the desired state.
            } catch {
                artifactPurgeFailed = true
            }
        }
        lock.unlock()
        for waiter in waiters {
            waiter.continuation.resume(with: result)
        }
    }

    func urlSessionDidFinishEvents(forBackgroundURLSession _: URLSession) {
        let handlers = MessagingBackgroundEventCompletionBroker.shared.drain()
        guard !handlers.isEmpty else { return }
        DispatchQueue.main.async {
            handlers.forEach { $0() }
        }
    }
}
