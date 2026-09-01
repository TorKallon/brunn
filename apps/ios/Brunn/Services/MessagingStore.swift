import Foundation
import SwiftData

enum MessagingDeliveryState: String, Codable, Sendable, Equatable {
    case sending
    case queued
    case canonical
}

enum MessagingStoreError: Error, LocalizedError, Equatable {
    case inactiveAccount
    case invalidAccountBinding
    case ambiguousAccountBinding
    case idempotencyConflict
    case corruptStoredValue
    case invalidStoreNamespace

    var errorDescription: String? {
        switch self {
        case .inactiveAccount:
            "The protected message cache is not bound to this account."
        case .invalidAccountBinding:
            "The account or authenticated-session binding is invalid."
        case .ambiguousAccountBinding:
            "The authenticated session matches more than one cached account."
        case .idempotencyConflict:
            "The message key was already used with a different request."
        case .corruptStoredValue:
            "The protected message cache contains an invalid value."
        case .invalidStoreNamespace:
            "The message store namespace is invalid."
        }
    }
}

@Model
final class MessagingConversationRecord {
    @Attribute(.unique) var storageKey: String
    var accountID: String
    var conversationID: String
    var conversationKind: String
    var subject: String?
    var status: String
    var participantsData: Data
    var lastSeq: Int64
    var lastMessageAt: String?
    var lastReadSeq: Int64
    var unreadCount: Int64
    var needsHuman: Bool
    var continuesFrom: String?
    var continuationID: String?
    var latestSyncCursor: Int64

    var participants: [MessagingParticipant] {
        (try? JSONDecoder().decode([MessagingParticipant].self, from: participantsData)) ?? []
    }

    init(accountID: String, value: MessagingConversation, participantsData: Data) {
        storageKey = Self.key(accountID: accountID, conversationID: value.conversationID)
        self.accountID = accountID
        conversationID = value.conversationID
        conversationKind = value.conversationKind
        subject = value.subject
        status = value.status
        self.participantsData = participantsData
        lastSeq = value.lastSeq
        lastMessageAt = value.lastMessageAt
        lastReadSeq = value.lastReadSeq
        unreadCount = value.unreadCount
        needsHuman = value.needsHuman
        continuesFrom = value.continuesFrom
        continuationID = value.continuationID
        latestSyncCursor = value.latestSyncCursor
    }

    static func key(accountID: String, conversationID: String) -> String {
        "\(accountID)\u{1f}conversation\u{1f}\(conversationID)"
    }
}

@Model
final class MessagingMessageRecord {
    @Attribute(.unique) var storageKey: String
    var accountID: String
    var conversationID: String
    var sequence: Int64
    var messageID: String?
    var fromAgentID: String?
    var clientKey: String?
    var kind: String
    var bodyMarkdown: String
    var refsData: Data
    var inReplyToConversationID: String?
    var inReplyTo: Int64?
    var correlationID: String?
    var expectsReply: Bool
    var replyBy: String?
    var syncCursor: Int64
    var createdAt: String
    var deliveryStateRaw: String
    var exactRequestData: Data?
    var attemptCount: Int
    var nextAttemptAt: Date?

    var deliveryState: MessagingDeliveryState {
        get { MessagingDeliveryState(rawValue: deliveryStateRaw) ?? .queued }
        set { deliveryStateRaw = newValue.rawValue }
    }

    var refs: [MessagingReference] {
        (try? JSONDecoder().decode([MessagingReference].self, from: refsData)) ?? []
    }

    /// Stable wire identity. Sequence numbers restart in continuations, so a
    /// logical-thread UI must never identify a canonical row by sequence alone.
    var wireIdentity: String {
        if sequence > 0 {
            return "\(conversationID):\(sequence)"
        }
        return "\(conversationID):outbox:\(clientKey ?? storageKey)"
    }

    init(
        storageKey: String,
        accountID: String,
        conversationID: String,
        sequence: Int64,
        messageID: String?,
        fromAgentID: String?,
        clientKey: String?,
        kind: String,
        bodyMarkdown: String,
        refsData: Data,
        inReplyToConversationID: String?,
        inReplyTo: Int64?,
        correlationID: String?,
        expectsReply: Bool,
        replyBy: String?,
        syncCursor: Int64,
        createdAt: String,
        deliveryState: MessagingDeliveryState,
        exactRequestData: Data?,
        attemptCount: Int = 0,
        nextAttemptAt: Date? = nil
    ) {
        self.storageKey = storageKey
        self.accountID = accountID
        self.conversationID = conversationID
        self.sequence = sequence
        self.messageID = messageID
        self.fromAgentID = fromAgentID
        self.clientKey = clientKey
        self.kind = kind
        self.bodyMarkdown = bodyMarkdown
        self.refsData = refsData
        self.inReplyToConversationID = inReplyToConversationID
        self.inReplyTo = inReplyTo
        self.correlationID = correlationID
        self.expectsReply = expectsReply
        self.replyBy = replyBy
        self.syncCursor = syncCursor
        self.createdAt = createdAt
        deliveryStateRaw = deliveryState.rawValue
        self.exactRequestData = exactRequestData
        self.attemptCount = attemptCount
        self.nextAttemptAt = nextAttemptAt
    }

