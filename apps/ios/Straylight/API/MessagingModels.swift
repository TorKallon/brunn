import Foundation

struct MessagingSyncRequest: Sendable, Equatable {
    let cursor: Int64
    let waitSeconds: Int
    let conversationID: String?
    let afterSequence: Int64?
    let limit: Int

    init(
        cursor: Int64,
        waitSeconds: Int = 0,
        conversationID: String? = nil,
        afterSequence: Int64? = nil,
        limit: Int = 200
    ) {
        self.cursor = cursor
        self.waitSeconds = min(max(waitSeconds, 0), 25)
        self.conversationID = conversationID
        self.afterSequence = afterSequence
        self.limit = min(max(limit, 1), 200)
    }
}

/// Minimal shape of the authenticated `/status` response used to discover the
/// server-authoritative messaging gate. Unknown status fields decode normally.
public struct MessagingRuntimeStatus: Decodable, Sendable, Equatable {
    public struct FeatureFlags: Decodable, Sendable, Equatable {
        public let messagingEnabled: Bool?

        public init(messagingEnabled: Bool?) {
            self.messagingEnabled = messagingEnabled
        }

        enum CodingKeys: String, CodingKey {
            case messagingEnabled = "messaging_enabled"
        }
    }

    public let status: String
    public let buildRevision: String?
    public let corpusRevision: String?
    public let featureFlags: FeatureFlags?

    public init(
        status: String,
        buildRevision: String? = nil,
        corpusRevision: String? = nil,
        featureFlags: FeatureFlags? = nil
    ) {
        self.status = status
        self.buildRevision = buildRevision
        self.corpusRevision = corpusRevision
        self.featureFlags = featureFlags
    }

    enum CodingKeys: String, CodingKey {
        case status
        case buildRevision = "build_revision"
        case corpusRevision = "corpus_revision"
        case featureFlags = "feature_flags"
    }
}

public enum MessagingWritableMessageKind: String, Codable, Sendable, Equatable {
    case text
    case question
}

public struct MessagingReference: Codable, Sendable, Equatable {
    public let entryReference: String?
    public let url: String?
    public let label: String?

    public init(entryReference: String? = nil, url: String? = nil, label: String? = nil) {
        self.entryReference = entryReference
        self.url = url
        self.label = label
    }

    enum CodingKeys: String, CodingKey {
        case entryReference = "entry_ref"
        case url
        case label
    }
}

public struct MessagingParticipant: Codable, Sendable, Equatable {
    public let agentID: String
    public let role: String

    public init(agentID: String, role: String) {
        self.agentID = agentID
        self.role = role
    }

    enum CodingKeys: String, CodingKey {
        case agentID = "agent_id"
        case role
    }
}

public struct MessagingConversation: Identifiable, Codable, Sendable, Equatable {
    public let conversationID: String
    public let conversationKind: String
    public let subject: String?
    public let status: String
    public let participants: [MessagingParticipant]
    public let lastSeq: Int64
    public let lastMessageAt: String?
    public let lastReadSeq: Int64
    public let unreadCount: Int64
    public let needsHuman: Bool
    public let continuesFrom: String?
    public let continuationID: String?
    public let latestSyncCursor: Int64

    public var id: String { conversationID }

    public init(
        conversationID: String,
        conversationKind: String,
        subject: String?,
        status: String,
        participants: [MessagingParticipant],
        lastSeq: Int64,
        lastMessageAt: String?,
        lastReadSeq: Int64,
        unreadCount: Int64,
        needsHuman: Bool,
        continuesFrom: String? = nil,
        continuationID: String? = nil,
        latestSyncCursor: Int64
    ) {
        self.conversationID = conversationID
        self.conversationKind = conversationKind
        self.subject = subject
        self.status = status
        self.participants = participants
        self.lastSeq = lastSeq
        self.lastMessageAt = lastMessageAt
        self.lastReadSeq = lastReadSeq
        self.unreadCount = unreadCount
        self.needsHuman = needsHuman
        self.continuesFrom = continuesFrom
        self.continuationID = continuationID
        self.latestSyncCursor = latestSyncCursor
    }

