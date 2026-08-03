import Foundation

public struct WorkspaceEnvelope<Payload: Decodable & Sendable>: Decodable, Sendable {
    public let sessionID: String?
    public let corpusRevision: String?
    public let status: String
    public let data: Payload

    public init(
        sessionID: String? = nil,
        corpusRevision: String? = nil,
        status: String,
        data: Payload
    ) {
        self.sessionID = sessionID
        self.corpusRevision = corpusRevision
        self.status = status
        self.data = data
    }

    enum CodingKeys: String, CodingKey {
        case sessionID = "session_id"
        case corpusRevision = "corpus_revision"
        case status
        case data
    }
}

public struct UserSummary: Codable, Sendable, Equatable {
    public let id: String
    public let displayName: String
    public let externalRef: String?

    public init(id: String, displayName: String, externalRef: String? = nil) {
        self.id = id
        self.displayName = displayName
        self.externalRef = externalRef
    }

    enum CodingKeys: String, CodingKey {
        case id
        case displayName = "display_name"
        case externalRef = "external_ref"
    }
}

public struct MeData: Codable, Sendable, Equatable {
    public let user: UserSummary
    public let credentialID: String?
    public let corpusRevision: String?
    public let capabilities: [String]
    public let readOnly: Bool

    public init(
        user: UserSummary,
        credentialID: String? = nil,
        corpusRevision: String? = nil,
        capabilities: [String] = [],
        readOnly: Bool = false
    ) {
        self.user = user
        self.credentialID = credentialID
        self.corpusRevision = corpusRevision
        self.capabilities = capabilities
        self.readOnly = readOnly
    }

    enum CodingKeys: String, CodingKey {
        case user
        case credentialID = "credential_id"
        case corpusRevision = "corpus_revision"
        case capabilities
        case readOnly = "read_only"
    }
}

public struct WorkspaceDashboardData: Codable, Sendable, Equatable {
    public let generatedAt: String
    public let timezone: String
    public let workspaceGeneration: Int
    public let activityTrackingStartedAt: String?
    public let storage: DashboardStorage
    public let today: DashboardTodayActivity
    public let activity: [DashboardActivityPoint]
    public let access: [DashboardAccessClient]
    public let coverage: DashboardCoverage?

    public init(
        generatedAt: String,
        timezone: String,
        workspaceGeneration: Int,
        activityTrackingStartedAt: String? = nil,
        storage: DashboardStorage,
        today: DashboardTodayActivity,
        activity: [DashboardActivityPoint],
        access: [DashboardAccessClient],
        coverage: DashboardCoverage? = nil
    ) {
        self.generatedAt = generatedAt
        self.timezone = timezone
        self.workspaceGeneration = workspaceGeneration
        self.activityTrackingStartedAt = activityTrackingStartedAt
        self.storage = storage
        self.today = today
        self.activity = activity
        self.access = access
        self.coverage = coverage
    }

    enum CodingKeys: String, CodingKey {
        case generatedAt = "generated_at"
        case timezone
        case workspaceGeneration = "workspace_generation"
        case activityTrackingStartedAt = "activity_tracking_started_at"
        case storage
        case today
        case activity
        case access
        case coverage
    }
}

public struct DashboardStorage: Codable, Sendable, Equatable {
    public let text: DashboardStorageMetric
    public let binary: DashboardStorageMetric

    public init(text: DashboardStorageMetric, binary: DashboardStorageMetric) {
        self.text = text
        self.binary = binary
    }
}

public struct DashboardStorageMetric: Codable, Sendable, Equatable {
    public let count: Int
    public let sizeBytes: Int64
    public let semantics: String?

    public init(count: Int, sizeBytes: Int64, semantics: String? = nil) {
        self.count = count
        self.sizeBytes = sizeBytes
        self.semantics = semantics
    }

    enum CodingKeys: String, CodingKey {
        case count
        case sizeBytes = "size_bytes"
        case semantics
    }
}

public struct DashboardTodayActivity: Codable, Sendable, Equatable {
    public let readOperations: Int64
    public let readBytes: Int64
    public let writeOperations: Int64
    public let writeBytes: Int64

    public init(
        readOperations: Int64,
        readBytes: Int64,
        writeOperations: Int64,
        writeBytes: Int64
    ) {
        self.readOperations = readOperations
        self.readBytes = readBytes
        self.writeOperations = writeOperations
        self.writeBytes = writeBytes
    }