    static func key(
        accountID: String,
        messageID: String,
        fromAgentID: String?,
        clientKey: String?
    ) -> String {
        if let fromAgentID, let clientKey {
            return "\(accountID)\u{1f}client\u{1f}\(fromAgentID)\u{1f}\(clientKey)"
        }
        return "\(accountID)\u{1f}message\u{1f}\(messageID)"
    }
}

@Model
final class MessagingAgentRecord {
    @Attribute(.unique) var storageKey: String
    var accountID: String
    var agentID: String
    var displayName: String
    var principalKind: String
    var deliveryMode: String
    var online: Bool
    var lastSeenAt: String?
    var leaseExpiresAt: String?
    var archived: Bool
    var credentialNamesData: Data?

    var credentialNames: [String]? {
        guard let credentialNamesData else { return nil }
        return try? JSONDecoder().decode([String].self, from: credentialNamesData)
    }

    init(accountID: String, value: MessagingAgent, credentialNamesData: Data?) {
        storageKey = Self.key(accountID: accountID, agentID: value.agentID)
        self.accountID = accountID
        agentID = value.agentID
        displayName = value.displayName
        principalKind = value.principalKind
        deliveryMode = value.deliveryMode
        online = value.online
        lastSeenAt = value.lastSeenAt
        leaseExpiresAt = value.leaseExpiresAt
        archived = value.archived
        self.credentialNamesData = credentialNamesData
    }

    static func key(accountID: String, agentID: String) -> String {
        "\(accountID)\u{1f}agent\u{1f}\(agentID)"
    }
}

@Model
final class MessagingAccountState {
    @Attribute(.unique) var accountID: String
    var sessionFingerprint: String
    var inboxCursor: Int64
    var messagingEnabled: Bool
    var updatedAt: Date

    init(
        accountID: String,
        sessionFingerprint: String,
        inboxCursor: Int64 = 0,
        messagingEnabled: Bool = false,
        updatedAt: Date = .now
    ) {
        self.accountID = accountID
        self.sessionFingerprint = sessionFingerprint
        self.inboxCursor = inboxCursor
        self.messagingEnabled = messagingEnabled
        self.updatedAt = updatedAt
    }
}

@MainActor
final class MessagingStore {
    static let namespaceEnvironmentKey = "BRUNN_MESSAGING_STORE_NAMESPACE"
    static let fileProtection: FileProtectionType = .completeUntilFirstUserAuthentication
    static let modelTypes: [any PersistentModel.Type] = [
        MessagingConversationRecord.self,
        MessagingMessageRecord.self,
        MessagingAgentRecord.self,
        MessagingAccountState.self,
    ]
    static var persistentModelCount: Int { modelTypes.count }

    let storeURL: URL
    let fileProtectionType = MessagingStore.fileProtection
    private(set) var saveCount = 0
    private(set) var activeAccountID: String?

    var persistentModelCount: Int { container.schema.entities.count }

    private let fileManager: FileManager
    private let isStoredInMemoryOnly: Bool
    private let container: ModelContainer
    private let context: ModelContext

    init(
        storeURL: URL? = nil,
        isStoredInMemoryOnly: Bool = false,
        fileManager: FileManager = .default,
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) throws {
        self.fileManager = fileManager
        self.isStoredInMemoryOnly = isStoredInMemoryOnly
        let resolvedURL = try storeURL ?? Self.defaultStoreURL(
            fileManager: fileManager,
            environment: environment
        )
        self.storeURL = resolvedURL
        let schema = Schema(Self.modelTypes)
        let configuration: ModelConfiguration
        if isStoredInMemoryOnly {
            configuration = ModelConfiguration(
                "AgentMessaging",
                schema: schema,
                isStoredInMemoryOnly: true,
                allowsSave: true,
                cloudKitDatabase: .none
            )
        } else {
            try Self.prepareStoreDirectory(for: resolvedURL, fileManager: fileManager)
            configuration = ModelConfiguration(
                "AgentMessaging",
                schema: schema,
                url: resolvedURL,
                allowsSave: true,
                cloudKitDatabase: .none
            )
        }
        container = try ModelContainer(for: schema, configurations: [configuration])
        context = ModelContext(container)
        context.autosaveEnabled = false
        if !isStoredInMemoryOnly {
            try protectStoreFiles()
        }
    }

