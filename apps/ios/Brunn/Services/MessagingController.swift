import Combine
import Foundation

enum MessagingTransportError: Error, LocalizedError {
    case operationNotConfigured

    var errorDescription: String? {
        "The messaging transport operation is not configured."
    }
}

enum MessagingControllerError: Error, LocalizedError {
    case inboxPaginationDidNotAdvance
    case inboxPaginationLimitExceeded
    case threadPaginationDidNotAdvance
    case threadPaginationLimitExceeded
    case continuationChainLimitExceeded

    var errorDescription: String? {
        switch self {
        case .inboxPaginationDidNotAdvance:
            "The message inbox page did not advance."
        case .inboxPaginationLimitExceeded:
            "The message inbox exceeded its bounded catch-up window."
        case .threadPaginationDidNotAdvance:
            "The message thread page did not advance."
        case .threadPaginationLimitExceeded:
            "The message thread exceeded its bounded catch-up window."
        case .continuationChainLimitExceeded:
            "The linked message thread is too long to load safely."
        }
    }
}

struct MessagingTransportOperations: Sendable {
    let sync: @Sendable (MessagingSyncRequest) async throws -> MessagingSyncResponse
    let send: @Sendable (String, Data) async throws -> MessagingSendResponse
    let createConversation: @Sendable (
        MessagingCreateConversationRequest
    ) async throws -> MessagingCreateConversationResponse
    let markRead: @Sendable (String, MessagingReadRequest) async throws -> MessagingReadResponse

    init(
        sync: @escaping @Sendable (MessagingSyncRequest) async throws -> MessagingSyncResponse,
        send: @escaping @Sendable (String, Data) async throws -> MessagingSendResponse,
        createConversation: @escaping @Sendable (
            MessagingCreateConversationRequest
        ) async throws -> MessagingCreateConversationResponse = { _ in
            throw MessagingTransportError.operationNotConfigured
        },
        markRead: @escaping @Sendable (
            String,
            MessagingReadRequest
        ) async throws -> MessagingReadResponse = { _, _ in
            throw MessagingTransportError.operationNotConfigured
        }
    ) {
        self.sync = sync
        self.send = send
        self.createConversation = createConversation
        self.markRead = markRead
    }
}

enum MessagingSyncTrigger: Sendable, Equatable {
    case launch
    case foreground
    case pullToRefresh
    case thread(conversationID: String, waitSeconds: Int)
    case notificationPush

    var conversationID: String? {
        if case let .thread(conversationID, _) = self { conversationID } else { nil }
    }

    var waitSeconds: Int {
        if case let .thread(_, waitSeconds) = self { waitSeconds } else { 0 }
    }

    var isThread: Bool { conversationID != nil }
}

@MainActor
final class MessagingController: ObservableObject {
    @Published private(set) var conversations: [MessagingConversationRecord] = []
    @Published private(set) var agents: [MessagingAgentRecord] = []
    @Published private(set) var selectedMessages: [MessagingMessageRecord] = []
    @Published private(set) var activeAccountID: String?
    @Published private(set) var inboxCursor: Int64 = 0
    @Published private(set) var messagingEnabled = false
    @Published private(set) var isSyncing = false
    @Published private(set) var isMutating = false
    @Published private(set) var lastTransportError: String?
    @Published private(set) var selectedConversationID: String?
    @Published private(set) var selectedActiveConversationID: String?

    private let store: MessagingStore
    private let transport: MessagingTransportOperations
    private let clientKeyGenerator: () -> String
    private let now: () -> Date

    init(
        store: MessagingStore,
        transport: MessagingTransportOperations,
        clientKeyGenerator: @escaping () -> String = { MessagingClientKey.generate() },
        now: @escaping () -> Date = { .now }
    ) {
        self.store = store
        self.transport = transport
        self.clientKeyGenerator = clientKeyGenerator
        self.now = now
        activeAccountID = store.activeAccountID
        try? loadCache()
    }

    @discardableResult
    func activateCachedSession(sessionFingerprint: String) throws -> Bool {
        let activated = try store.activateCachedSession(sessionFingerprint: sessionFingerprint)
        activeAccountID = store.activeAccountID
        try loadCache()
        return activated
    }

    func bindValidatedAccount(accountID: String, sessionFingerprint: String) throws {
        try store.bindValidatedAccount(
            accountID: accountID,
            sessionFingerprint: sessionFingerprint
        )
        activeAccountID = store.activeAccountID
        try loadCache()
    }