    enum CodingKeys: String, CodingKey {
        case conversationID = "conversation_id"
        case conversationKind = "conversation_kind"
        case subject
        case status
        case participants
        case lastSeq = "last_seq"
        case lastMessageAt = "last_message_at"
        case lastReadSeq = "last_read_seq"
        case unreadCount = "unread_count"
        case needsHuman = "needs_human"
        case continuesFrom = "continues_from"
        case continuationID = "continuation_id"
        case latestSyncCursor = "latest_sync_cursor"
    }
}

public struct MessagingMessage: Identifiable, Codable, Sendable, Equatable {
    public let conversationID: String
    public let sequence: Int64
    public let messageID: String
    public let fromAgentID: String?
    public let clientKey: String?
    public let kind: String
    public let bodyMarkdown: String
    public let refs: [MessagingReference]
    public let inReplyToConversationID: String?
    public let inReplyTo: Int64?
    public let correlationID: String?
    public let expectsReply: Bool
    public let replyBy: String?
    public let syncCursor: Int64
    public let createdAt: String

    public var id: String { messageID }

    public init(
        conversationID: String,
        sequence: Int64,
        messageID: String,
        fromAgentID: String?,
        clientKey: String?,
        kind: String,
        bodyMarkdown: String,
        refs: [MessagingReference],
        inReplyToConversationID: String? = nil,
        inReplyTo: Int64? = nil,
        correlationID: String? = nil,
        expectsReply: Bool,
        replyBy: String? = nil,
        syncCursor: Int64,
        createdAt: String
    ) {
        self.conversationID = conversationID
        self.sequence = sequence
        self.messageID = messageID
        self.fromAgentID = fromAgentID
        self.clientKey = clientKey
        self.kind = kind
        self.bodyMarkdown = bodyMarkdown
        self.refs = refs
        self.inReplyToConversationID = inReplyToConversationID
        self.inReplyTo = inReplyTo
        self.correlationID = correlationID
        self.expectsReply = expectsReply
        self.replyBy = replyBy
        self.syncCursor = syncCursor
        self.createdAt = createdAt
    }

    enum CodingKeys: String, CodingKey {
        case conversationID = "conversation_id"
        case sequence = "seq"
        case messageID = "message_id"
        case fromAgentID = "from_agent_id"
        case clientKey = "client_key"
        case kind
        case bodyMarkdown = "body_md"
        case refs
        case inReplyToConversationID = "in_reply_to_conversation_id"
        case inReplyTo = "in_reply_to"
        case correlationID = "correlation_id"
        case expectsReply = "expects_reply"
        case replyBy = "reply_by"
        case syncCursor = "sync_cursor"
        case createdAt = "created_at"
    }
}

public struct MessagingAgent: Identifiable, Codable, Sendable, Equatable {
    public let agentID: String
    public let displayName: String
    public let principalKind: String
    public let deliveryMode: String
    public let online: Bool
    public let lastSeenAt: String?
    public let leaseExpiresAt: String?
    public let archived: Bool
    public let credentialNames: [String]?

    public var id: String { agentID }

    public init(
        agentID: String,
        displayName: String,
        principalKind: String,
        deliveryMode: String,
        online: Bool,
        lastSeenAt: String? = nil,
        leaseExpiresAt: String? = nil,
        archived: Bool,
        credentialNames: [String]? = nil
    ) {
        self.agentID = agentID
        self.displayName = displayName
        self.principalKind = principalKind
        self.deliveryMode = deliveryMode
        self.online = online
        self.lastSeenAt = lastSeenAt
        self.leaseExpiresAt = leaseExpiresAt
        self.archived = archived
        self.credentialNames = credentialNames
    }

    enum CodingKeys: String, CodingKey {
        case agentID = "agent_id"
        case displayName = "display_name"
        case principalKind = "principal_kind"
        case deliveryMode = "delivery_mode"
        case online
        case lastSeenAt = "last_seen_at"
        case leaseExpiresAt = "lease_expires_at"
        case archived
        case credentialNames = "credential_names"
    }
}