    static func defaultStoreURL(
        fileManager: FileManager = .default,
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) throws -> URL {
        guard let root = fileManager.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        ).first else {
            throw CocoaError(.fileNoSuchFile)
        }
        return try storeURL(applicationSupportRoot: root, environment: environment)
    }

    static func storeURL(
        applicationSupportRoot: URL,
        environment: [String: String]
    ) throws -> URL {
        var directory = applicationSupportRoot
            .appendingPathComponent("Brunn", isDirectory: true)
            .appendingPathComponent("AgentMessaging", isDirectory: true)
#if DEBUG
        if let rawNamespace = environment[namespaceEnvironmentKey] {
            guard let namespace = validatedStoreNamespace(rawNamespace) else {
                throw MessagingStoreError.invalidStoreNamespace
            }
            directory.appendPathComponent(namespace, isDirectory: true)
        }
#endif
        return directory.appendingPathComponent("messaging.sqlite", isDirectory: false)
    }

    static func validatedStoreNamespace(_ value: String) -> String? {
        guard (1 ... 64).contains(value.utf8.count), value.count == value.utf8.count else {
            return nil
        }
        let allowed = Set(
            "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_"
        )
        guard value.allSatisfy(allowed.contains) else { return nil }
        return value
    }

    @discardableResult
    func activateCachedSession(sessionFingerprint: String) throws -> Bool {
        guard Self.isValidFingerprint(sessionFingerprint) else {
            activeAccountID = nil
            throw MessagingStoreError.invalidAccountBinding
        }
        let matches = try context.fetch(FetchDescriptor<MessagingAccountState>())
            .filter { $0.sessionFingerprint == sessionFingerprint }
        guard matches.count <= 1 else {
            activeAccountID = nil
            throw MessagingStoreError.ambiguousAccountBinding
        }
        guard let match = matches.first else {
            activeAccountID = nil
            return false
        }
        activeAccountID = match.accountID
        try recoverSendingOutboxForActiveAccount()
        return true
    }

    func bindValidatedAccount(accountID: String, sessionFingerprint: String) throws {
        guard Self.isValidAccountID(accountID),
              Self.isValidFingerprint(sessionFingerprint)
        else {
            activeAccountID = nil
            throw MessagingStoreError.invalidAccountBinding
        }
        let states = try context.fetch(FetchDescriptor<MessagingAccountState>())
        if states.contains(where: {
            $0.sessionFingerprint == sessionFingerprint && $0.accountID != accountID
        }) {
            activeAccountID = nil
            throw MessagingStoreError.ambiguousAccountBinding
        }
        let state: MessagingAccountState
        if let existing = states.first(where: { $0.accountID == accountID }) {
            state = existing
            state.sessionFingerprint = sessionFingerprint
            state.updatedAt = .now
        } else {
            state = MessagingAccountState(
                accountID: accountID,
                sessionFingerprint: sessionFingerprint
            )
            context.insert(state)
        }
        activeAccountID = accountID
        for message in try allMessages(for: accountID)
            where message.deliveryState == .sending
        {
            message.deliveryState = .queued
            message.nextAttemptAt = nil
        }
        try saveChanges()
    }

    func deactivate() {
        activeAccountID = nil
        context.rollback()
    }

    func clearActiveAccount() throws {
        guard let accountID = activeAccountID else { return }
        do {
            for record in try context.fetch(FetchDescriptor<MessagingConversationRecord>())
                where record.accountID == accountID
            {
                context.delete(record)
            }
            for record in try context.fetch(FetchDescriptor<MessagingMessageRecord>())
                where record.accountID == accountID
            {
                context.delete(record)
            }
            for record in try context.fetch(FetchDescriptor<MessagingAgentRecord>())
                where record.accountID == accountID
            {
                context.delete(record)
            }
            for record in try context.fetch(FetchDescriptor<MessagingAccountState>())
                where record.accountID == accountID
            {
                context.delete(record)
            }
            try saveChanges()
            activeAccountID = nil
        } catch {
            context.rollback()
            throw error
        }
    }

    func conversations() throws -> [MessagingConversationRecord] {
        guard let accountID = activeAccountID else { return [] }
        return try context.fetch(FetchDescriptor<MessagingConversationRecord>())
            .filter { $0.accountID == accountID }
            .sorted {
                if $0.lastMessageAt == $1.lastMessageAt {
                    return $0.conversationID < $1.conversationID
                }
                return ($0.lastMessageAt ?? "") > ($1.lastMessageAt ?? "")
            }
    }

    func messages(conversationID: String) throws -> [MessagingMessageRecord] {
        guard let accountID = activeAccountID else { return [] }
        return try allMessages(for: accountID)
            .filter { $0.conversationID == conversationID }
            .sorted {
                switch ($0.sequence > 0, $1.sequence > 0) {
                case (true, true):
                    return $0.sequence == $1.sequence
                        ? $0.createdAt < $1.createdAt
                        : $0.sequence < $1.sequence
                case (true, false):
                    return true
                case (false, true):
                    return false
                case (false, false):
                    return $0.createdAt < $1.createdAt
                }
            }
    }

    func logicalThreadMessages(
        containing conversationID: String
    ) throws -> [MessagingMessageRecord] {
        guard let accountID = activeAccountID else { return [] }
        let conversationIDs = try logicalConversationIDs(containing: conversationID)
        let positions = Dictionary(
            uniqueKeysWithValues: conversationIDs.enumerated().map { ($0.element, $0.offset) }
        )
        return try allMessages(for: accountID)
            .filter { positions[$0.conversationID] != nil }
            .sorted { left, right in
                let leftPosition = positions[left.conversationID] ?? .max
                let rightPosition = positions[right.conversationID] ?? .max
                if leftPosition != rightPosition { return leftPosition < rightPosition }
                switch (left.sequence > 0, right.sequence > 0) {
                case (true, true):
                    return left.sequence == right.sequence
                        ? left.createdAt < right.createdAt
                        : left.sequence < right.sequence
                case (true, false):
                    return true
                case (false, true):
                    return false
                case (false, false):
                    return left.createdAt == right.createdAt
                        ? left.storageKey < right.storageKey
                        : left.createdAt < right.createdAt
                }
            }
    }

    func logicalConversationIDs(containing conversationID: String) throws -> [String] {
        guard let accountID = activeAccountID else { return [] }
        let conversations = try context.fetch(FetchDescriptor<MessagingConversationRecord>())
            .filter { $0.accountID == accountID }
        let byID = Dictionary(
            uniqueKeysWithValues: conversations.map { ($0.conversationID, $0) }
        )
        var childrenByParent: [String: [String]] = [:]
        var parentByChild: [String: String] = [:]
        for conversation in conversations {
            if let parent = conversation.continuesFrom {
                childrenByParent[parent, default: []].append(conversation.conversationID)
                if let existing = parentByChild[conversation.conversationID],
                   existing != parent
                {
                    throw MessagingStoreError.corruptStoredValue
                }
                parentByChild[conversation.conversationID] = parent
            }
            if let child = conversation.continuationID {
                childrenByParent[conversation.conversationID, default: []].append(child)
                if let existing = parentByChild[child],
                   existing != conversation.conversationID
                {
                    throw MessagingStoreError.corruptStoredValue
                }
                parentByChild[child] = conversation.conversationID
            }
        }

        var root = conversationID
        var backwardSeen = Set<String>()
        for _ in 0 ..< 64 {
            guard backwardSeen.insert(root).inserted else {
                throw MessagingStoreError.corruptStoredValue
            }
            guard let predecessor = parentByChild[root] else { break }
            root = predecessor
        }
        if parentByChild[root] != nil {
            throw MessagingStoreError.corruptStoredValue
        }

        var ordered: [String] = []
        var forwardSeen = Set<String>()
        var current: String? = root
        for _ in 0 ..< 64 {
            guard let currentID = current else { break }
            guard forwardSeen.insert(currentID).inserted else {
                throw MessagingStoreError.corruptStoredValue
            }
            ordered.append(currentID)
            let inverseChildren = Array(Set(childrenByParent[currentID] ?? [])).sorted()
            let declaredChild = byID[currentID]?.continuationID
            if inverseChildren.count > 1 {
                throw MessagingStoreError.corruptStoredValue
            }
            if let declaredChild,
               let inverseChild = inverseChildren.first,
               declaredChild != inverseChild
            {
                throw MessagingStoreError.corruptStoredValue
            }
            current = declaredChild ?? inverseChildren.first
        }
        if current != nil || !ordered.contains(conversationID) {
            throw MessagingStoreError.corruptStoredValue
        }
        return ordered
    }

    func activeConversationID(containing conversationID: String) throws -> String {
        try logicalConversationIDs(containing: conversationID).last ?? conversationID
    }

    func logicalRootConversationID(for conversationID: String) throws -> String {
        try logicalConversationIDs(containing: conversationID).first ?? conversationID
    }

    func agents() throws -> [MessagingAgentRecord] {
        guard let accountID = activeAccountID else { return [] }
        return try context.fetch(FetchDescriptor<MessagingAgentRecord>())
            .filter { $0.accountID == accountID }
            .sorted {
                if $0.online != $1.online { return $0.online }
                return $0.displayName.localizedCaseInsensitiveCompare($1.displayName)
                    == .orderedAscending
            }
    }

    func inboxCursor() throws -> Int64 {
        guard let accountID = activeAccountID else { return 0 }
        return try accountState(for: accountID).inboxCursor
    }

    func isMessagingEnabled() throws -> Bool {
        guard let accountID = activeAccountID else { return false }
        return try accountState(for: accountID).messagingEnabled
    }

    func setMessagingEnabled(_ enabled: Bool) throws {
        let accountID = try requireActiveAccount()
        let state = try accountState(for: accountID)
        guard state.messagingEnabled != enabled else { return }
        state.messagingEnabled = enabled
        state.updatedAt = .now
        try saveChanges()
    }

    func lastSequence(conversationID: String) throws -> Int64 {
        try messages(conversationID: conversationID).map(\.sequence).max() ?? 0
    }

    /// Thread deltas can safely resume only after a contiguous canonical prefix.
    /// A cold deep link may seed one high sequence before the historical page
    /// drain; treating that row as the watermark would permanently skip the
    /// missing prefix.
    func lastContiguousSequence(conversationID: String) throws -> Int64 {
        var expected: Int64 = 1
        for sequence in try messages(conversationID: conversationID)
            .map(\.sequence)
            .filter({ $0 > 0 })
        {
            if sequence < expected { continue }
            guard sequence == expected else { return expected - 1 }
            expected += 1
        }
        return expected - 1
    }

    func pendingOutbox(now: Date = .now) throws -> [MessagingMessageRecord] {
        guard let accountID = activeAccountID else { return [] }
        return try allMessages(for: accountID)
            .filter {
                $0.deliveryState != .canonical
                    && ($0.nextAttemptAt == nil || $0.nextAttemptAt! <= now)
            }
            .sorted {
                if $0.nextAttemptAt == $1.nextAttemptAt {
                    return $0.createdAt < $1.createdAt
                }
                return ($0.nextAttemptAt ?? .distantPast) < ($1.nextAttemptAt ?? .distantPast)
            }
    }

    @discardableResult
    func enqueueOptimisticMessage(
        conversationID: String,
        senderAgentID: String,
        request: MessagingSendRequest,
        exactRequestData: Data,
        createdAt: String
    ) throws -> MessagingMessageRecord {
        let accountID = try requireActiveAccount()
        guard MessagingClientKey.isValid(request.clientKey),
              !senderAgentID.isEmpty,
              !conversationID.isEmpty,
              !exactRequestData.isEmpty
        else {
            throw MessagingStoreError.corruptStoredValue
        }
        let storageKey = MessagingMessageRecord.key(
            accountID: accountID,
            messageID: request.clientKey,
            fromAgentID: senderAgentID,
            clientKey: request.clientKey
        )
        if let existing = try allMessages(for: accountID)
            .first(where: { $0.storageKey == storageKey })
        {
            if existing.deliveryState == .canonical || existing.exactRequestData == exactRequestData {
                return existing
            }
            throw MessagingStoreError.idempotencyConflict
        }
        let refsData = try encode(request.refs)
        let record = MessagingMessageRecord(
            storageKey: storageKey,
            accountID: accountID,
            conversationID: conversationID,
            sequence: 0,
            messageID: nil,
            fromAgentID: senderAgentID,
            clientKey: request.clientKey,
            kind: request.kind.rawValue,
            bodyMarkdown: request.bodyMarkdown,
            refsData: refsData,
            inReplyToConversationID: request.inReplyTo.map { _ in conversationID },
            inReplyTo: request.inReplyTo,
            correlationID: request.correlationID,
            expectsReply: request.expectsReply,
            replyBy: request.replyBy,
            syncCursor: 0,
            createdAt: createdAt,
            deliveryState: .sending,
            exactRequestData: exactRequestData
        )
        context.insert(record)
        try saveChanges()
        return record
    }

    func markSending(_ record: MessagingMessageRecord) throws {
        let accountID = try requireActiveAccount()
        guard record.accountID == accountID, record.deliveryState != .canonical else { return }
        record.deliveryState = .sending
        record.nextAttemptAt = nil
        try saveChanges()
    }

    func markQueued(
        _ record: MessagingMessageRecord,
        nextAttemptAt: Date
    ) throws {
        let accountID = try requireActiveAccount()
        guard record.accountID == accountID, record.deliveryState != .canonical else { return }
        record.deliveryState = .queued
        record.attemptCount += 1
        record.nextAttemptAt = nextAttemptAt
        try saveChanges()
    }

    func applySendAcknowledgement(
        _ response: MessagingSendResponse,
        requestedConversationID: String? = nil
    ) throws {
        var continuationLinks: [(parent: String, child: String)] = []
        if let requestedConversationID,
           requestedConversationID != response.conversationID
        {
            continuationLinks.append((requestedConversationID, response.conversationID))
        }
        if let continuationID = response.continuationID {
            continuationLinks.append((response.conversationID, continuationID))
        }
        try apply(
            MessagingSyncResponse(
                status: "complete",
                cursor: response.message.syncCursor,
                hasMore: false,
                messages: [response.message],
                conversations: [],
                presence: [],
                unread: [:],
                asOf: response.message.createdAt
            ),
            advancesInboxCursor: false,
            replacePresence: false,
            continuationLinks: continuationLinks
        )
    }

    func applyCreateAcknowledgement(
        _ response: MessagingCreateConversationResponse,
        asOf: String
    ) throws {
        try apply(
            MessagingSyncResponse(
                status: "complete",
                cursor: response.conversation.latestSyncCursor,
                hasMore: false,
                messages: [],
                conversations: [response.conversation],
                presence: [],
                unread: [
                    response.conversationID: response.conversation.unreadCount,
                ],
                asOf: asOf
            ),
            advancesInboxCursor: false,
            replacePresence: false
        )
    }

    func applyReadAcknowledgement(_ response: MessagingReadResponse) throws {
        let accountID = try requireActiveAccount()
        do {
            if let conversation = try context.fetch(
                FetchDescriptor<MessagingConversationRecord>()
            ).first(where: {
                $0.accountID == accountID && $0.conversationID == response.conversationID
            }) {
                conversation.lastReadSeq = max(
                    conversation.lastReadSeq,
                    response.lastReadSeq
                )
                conversation.unreadCount = max(
                    conversation.lastSeq - conversation.lastReadSeq,
                    0
                )
                conversation.latestSyncCursor = max(
                    conversation.latestSyncCursor,
                    response.cursor
                )
            }
            try saveChanges()
        } catch {
            context.rollback()
            throw error
        }
    }

    func applyInboxDelta(_ response: MessagingSyncResponse) throws {
        try apply(response, advancesInboxCursor: true, replacePresence: true)
    }

    func applyThreadDelta(_ response: MessagingSyncResponse) throws {
        try apply(response, advancesInboxCursor: false, replacePresence: true)
    }

    private func apply(
        _ response: MessagingSyncResponse,
        advancesInboxCursor: Bool,
        replacePresence: Bool,
        continuationLinks: [(parent: String, child: String)] = []
    ) throws {
        let accountID = try requireActiveAccount()
        do {
            let state = try accountState(for: accountID)
            if advancesInboxCursor, response.cursor > state.inboxCursor {
                state.inboxCursor = response.cursor
                state.updatedAt = .now
            }

            let existingConversations = try context.fetch(
                FetchDescriptor<MessagingConversationRecord>()
            ).filter { $0.accountID == accountID }
            var conversationByID = Dictionary(
                uniqueKeysWithValues: existingConversations.map { ($0.conversationID, $0) }
            )
            for value in response.conversations {
                let participantsData = try encode(value.participants)
                if let record = conversationByID[value.conversationID] {
                    update(record, with: value, participantsData: participantsData)
                } else {
                    let record = MessagingConversationRecord(
                        accountID: accountID,
                        value: value,
                        participantsData: participantsData
                    )
                    context.insert(record)
                    conversationByID[value.conversationID] = record
                }
            }
            for link in continuationLinks where link.parent != link.child {
                guard let parent = conversationByID[link.parent] else { continue }
                if parent.continuationID == nil {
                    parent.continuationID = link.child
                }
            }

            let existingMessages = try allMessages(for: accountID)
            var messageByID: [String: MessagingMessageRecord] = [:]
            var messageBySenderClient: [String: MessagingMessageRecord] = [:]
            for record in existingMessages {
                if let messageID = record.messageID {
                    messageByID[messageID] = record
                }
                if let sender = record.fromAgentID, let clientKey = record.clientKey {
                    messageBySenderClient[Self.senderClientKey(sender, clientKey)] = record
                }
            }
            for value in response.messages {
                let clientMatch = value.fromAgentID.flatMap { sender in
                    value.clientKey.flatMap {
                        messageBySenderClient[Self.senderClientKey(sender, $0)]
                    }
                }
                let idMatch = messageByID[value.messageID]
                let target = clientMatch ?? idMatch
                if let clientMatch, let idMatch, clientMatch !== idMatch {
                    context.delete(idMatch)
                }
                let refsData = try encode(value.refs)
                let record: MessagingMessageRecord
                if let target {
                    record = target
                    update(record, with: value, refsData: refsData)
                } else {
                    record = MessagingMessageRecord(
                        storageKey: MessagingMessageRecord.key(
                            accountID: accountID,
                            messageID: value.messageID,
                            fromAgentID: value.fromAgentID,
                            clientKey: value.clientKey
                        ),
                        accountID: accountID,
                        conversationID: value.conversationID,
                        sequence: value.sequence,
                        messageID: value.messageID,
                        fromAgentID: value.fromAgentID,
                        clientKey: value.clientKey,
                        kind: value.kind,
                        bodyMarkdown: value.bodyMarkdown,
                        refsData: refsData,
                        inReplyToConversationID: value.inReplyToConversationID,
                        inReplyTo: value.inReplyTo,
                        correlationID: value.correlationID,
                        expectsReply: value.expectsReply,
                        replyBy: value.replyBy,
                        syncCursor: value.syncCursor,
                        createdAt: value.createdAt,
                        deliveryState: .canonical,
                        exactRequestData: nil
                    )
                    context.insert(record)
                }
                messageByID[value.messageID] = record
                if let sender = value.fromAgentID, let clientKey = value.clientKey {
                    messageBySenderClient[Self.senderClientKey(sender, clientKey)] = record
                }
            }

            let existingAgents = try context.fetch(FetchDescriptor<MessagingAgentRecord>())
                .filter { $0.accountID == accountID }
            var agentByID = Dictionary(
                uniqueKeysWithValues: existingAgents.map { ($0.agentID, $0) }
            )
            for value in response.presence {
                let credentialNamesData = try value.credentialNames.map(encode)
                if let record = agentByID[value.agentID] {
                    update(record, with: value, credentialNamesData: credentialNamesData)
                } else {
                    let record = MessagingAgentRecord(
                        accountID: accountID,
                        value: value,
                        credentialNamesData: credentialNamesData
                    )
                    context.insert(record)
                    agentByID[value.agentID] = record
                }
            }
            if replacePresence {
                let returnedAgentIDs = Set(response.presence.map(\.agentID))
                for record in existingAgents where !returnedAgentIDs.contains(record.agentID) {
                    context.delete(record)
                }
            }
            try saveChanges()
        } catch {
            context.rollback()
            throw error
        }
    }

    private func recoverSendingOutboxForActiveAccount() throws {
        let accountID = try requireActiveAccount()
        var changed = false
        for message in try allMessages(for: accountID)
            where message.deliveryState == .sending
        {
            message.deliveryState = .queued
            message.nextAttemptAt = nil
            changed = true
        }
        if changed {
            try saveChanges()
        }
    }

    private func accountState(for accountID: String) throws -> MessagingAccountState {
        guard let state = try context.fetch(FetchDescriptor<MessagingAccountState>())
            .first(where: { $0.accountID == accountID })
        else {
            throw MessagingStoreError.invalidAccountBinding
        }
        return state
    }

    private func allMessages(for accountID: String) throws -> [MessagingMessageRecord] {
        try context.fetch(FetchDescriptor<MessagingMessageRecord>())
            .filter { $0.accountID == accountID }
    }

    private func requireActiveAccount() throws -> String {
        guard let activeAccountID else { throw MessagingStoreError.inactiveAccount }
        return activeAccountID
    }

    private func saveChanges() throws {
        guard context.hasChanges else { return }
        do {
            try context.save()
            saveCount += 1
            if !isStoredInMemoryOnly {
                try protectStoreFiles()
            }
        } catch {
            context.rollback()
            throw error
        }
    }

    private func protectStoreFiles() throws {
        let directory = storeURL.deletingLastPathComponent()
        guard fileManager.fileExists(atPath: directory.path) else { return }
        for url in try fileManager.contentsOfDirectory(
            at: directory,
            includingPropertiesForKeys: nil
        ) {
            try? fileManager.setAttributes(
                [.protectionKey: Self.fileProtection],
                ofItemAtPath: url.path
            )
        }
    }

    private func update(
        _ record: MessagingConversationRecord,
        with value: MessagingConversation,
        participantsData: Data
    ) {
        record.conversationKind = value.conversationKind
        record.subject = value.subject
        record.status = value.status
        record.participantsData = participantsData
        record.lastSeq = value.lastSeq
        record.lastMessageAt = value.lastMessageAt
        record.lastReadSeq = value.lastReadSeq
        record.unreadCount = value.unreadCount
        record.needsHuman = value.needsHuman
        record.continuesFrom = value.continuesFrom
        record.continuationID = value.continuationID
        record.latestSyncCursor = value.latestSyncCursor
    }

    private func update(
        _ record: MessagingMessageRecord,
        with value: MessagingMessage,
        refsData: Data
    ) {
        record.conversationID = value.conversationID
        record.sequence = value.sequence
        record.messageID = value.messageID
        record.fromAgentID = value.fromAgentID
        record.clientKey = value.clientKey
        record.kind = value.kind
        record.bodyMarkdown = value.bodyMarkdown
        record.refsData = refsData
        record.inReplyToConversationID = value.inReplyToConversationID
        record.inReplyTo = value.inReplyTo
        record.correlationID = value.correlationID
        record.expectsReply = value.expectsReply
        record.replyBy = value.replyBy
        record.syncCursor = value.syncCursor
        record.createdAt = value.createdAt
        record.deliveryState = .canonical
        record.exactRequestData = nil
        record.nextAttemptAt = nil
    }

    private func update(
        _ record: MessagingAgentRecord,
        with value: MessagingAgent,
        credentialNamesData: Data?
    ) {
        record.displayName = value.displayName
        record.principalKind = value.principalKind
        record.deliveryMode = value.deliveryMode
        record.online = value.online
        record.lastSeenAt = value.lastSeenAt
        record.leaseExpiresAt = value.leaseExpiresAt
        record.archived = value.archived
        record.credentialNamesData = credentialNamesData
    }

    private func encode<Value: Encodable>(_ value: Value) throws -> Data {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        return try encoder.encode(value)
    }

    private static func senderClientKey(_ sender: String, _ clientKey: String) -> String {
        "\(sender)\u{1f}\(clientKey)"
    }

    private static func isValidAccountID(_ value: String) -> Bool {
        !value.isEmpty
            && value == value.trimmingCharacters(in: .whitespacesAndNewlines)
            && !value.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains)
    }

    private static func isValidFingerprint(_ value: String) -> Bool {
        guard value.count == 71, value.hasPrefix("sha256:") else { return false }
        let hexadecimal = Set("0123456789abcdef")
        return value.dropFirst(7).allSatisfy(hexadecimal.contains)
    }

    private static func prepareStoreDirectory(
        for storeURL: URL,
        fileManager: FileManager
    ) throws {
        let directory = storeURL.deletingLastPathComponent()
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
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try protectedDirectory.setResourceValues(values)
    }
}