    func deactivate() {
        store.deactivate()
        activeAccountID = nil
        conversations = []
        agents = []
        selectedMessages = []
        selectedConversationID = nil
        selectedActiveConversationID = nil
        inboxCursor = 0
        messagingEnabled = false
        lastTransportError = nil
    }

    func clearActiveAccount() throws {
        try store.clearActiveAccount()
        deactivate()
    }

    func selectConversation(_ conversationID: String?) throws {
        selectedConversationID = conversationID
        selectedMessages = try conversationID.map(
            store.logicalThreadMessages(containing:)
        ) ?? []
        selectedActiveConversationID = try conversationID.map(
            store.activeConversationID(containing:)
        )
    }

    func setMessagingEnabled(_ enabled: Bool) throws {
        try store.setMessagingEnabled(enabled)
        messagingEnabled = enabled
    }

    func messages(conversationID: String) throws -> [MessagingMessageRecord] {
        try store.logicalThreadMessages(containing: conversationID)
    }

    func activeConversationID(containing conversationID: String) throws -> String {
        try store.activeConversationID(containing: conversationID)
    }

    func conversationChain(containing conversationID: String) throws -> [String] {
        try store.logicalConversationIDs(containing: conversationID)
    }

    func logicalRootConversationID(for physicalConversationID: String) throws -> String {
        try store.logicalRootConversationID(for: physicalConversationID)
    }

    @discardableResult
    func refreshInbox() async throws -> MessagingSyncResponse {
        try await refresh(.pullToRefresh)
    }

    @discardableResult
    func refreshThread(
        conversationID: String,
        waitSeconds: Int = 0
    ) async throws -> MessagingSyncResponse {
        try await refresh(.thread(
            conversationID: conversationID,
            waitSeconds: waitSeconds
        ))
    }

    /// Launch, foreground, pull-to-refresh, thread polling, and push-prefetch all
    /// converge here so cursor and reconciliation behavior cannot drift.
    @discardableResult
    func refresh(_ trigger: MessagingSyncTrigger) async throws -> MessagingSyncResponse {
        isSyncing = true
        defer { isSyncing = false }
        do {
            let response: MessagingSyncResponse
            if let conversationID = trigger.conversationID {
                response = try await refreshLogicalThread(
                    containing: conversationID,
                    waitSeconds: trigger.waitSeconds
                )
                selectedConversationID = conversationID
            } else {
                response = try await drainInbox()
            }
            try loadCache()
            lastTransportError = nil
            return response
        } catch {
            lastTransportError = error.localizedDescription
            throw error
        }
    }

    /// Persists the ULID and exact request bytes before the transport is called.
    /// A transport failure is an ambiguous result, so the durable state is queued.
    @discardableResult
    func send(
        conversationID: String,
        senderAgentID: String,
        kind: MessagingWritableMessageKind,
        bodyMarkdown: String,
        refs: [MessagingReference] = [],
        inReplyTo: Int64? = nil,
        correlationID: String? = nil,
        expectsReply: Bool = false,
        replyBy: String? = nil
    ) async throws -> MessagingMessageRecord {
        isMutating = true
        defer { isMutating = false }
        let clientKey = clientKeyGenerator()
        guard MessagingClientKey.isValid(clientKey) else {
            throw MessagingStoreError.corruptStoredValue
        }
        let request = MessagingSendRequest(
            clientKey: clientKey,
            kind: kind,
            bodyMarkdown: bodyMarkdown,
            refs: refs,
            inReplyTo: inReplyTo,
            correlationID: correlationID,
            expectsReply: expectsReply,
            replyBy: replyBy
        )
        let exactRequestData = try request.encodedData()
        let record = try store.enqueueOptimisticMessage(
            conversationID: conversationID,
            senderAgentID: senderAgentID,
            request: request,
            exactRequestData: exactRequestData,
            createdAt: Self.timestamp(now())
        )
        selectedConversationID = conversationID
        try loadCache()

        do {
            let response = try await transport.send(conversationID, exactRequestData)
            try store.applySendAcknowledgement(
                response,
                requestedConversationID: conversationID
            )
            lastTransportError = nil
        } catch {
            let retryAt = now().addingTimeInterval(Self.retryDelay(
                afterAttemptCount: record.attemptCount
            ))
            try store.markQueued(record, nextAttemptAt: retryAt)
            lastTransportError = error.localizedDescription
        }
        try loadCache()
        return try store.messages(conversationID: conversationID)
            .first(where: { $0.clientKey == clientKey }) ?? record
    }