    enum CodingKeys: String, CodingKey {
        case readOperations = "read_operations"
        case readBytes = "read_bytes"
        case writeOperations = "write_operations"
        case writeBytes = "write_bytes"
    }
}

public struct DashboardActivityPoint: Codable, Sendable, Equatable, Identifiable {
    public let date: String
    public let periodStart: String?
    public let periodEnd: String?
    public let readOperations: Int64
    public let readBytes: Int64
    public let writeOperations: Int64
    public let writeBytes: Int64

    public var id: String { date }

    public init(
        date: String,
        periodStart: String? = nil,
        periodEnd: String? = nil,
        readOperations: Int64,
        readBytes: Int64,
        writeOperations: Int64,
        writeBytes: Int64
    ) {
        self.date = date
        self.periodStart = periodStart
        self.periodEnd = periodEnd
        self.readOperations = readOperations
        self.readBytes = readBytes
        self.writeOperations = writeOperations
        self.writeBytes = writeBytes
    }

    enum CodingKeys: String, CodingKey {
        case date
        case periodStart = "period_start"
        case periodEnd = "period_end"
        case readOperations = "read_operations"
        case readBytes = "read_bytes"
        case writeOperations = "write_operations"
        case writeBytes = "write_bytes"
    }
}

public struct DashboardAccessClient: Codable, Sendable, Equatable, Identifiable {
    public let id: String
    public let name: String
    public let kind: String
    public let access: String
    public let status: String
    public let scopeIDs: [String]
    public let capabilities: [String]
    public let createdAt: String?
    public let revokedAt: String?
    public let lastUsedAt: String?
    public let lastOperation: String?
    public let readOperationsToday: Int64
    public let writeOperationsToday: Int64

    public init(
        id: String,
        name: String,
        kind: String = "api_credential",
        access: String,
        status: String,
        scopeIDs: [String],
        capabilities: [String] = [],
        createdAt: String? = nil,
        revokedAt: String? = nil,
        lastUsedAt: String? = nil,
        lastOperation: String? = nil,
        readOperationsToday: Int64 = 0,
        writeOperationsToday: Int64 = 0
    ) {
        self.id = id
        self.name = name
        self.kind = kind
        self.access = access
        self.status = status
        self.scopeIDs = scopeIDs
        self.capabilities = capabilities
        self.createdAt = createdAt
        self.revokedAt = revokedAt
        self.lastUsedAt = lastUsedAt
        self.lastOperation = lastOperation
        self.readOperationsToday = readOperationsToday
        self.writeOperationsToday = writeOperationsToday
    }

    enum CodingKeys: String, CodingKey {
        case id
        case name
        case kind
        case access
        case status
        case scopeIDs = "scope_ids"
        case capabilities
        case createdAt = "created_at"
        case revokedAt = "revoked_at"
        case lastUsedAt = "last_used_at"
        case lastOperation = "last_operation"
        case readOperationsToday = "read_operations_today"
        case writeOperationsToday = "write_operations_today"
    }
}

public struct DashboardCoverage: Codable, Sendable, Equatable {
    public let days: Int?
    public let activity: String?

    public init(days: Int? = nil, activity: String? = nil) {
        self.days = days
        self.activity = activity
    }
}

public struct BriefingListData: Codable, Sendable, Equatable {
    public let editions: [BriefingListRow]
    public let limit: Int
    public let truncated: Bool
    public let next: BriefingListCursor?
    public let workspaceGeneration: Int

    public init(
        editions: [BriefingListRow],
        limit: Int,
        truncated: Bool,
        next: BriefingListCursor? = nil,
        workspaceGeneration: Int
    ) {
        self.editions = editions
        self.limit = limit
        self.truncated = truncated
        self.next = next
        self.workspaceGeneration = workspaceGeneration
    }

    enum CodingKeys: String, CodingKey {
        case editions
        case limit
        case truncated
        case next
        case workspaceGeneration = "workspace_generation"
    }
}

public struct BriefingListCursor: Codable, Sendable, Equatable {
    public let afterPath: String

    public init(afterPath: String) {
        self.afterPath = afterPath
    }

    enum CodingKeys: String, CodingKey {
        case afterPath = "after_path"
    }
}

public struct BriefingListRow: Codable, Sendable, Equatable, Identifiable {
    public let date: String
    public let edition: String
    public let path: String
    public let entryRef: String
    public let version: Int
    public let generatedAt: String?
    public let summaryMD: [String]
    public let sectionTitles: [String]
    public let itemCount: Int