public struct MessagingSyncResponse: Codable, Sendable, Equatable {
    public let status: String
    public let cursor: Int64
    public let resumeCursor: Int64?
    public let hasMore: Bool
    public let messages: [MessagingMessage]
    public let conversations: [MessagingConversation]
    public let presence: [MessagingAgent]
    public let unread: [String: Int64]
    public let asOf: String

    public init(
        status: String,
        cursor: Int64,
        resumeCursor: Int64? = nil,
        hasMore: Bool,
        messages: [MessagingMessage],
        conversations: [MessagingConversation],
        presence: [MessagingAgent],
        unread: [String: Int64],
        asOf: String
    ) {
        self.status = status
        self.cursor = cursor
        self.resumeCursor = resumeCursor
        self.hasMore = hasMore
        self.messages = messages
        self.conversations = conversations
        self.presence = presence
        self.unread = unread
        self.asOf = asOf
    }

    enum CodingKeys: String, CodingKey {
        case status
        case cursor
        case resumeCursor = "resume_cursor"
        case hasMore = "has_more"
        case messages
        case conversations
        case presence
        case unread
        case asOf = "as_of"
    }
}

public struct MessagingCreateConversationRequest: Encodable, Sendable, Equatable {
    public let participants: [String]
    public let subject: String?

    public init(participants: [String], subject: String? = nil) {
        self.participants = participants
        self.subject = subject
    }
}

public struct MessagingCreateConversationResponse: Codable, Sendable, Equatable {
    public let conversationID: String
    public let conversation: MessagingConversation
    public let duplicate: Bool

    public init(
        conversationID: String,
        conversation: MessagingConversation,
        duplicate: Bool
    ) {
        self.conversationID = conversationID
        self.conversation = conversation
        self.duplicate = duplicate
    }

    enum CodingKeys: String, CodingKey {
        case conversationID = "conversation_id"
        case conversation
        case duplicate
    }
}

/// The send payload is deliberately Encodable-only. It has no sender field:
/// the server always derives the sender from the authenticated credential.
public struct MessagingSendRequest: Encodable, Sendable, Equatable {
    public let clientKey: String
    public let kind: MessagingWritableMessageKind
    public let bodyMarkdown: String
    public let refs: [MessagingReference]
    public let inReplyTo: Int64?
    public let correlationID: String?
    public let expectsReply: Bool
    public let replyBy: String?

    public init(
        clientKey: String,
        kind: MessagingWritableMessageKind = .text,
        bodyMarkdown: String,
        refs: [MessagingReference] = [],
        inReplyTo: Int64? = nil,
        correlationID: String? = nil,
        expectsReply: Bool = false,
        replyBy: String? = nil
    ) {
        self.clientKey = clientKey
        self.kind = kind
        self.bodyMarkdown = bodyMarkdown
        self.refs = refs
        self.inReplyTo = inReplyTo
        self.correlationID = correlationID
        self.expectsReply = expectsReply
        self.replyBy = replyBy
    }

    public func encodedData() throws -> Data {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        return try encoder.encode(self)
    }

    enum CodingKeys: String, CodingKey {
        case clientKey = "client_key"
        case kind
        case bodyMarkdown = "body_md"
        case refs
        case inReplyTo = "in_reply_to"
        case correlationID = "correlation_id"
        case expectsReply = "expects_reply"
        case replyBy = "reply_by"
    }
}

public struct MessagingSendResponse: Codable, Sendable, Equatable {
    public let conversationID: String
    public let sequence: Int64
    public let message: MessagingMessage
    public let duplicate: Bool
    public let continuationID: String?

    public init(
        conversationID: String,
        sequence: Int64,
        message: MessagingMessage,
        duplicate: Bool,
        continuationID: String? = nil
    ) {
        self.conversationID = conversationID
        self.sequence = sequence
        self.message = message
        self.duplicate = duplicate
        self.continuationID = continuationID
    }