    @discardableResult
    func createConversation(
        participants: [String],
        subject: String? = nil
    ) async throws -> MessagingConversationRecord {
        isMutating = true
        defer { isMutating = false }
        do {
            let response = try await transport.createConversation(
                MessagingCreateConversationRequest(
                    participants: participants,
                    subject: subject
                )
            )
            try store.applyCreateAcknowledgement(
                response,
                asOf: Self.timestamp(now())
            )
            selectedConversationID = response.conversationID
            try loadCache()
            lastTransportError = nil
            guard let conversation = conversations.first(where: {
                $0.conversationID == response.conversationID
            }) else {
                throw MessagingStoreError.corruptStoredValue
            }
            return conversation
        } catch {
            lastTransportError = error.localizedDescription
            throw error
        }
    }

    func markRead(conversationID: String, lastReadSequence: Int64) async throws {
        isMutating = true
        defer { isMutating = false }
        do {
            let response = try await transport.markRead(
                conversationID,
                MessagingReadRequest(lastReadSeq: lastReadSequence)
            )
            try store.applyReadAcknowledgement(response)
            try loadCache()
            lastTransportError = nil
        } catch {
            lastTransportError = error.localizedDescription
            throw error
        }
    }

    func markRead(through message: MessagingMessageRecord) async throws {
        guard message.sequence > 0 else { return }
        try await markRead(
            conversationID: message.conversationID,
            lastReadSequence: message.sequence
        )
    }

    /// Called from launch/foreground/connectivity opportunities. URLSession's
    /// existing waitsForConnectivity behavior remains the only connectivity wait.
    func flushOutbox(maximumMessages: Int = 20) async throws {
        let boundedLimit = min(max(maximumMessages, 1), 50)
        // A freshly composed message is already `.sending` while its transport
        // awaits an acknowledgement. MainActor methods are reentrant at that
        // await, so a foreground refresh must not submit the same exact bytes a
        // second time. Relaunch recovery converts interrupted sends to `.queued`.
        let due = try Array(store.pendingOutbox(now: now())
            .filter { $0.deliveryState == .queued }
            .prefix(boundedLimit))
        for record in due {
            guard let exactRequestData = record.exactRequestData else {
                throw MessagingStoreError.corruptStoredValue
            }
            try store.markSending(record)
            try loadCache()
            do {
                let response = try await transport.send(record.conversationID, exactRequestData)
                try store.applySendAcknowledgement(
                    response,
                    requestedConversationID: record.conversationID
                )
                lastTransportError = nil
            } catch {
                let retryAt = now().addingTimeInterval(Self.retryDelay(
                    afterAttemptCount: record.attemptCount
                ))
                try store.markQueued(record, nextAttemptAt: retryAt)
                lastTransportError = error.localizedDescription
            }
            try loadCache()
        }
    }

    func loadCache() throws {
        conversations = try store.conversations()
        agents = try store.agents()
        inboxCursor = try store.inboxCursor()
        messagingEnabled = try store.isMessagingEnabled()
        selectedMessages = try selectedConversationID
            .map(store.logicalThreadMessages(containing:)) ?? []
        selectedActiveConversationID = try selectedConversationID
            .map(store.activeConversationID(containing:))
    }

    private func refreshLogicalThread(
        containing selectedConversationID: String,
        waitSeconds: Int
    ) async throws -> MessagingSyncResponse {
        var pending = try store.logicalConversationIDs(containing: selectedConversationID)
        if pending.isEmpty { pending = [selectedConversationID] }
        var visited = Set<String>()
        var aggregateResponse: MessagingSyncResponse?
        var index = 0

        while index < pending.count {
            guard pending.count <= 64 else {
                throw MessagingControllerError.continuationChainLimitExceeded
            }
            let conversationID = pending[index]
            index += 1
            guard visited.insert(conversationID).inserted else { continue }
            let response = try await drainThreadConversation(
                conversationID: conversationID,
                initialWaitSeconds: 0
            )
            aggregateResponse = Self.merging(aggregateResponse, with: response)
            try loadCache()
            let discovered = try store.logicalConversationIDs(
                containing: selectedConversationID
            )
            for candidate in discovered
                where !visited.contains(candidate) && !pending.contains(candidate)
            {
                pending.append(candidate)
            }
        }

        if waitSeconds > 0 {
            let linked = try store.logicalConversationIDs(containing: selectedConversationID)
            let activeTail = linked.last ?? selectedConversationID
            let response = try await drainThreadConversation(
                conversationID: activeTail,
                initialWaitSeconds: waitSeconds
            )
            aggregateResponse = Self.merging(aggregateResponse, with: response)
            try loadCache()

            // A boundary message can create a continuation during the wait.
            // Fetch newly discovered members immediately; never stack another
            // long poll in the same refresh call.
            let discovered = try store.logicalConversationIDs(
                containing: selectedConversationID
            )
            for candidate in discovered where !visited.contains(candidate) {
                guard visited.count < 64 else {
                    throw MessagingControllerError.continuationChainLimitExceeded
                }
                visited.insert(candidate)
                let response = try await drainThreadConversation(
                    conversationID: candidate,
                    initialWaitSeconds: 0
                )
                aggregateResponse = Self.merging(aggregateResponse, with: response)
                try loadCache()
            }
        }

        guard let aggregateResponse else {
            throw MessagingControllerError.threadPaginationDidNotAdvance
        }
        return aggregateResponse
    }