    public var id: String {
        entryRef
    }

    public init(
        date: String,
        edition: String,
        path: String,
        entryRef: String,
        version: Int,
        generatedAt: String? = nil,
        summaryMD: [String] = [],
        sectionTitles: [String] = [],
        itemCount: Int = 0
    ) {
        self.date = date
        self.edition = edition
        self.path = path
        self.entryRef = entryRef
        self.version = version
        self.generatedAt = generatedAt
        self.summaryMD = summaryMD
        self.sectionTitles = sectionTitles
        self.itemCount = itemCount
    }

    enum CodingKeys: String, CodingKey {
        case date
        case edition
        case path
        case entryRef = "entry_ref"
        case version
        case generatedAt = "generated_at"
        case summaryMD = "summary_md"
        case sectionTitles = "section_titles"
        case itemCount = "item_count"
    }
}

public struct BriefingEditionData: Codable, Sendable, Equatable {
    public let path: String
    public let entryRef: String
    public let version: Int
    public let currentVersion: Int
    public let date: String
    public let edition: String
    public let briefing: BriefingPayload?
    public let markdown: String
    public let createdAt: String
    public let versions: [BriefingEditionVersion]
    public let workspaceGeneration: Int

    public init(
        path: String,
        entryRef: String,
        version: Int,
        currentVersion: Int,
        date: String,
        edition: String,
        briefing: BriefingPayload?,
        markdown: String,
        createdAt: String,
        versions: [BriefingEditionVersion],
        workspaceGeneration: Int
    ) {
        self.path = path
        self.entryRef = entryRef
        self.version = version
        self.currentVersion = currentVersion
        self.date = date
        self.edition = edition
        self.briefing = briefing
        self.markdown = markdown
        self.createdAt = createdAt
        self.versions = versions
        self.workspaceGeneration = workspaceGeneration
    }

    enum CodingKeys: String, CodingKey {
        case path
        case entryRef = "entry_ref"
        case version
        case currentVersion = "current_version"
        case date
        case edition
        case briefing
        case markdown
        case createdAt = "created_at"
        case versions
        case workspaceGeneration = "workspace_generation"
    }
}

public struct BriefingEditionVersion: Codable, Sendable, Equatable, Identifiable {
    public let version: Int
    public let createdAt: String

    public var id: Int {
        version
    }

    public init(version: Int, createdAt: String) {
        self.version = version
        self.createdAt = createdAt
    }

    enum CodingKeys: String, CodingKey {
        case version
        case createdAt = "created_at"
    }
}

public struct BriefingPayload: Codable, Sendable, Equatable {
    public let schema: String?
    public let date: String
    public let edition: String
    public let timezone: String?
    public let generatedAt: String?
    public let summaryMD: [String]?
    public let sections: [BriefingSection]?
    public let delta: BriefingDelta?

    public init(
        schema: String? = "briefing.v1",
        date: String,
        edition: String,
        timezone: String? = nil,
        generatedAt: String? = nil,
        summaryMD: [String]? = [],
        sections: [BriefingSection]? = [],
        delta: BriefingDelta? = nil
    ) {
        self.schema = schema
        self.date = date
        self.edition = edition
        self.timezone = timezone
        self.generatedAt = generatedAt
        self.summaryMD = summaryMD
        self.sections = sections
        self.delta = delta
    }

    enum CodingKeys: String, CodingKey {
        case schema
        case date
        case edition
        case timezone
        case generatedAt = "generated_at"
        case summaryMD = "summary_md"
        case sections
        case delta
    }
}

public struct BriefingDelta: Codable, Sendable, Equatable {
    public let added: [String]
    public let changed: [String]
    public let removed: [String]

    public init(added: [String] = [], changed: [String] = [], removed: [String] = []) {
        self.added = added
        self.changed = changed
        self.removed = removed
    }
}

public struct BriefingSection: Codable, Sendable, Equatable, Identifiable {
    public let topic: String
    public let title: String
    public let items: [BriefingItem]

    public var id: String {
        topic
    }

    public init(topic: String, title: String, items: [BriefingItem]) {
        self.topic = topic
        self.title = title
        self.items = items
    }
}

public struct BriefingItem: Codable, Sendable, Equatable, Identifiable {
    public let id: String
    public let kind: String
    public let headlineMD: String
    public let bodyMD: String?
    public let whyItMatters: String?
    public let detailMD: String?
    public let whatChanged: String?
    public let delta: String?
    public let story: BriefingStory?
    public let times: BriefingTimes?