enum MessagingClientKey {
    private static let alphabet = Array("0123456789ABCDEFGHJKMNPQRSTVWXYZ")
    private static let allowed = Set(alphabet)

    static func isValid(_ value: String) -> Bool {
        guard value.count == 26,
              let first = value.first,
              ("0" ... "7").contains(first)
        else {
            return false
        }
        return value.allSatisfy { allowed.contains($0) }
    }

    static func generate(date: Date = .now, randomUUID: UUID = UUID()) -> String {
        let milliseconds = UInt64(max(0, date.timeIntervalSince1970 * 1_000))
        var bytes = [UInt8](repeating: 0, count: 16)
        for index in 0 ..< 6 {
            let shift = UInt64((5 - index) * 8)
            bytes[index] = UInt8(truncatingIfNeeded: milliseconds >> shift)
        }
        withUnsafeBytes(of: randomUUID.uuid) { randomBytes in
            for index in 0 ..< 10 {
                bytes[index + 6] = randomBytes[index]
            }
        }

        var result = String()
        result.reserveCapacity(26)
        for group in 0 ..< 26 {
            var value = 0
            for offset in 0 ..< 5 {
                value <<= 1
                let virtualBit = group * 5 + offset
                guard virtualBit >= 2 else { continue }
                let sourceBit = virtualBit - 2
                let byte = bytes[sourceBit / 8]
                value |= Int((byte >> UInt8(7 - (sourceBit % 8))) & 1)
            }
            result.append(alphabet[value])
        }
        return result
    }
}