    private func drainInbox() async throws -> MessagingSyncResponse {
        var cursor = try store.inboxCursor()
        var aggregateResponse: MessagingSyncResponse?

        for _ in 0 ..< 100 {
            let response = try await transport.sync(MessagingSyncRequest(
                cursor: cursor,
                waitSeconds: 0,
                limit: 200
            ))
            try store.applyInboxDelta(response)
            try loadCache()
            aggregateResponse = Self.merging(aggregateResponse, with: response)

            if !response.hasMore {
                return aggregateResponse ?? response
            }
            guard response.cursor > cursor else {
                throw MessagingControllerError.inboxPaginationDidNotAdvance
            }
            cursor = response.cursor
        }
        throw MessagingControllerError.inboxPaginationLimitExceeded
    }

    private func drainThreadConversation(
        conversationID: String,
        initialWaitSeconds: Int
    ) async throws -> MessagingSyncResponse {
        var afterSequence = try store.lastContiguousSequence(
            conversationID: conversationID
        )
        var waitSeconds = initialWaitSeconds
        var aggregateResponse: MessagingSyncResponse?
        for _ in 0 ..< 100 {
            let response = try await transport.sync(MessagingSyncRequest(
                cursor: 0,
                waitSeconds: waitSeconds,
                conversationID: conversationID,
                afterSequence: afterSequence,
                limit: 200
            ))
            try store.applyThreadDelta(response)
            try loadCache()
            aggregateResponse = Self.merging(aggregateResponse, with: response)
            guard response.hasMore else { return aggregateResponse ?? response }
            let nextSequence = response.messages
                .filter { $0.conversationID == conversationID }
                .map(\.sequence)
                .max() ?? afterSequence
            guard nextSequence > afterSequence else {
                throw MessagingControllerError.threadPaginationDidNotAdvance
            }
            afterSequence = nextSequence
            waitSeconds = 0
        }
        throw MessagingControllerError.threadPaginationLimitExceeded
    }

    private static func merging(
        _ current: MessagingSyncResponse?,
        with latest: MessagingSyncResponse
    ) -> MessagingSyncResponse {
        guard let current else { return latest }
        var messages = current.messages
        var messagePositions = Dictionary(
            uniqueKeysWithValues: messages.enumerated().map {
                ("\($0.element.conversationID)\u{1f}\($0.element.sequence)", $0.offset)
            }
        )
        for message in latest.messages {
            let key = "\(message.conversationID)\u{1f}\(message.sequence)"
            if let position = messagePositions[key] {
                messages[position] = message
            } else {
                messagePositions[key] = messages.count
                messages.append(message)
            }
        }

        var conversations = current.conversations
        var conversationPositions = Dictionary(
            uniqueKeysWithValues: conversations.enumerated().map {
                ($0.element.conversationID, $0.offset)
            }
        )
        for conversation in latest.conversations {
            if let position = conversationPositions[conversation.conversationID] {
                conversations[position] = conversation
            } else {
                conversationPositions[conversation.conversationID] = conversations.count
                conversations.append(conversation)
            }
        }
        var unread = current.unread
        unread.merge(latest.unread) { _, value in value }
        return MessagingSyncResponse(
            status: latest.status,
            cursor: latest.cursor,
            resumeCursor: latest.resumeCursor,
            hasMore: latest.hasMore,
            messages: messages,
            conversations: conversations,
            presence: latest.presence,
            unread: unread,
            asOf: latest.asOf
        )
    }

    private static func retryDelay(afterAttemptCount attemptCount: Int) -> TimeInterval {
        let exponent = min(max(attemptCount, 0), 6)
        return min(TimeInterval(1 << exponent), 60)
    }

    private static func timestamp(_ date: Date) -> String {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return formatter.string(from: date)
    }
}