    public init(
        id: String,
        kind: String,
        headlineMD: String,
        bodyMD: String? = nil,
        whyItMatters: String? = nil,
        detailMD: String? = nil,
        whatChanged: String? = nil,
        delta: String? = nil,
        story: BriefingStory? = nil,
        times: BriefingTimes? = nil
    ) {
        self.id = id
        self.kind = kind
        self.headlineMD = headlineMD
        self.bodyMD = bodyMD
        self.whyItMatters = whyItMatters
        self.detailMD = detailMD
        self.whatChanged = whatChanged
        self.delta = delta
        self.story = story
        self.times = times
    }

    enum CodingKeys: String, CodingKey {
        case id
        case kind
        case headlineMD = "headline_md"
        case bodyMD = "body_md"
        case whyItMatters = "why_it_matters"
        case detailMD = "detail_md"
        case whatChanged = "what_changed"
        case delta
        case story
        case times
    }
}

public struct BriefingStory: Codable, Sendable, Equatable {
    public let key: String
    public let urls: [String]?
    public let title: String?
    public let entities: [String]?
    public let eventAt: String?

    public init(
        key: String,
        urls: [String]? = [],
        title: String? = nil,
        entities: [String]? = [],
        eventAt: String? = nil
    ) {
        self.key = key
        self.urls = urls
        self.title = title
        self.entities = entities
        self.eventAt = eventAt
    }

    enum CodingKeys: String, CodingKey {
        case key
        case urls
        case title
        case entities
        case eventAt = "event_at"
    }
}

public struct BriefingTimes: Codable, Sendable, Equatable {
    public let publishedAt: String?
    public let eventAt: String?
    public let firstSeenAt: String?

    public init(publishedAt: String? = nil, eventAt: String? = nil, firstSeenAt: String? = nil) {
        self.publishedAt = publishedAt
        self.eventAt = eventAt
        self.firstSeenAt = firstSeenAt
    }

    enum CodingKeys: String, CodingKey {
        case publishedAt = "published_at"
        case eventAt = "event_at"
        case firstSeenAt = "first_seen_at"
    }
}

public struct BriefingTopic: Codable, Sendable, Equatable, Identifiable {
    public let slug: String
    public let name: String
    public let sectionOrder: Int
    public let mode: String
    public let editions: [String]
    public let schedule: String?
    public let entities: [String]
    public let symbols: [String]
    public let suppressUnchanged: Bool
    public let freshnessHours: Int
    public let body: String
    public let truncated: Bool?
    public let parseError: String?
    public let path: String
    public let entryRef: String
    public let version: Int

    public var id: String { slug }

    public init(
        slug: String,
        name: String,
        sectionOrder: Int,
        mode: String,
        editions: [String] = [],
        schedule: String? = nil,
        entities: [String] = [],
        symbols: [String] = [],
        suppressUnchanged: Bool = false,
        freshnessHours: Int = 24,
        body: String = "",
        truncated: Bool? = nil,
        parseError: String? = nil,
        path: String,
        entryRef: String,
        version: Int
    ) {
        self.slug = slug
        self.name = name
        self.sectionOrder = sectionOrder
        self.mode = mode
        self.editions = editions
        self.schedule = schedule
        self.entities = entities
        self.symbols = symbols
        self.suppressUnchanged = suppressUnchanged
        self.freshnessHours = freshnessHours
        self.body = body
        self.truncated = truncated
        self.parseError = parseError
        self.path = path
        self.entryRef = entryRef
        self.version = version
    }

    enum CodingKeys: String, CodingKey {
        case slug
        case name
        case sectionOrder = "section_order"
        case mode
        case editions
        case schedule
        case entities
        case symbols
        case suppressUnchanged = "suppress_unchanged"
        case freshnessHours = "freshness_hours"
        case body
        case truncated
        case parseError = "parse_error"
        case path
        case entryRef = "entry_ref"
        case version
    }
}

public struct BriefingPendingRequest: Codable, Sendable, Equatable, Identifiable {
    public let path: String
    public let entryRef: String
    public let date: String?
    public let itemID: String?
    public let editionRef: String?
    public let topic: String?
    public let note: String?

    public var id: String { entryRef }

    public init(
        path: String,
        entryRef: String,
        date: String? = nil,
        itemID: String? = nil,
        editionRef: String? = nil,
        topic: String? = nil,
        note: String? = nil
    ) {
        self.path = path
        self.entryRef = entryRef
        self.date = date
        self.itemID = itemID
        self.editionRef = editionRef
        self.topic = topic
        self.note = note
    }