    enum CodingKeys: String, CodingKey {
        case conversationID = "conversation_id"
        case sequence = "seq"
        case message
        case duplicate
        case continuationID = "continuation_id"
    }
}

public struct MessagingReadRequest: Encodable, Sendable, Equatable {
    public let lastReadSeq: Int64

    public init(lastReadSeq: Int64) {
        self.lastReadSeq = lastReadSeq
    }

    enum CodingKeys: String, CodingKey {
        case lastReadSeq = "last_read_seq"
    }
}

public struct MessagingReadResponse: Codable, Sendable, Equatable {
    public let conversationID: String
    public let lastReadSeq: Int64
    public let cursor: Int64
    public let duplicate: Bool

    public init(
        conversationID: String,
        lastReadSeq: Int64,
        cursor: Int64,
        duplicate: Bool
    ) {
        self.conversationID = conversationID
        self.lastReadSeq = lastReadSeq
        self.cursor = cursor
        self.duplicate = duplicate
    }

    enum CodingKeys: String, CodingKey {
        case conversationID = "conversation_id"
        case lastReadSeq = "last_read_seq"
        case cursor
        case duplicate
    }
}

public struct MessagingConversationMutationResponse: Codable, Sendable, Equatable {
    public let conversationID: String
    public let status: String
    public let cursor: Int64
    public let duplicate: Bool

    public init(conversationID: String, status: String, cursor: Int64, duplicate: Bool) {
        self.conversationID = conversationID
        self.status = status
        self.cursor = cursor
        self.duplicate = duplicate
    }

    enum CodingKeys: String, CodingKey {
        case conversationID = "conversation_id"
        case status
        case cursor
        case duplicate
    }
}

public struct MessagingAgentListResponse: Codable, Sendable, Equatable {
    public let agents: [MessagingAgent]
    public let asOf: String

    public init(agents: [MessagingAgent], asOf: String) {
        self.agents = agents
        self.asOf = asOf
    }

    enum CodingKeys: String, CodingKey {
        case agents
        case asOf = "as_of"
    }
}

public struct MessagingCreateAgentRequest: Encodable, Sendable, Equatable {
    public let agentID: String
    public let displayName: String
    public let principalKind: String
    public let deliveryMode: String

    public init(
        agentID: String,
        displayName: String,
        principalKind: String,
        deliveryMode: String = "pull"
    ) {
        self.agentID = agentID
        self.displayName = displayName
        self.principalKind = principalKind
        self.deliveryMode = deliveryMode
    }

    enum CodingKeys: String, CodingKey {
        case agentID = "agent_id"
        case displayName = "display_name"
        case principalKind = "principal_kind"
        case deliveryMode = "delivery_mode"
    }
}

public struct MessagingUpdateAgentRequest: Encodable, Sendable, Equatable {
    public let displayName: String?
    public let deliveryMode: String?
    public let archived: Bool?

    public init(
        displayName: String? = nil,
        deliveryMode: String? = nil,
        archived: Bool? = nil
    ) {
        self.displayName = displayName
        self.deliveryMode = deliveryMode
        self.archived = archived
    }

    enum CodingKeys: String, CodingKey {
        case displayName = "display_name"
        case deliveryMode = "delivery_mode"
        case archived
    }
}

public struct MessagingAgentMutationResponse: Codable, Sendable, Equatable {
    public let agent: MessagingAgent
}

public struct MessagingCredentialBindingRequest: Encodable, Sendable, Equatable {
    public let credentialID: String?

    public init(credentialID: String?) {
        self.credentialID = credentialID
    }

    enum CodingKeys: String, CodingKey {
        case credentialID = "credential_id"
    }
}

public struct MessagingCredentialBindingResponse: Codable, Sendable, Equatable {
    public let agentID: String
    public let credentialID: String?
    public let bound: Bool

    enum CodingKeys: String, CodingKey {
        case agentID = "agent_id"
        case credentialID = "credential_id"
        case bound
    }
}