    enum CodingKeys: String, CodingKey {
        case path
        case entryRef = "entry_ref"
        case date
        case itemID = "item_id"
        case editionRef = "edition_ref"
        case topic
        case note
    }
}

public struct BriefingTopicsSnapshot: Codable, Sendable, Equatable {
    public let topics: [BriefingTopic]
    public let pendingRequests: [BriefingPendingRequest]
    public let pendingRequestsTruncated: Bool?
    public let feedbackPath: String
    public let feedbackTail: [String]
    public let workspaceGeneration: Int

    public init(
        topics: [BriefingTopic],
        pendingRequests: [BriefingPendingRequest] = [],
        pendingRequestsTruncated: Bool? = nil,
        feedbackPath: String = "",
        feedbackTail: [String] = [],
        workspaceGeneration: Int
    ) {
        self.topics = topics
        self.pendingRequests = pendingRequests
        self.pendingRequestsTruncated = pendingRequestsTruncated
        self.feedbackPath = feedbackPath
        self.feedbackTail = feedbackTail
        self.workspaceGeneration = workspaceGeneration
    }

    enum CodingKeys: String, CodingKey {
        case topics
        case pendingRequests = "pending_requests"
        case pendingRequestsTruncated = "pending_requests_truncated"
        case feedbackPath = "feedback_path"
        case feedbackTail = "feedback_tail"
        case workspaceGeneration = "workspace_generation"
    }
}

public struct BriefingItemActionRequest: Codable, Sendable, Equatable {
    public let action: String
    public let editionRef: String?
    public let itemID: String?
    public let topicSlug: String?
    public let verdict: String?
    public let note: String?

    public init(
        action: String,
        editionRef: String? = nil,
        itemID: String? = nil,
        topicSlug: String? = nil,
        verdict: String? = nil,
        note: String? = nil
    ) {
        self.action = action
        self.editionRef = editionRef
        self.itemID = itemID
        self.topicSlug = topicSlug
        self.verdict = verdict
        self.note = note
    }

    enum CodingKeys: String, CodingKey {
        case action
        case editionRef = "edition_ref"
        case itemID = "item_id"
        case topicSlug = "topic_slug"
        case verdict
        case note
    }
}

public struct BriefingItemActionData: Codable, Sendable, Equatable {
    public let action: String?
    public let path: String
    public let entryRef: String
    public let versionRef: String?
    public let version: Int
    public let contentHash: String
    public let line: String?
    public let date: String?
    public let itemID: String?
    public let status: String?
    public let slug: String?
    public let mode: String?

    enum CodingKeys: String, CodingKey {
        case action
        case path
        case entryRef = "entry_ref"
        case versionRef = "version_ref"
        case version
        case contentHash = "content_hash"
        case line
        case date
        case itemID = "item_id"
        case status
        case slug
        case mode
    }
}

public struct WorkspaceSearchData: Codable, Sendable, Equatable {
    public let workspaceGeneration: Int
    public let results: [WorkspaceSearchResultSet]
    public let responseTruncated: Bool?

    public init(
        workspaceGeneration: Int,
        results: [WorkspaceSearchResultSet],
        responseTruncated: Bool? = nil
    ) {
        self.workspaceGeneration = workspaceGeneration
        self.results = results
        self.responseTruncated = responseTruncated
    }

    enum CodingKeys: String, CodingKey {
        case workspaceGeneration = "workspace_generation"
        case results
        case responseTruncated = "response_truncated"
    }
}

public struct WorkspaceSearchResultSet: Codable, Sendable, Equatable, Identifiable {
    public let id: String
    public let goal: String?
    public let candidates: [WorkspaceSearchCandidate]
    public let queryStatus: String?
    public let laneFailures: [String]?

    public init(
        id: String,
        goal: String? = nil,
        candidates: [WorkspaceSearchCandidate],
        queryStatus: String? = nil,
        laneFailures: [String]? = nil
    ) {
        self.id = id
        self.goal = goal
        self.candidates = candidates
        self.queryStatus = queryStatus
        self.laneFailures = laneFailures
    }

    enum CodingKeys: String, CodingKey {
        case id
        case goal
        case candidates
        case queryStatus = "query_status"
        case laneFailures = "lane_failures"
    }
}

public struct WorkspaceSearchCandidate: Codable, Sendable, Equatable, Identifiable {
    public let reference: String?
    public let path: String
    public let title: String
    public let version: Int?
    public let heading: String?
    public let excerpt: String?
    public let text: String?
    public let representation: String?
    public let lanes: [String]?

    public var id: String {
        reference ?? "\(path)#\(heading ?? "")"
    }

    public var previewText: String {
        excerpt ?? text ?? "Open the exact source to read it."
    }

    public init(
        reference: String? = nil,
        path: String,
        title: String,
        version: Int? = nil,
        heading: String? = nil,
        excerpt: String? = nil,
        text: String? = nil,
        representation: String? = nil,
        lanes: [String]? = nil
    ) {
        self.reference = reference
        self.path = path
        self.title = title
        self.version = version
        self.heading = heading
        self.excerpt = excerpt
        self.text = text
        self.representation = representation
        self.lanes = lanes
    }
}

public struct WorkspaceReadData: Codable, Sendable, Equatable {
    public let workspaceGeneration: Int
    public let items: [WorkspaceReadItem]

    public init(workspaceGeneration: Int, items: [WorkspaceReadItem]) {
        self.workspaceGeneration = workspaceGeneration
        self.items = items
    }

    enum CodingKeys: String, CodingKey {
        case workspaceGeneration = "workspace_generation"
        case items
    }
}

public struct WorkspaceReadItem: Codable, Sendable, Equatable, Identifiable {
    public let reference: String?
    public let path: String
    public let title: String?
    public let version: Int?
    public let contentHash: String?
    public let mediaType: String?
    public let view: String?
    public let status: String?
    public let error: WorkspaceReadError?
    public let text: String?
    public let updatedAt: String?
    public let truncated: Bool?

    public var id: String {
        reference ?? path
    }

    public init(
        reference: String? = nil,
        path: String,
        title: String? = nil,
        version: Int? = nil,
        contentHash: String? = nil,
        mediaType: String? = nil,
        view: String? = nil,
        status: String? = nil,
        error: WorkspaceReadError? = nil,
        text: String? = nil,
        updatedAt: String? = nil,
        truncated: Bool? = nil
    ) {
        self.reference = reference
        self.path = path
        self.title = title
        self.version = version
        self.contentHash = contentHash
        self.mediaType = mediaType
        self.view = view
        self.status = status
        self.error = error
        self.text = text
        self.updatedAt = updatedAt
        self.truncated = truncated
    }

    enum CodingKeys: String, CodingKey {
        case reference
        case path
        case title
        case version
        case contentHash = "content_hash"
        case mediaType = "media_type"
        case view
        case status
        case error
        case text
        case updatedAt = "updated_at"
        case truncated
    }
}

public struct WorkspaceReadError: Codable, Sendable, Equatable {
    public let code: String?
    public let message: String?

    public init(code: String? = nil, message: String? = nil) {
        self.code = code
        self.message = message
    }
}

public struct SearchRequest: Encodable, Sendable {
    public let queries: [SearchQuery]
    public let tokenBudget: Int

    public init(queries: [SearchQuery], tokenBudget: Int = 4000) {
        self.queries = queries
        self.tokenBudget = tokenBudget
    }

    enum CodingKeys: String, CodingKey {
        case queries
        case tokenBudget = "token_budget"
    }
}

public struct SearchQuery: Encodable, Sendable {
    public let id: String
    public let query: String
    public let goal: String
    public let limit: Int
    public let modes: [String]

    public init(
        id: String = "mobile",
        query: String,
        goal: String = "Find source-backed durable context for the owner",
        limit: Int = 12,
        modes: [String] = ["exact", "lexical"]
    ) {
        self.id = id
        self.query = query
        self.goal = goal
        self.limit = limit
        self.modes = modes
    }
}

public struct ReadRequest: Encodable, Sendable {
    public let requests: [ReadRequestItem]

    public init(requests: [ReadRequestItem]) {
        self.requests = requests
    }
}

public struct ReadRequestItem: Encodable, Sendable {
    public let reference: String?
    public let path: String?
    public let view: String
    public let maxChars: Int
    public let version: Int?

    enum CodingKeys: String, CodingKey {
        case reference = "ref"
        case path
        case view
        case maxChars = "max_chars"
        case version
    }

    public init(
        reference: String? = nil,
        path: String? = nil,
        view: String = "full",
        maxChars: Int = 80000,
        version: Int? = nil
    ) {
        self.reference = reference
        self.path = path
        self.view = view
        self.maxChars = maxChars
        self.version = version
    }
}
