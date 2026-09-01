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

public struct AuthSessionData: Codable, Sendable, Equatable {
    public let user: UserSummary
    public let expiresAt: String

    public init(user: UserSummary, expiresAt: String) {
        self.user = user
        self.expiresAt = expiresAt
    }

    enum CodingKeys: String, CodingKey {
        case user
        case expiresAt = "expires_at"
    }
}

public struct AuthCompletionData: Codable, Sendable, Equatable {
    public let message: String?

    public init(message: String? = nil) {
        self.message = message
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

public struct DeviceTaskCredentialBootstrapResponse: Codable, Sendable, Equatable {
    public let id: String
    public let access: String
    public let capabilities: [String]
    public let token: String
}

public struct CredentialRevocationResponse: Codable, Sendable, Equatable {
    public let id: String
    public let status: String
    public let revokedAt: String

    enum CodingKeys: String, CodingKey {
        case id
        case status
        case revokedAt = "revoked_at"
    }
}

// MARK: - Agent-first tasks

public enum AgentTaskStatus: String, Codable, Sendable, Equatable {
    case open
    case waiting
    case done
    case dropped
}

public enum AgentTaskView: String, Codable, Sendable, Equatable {
    case urgent
    case next
    case triage
    case all
}

public struct AgentTaskTodoistStatus: Codable, Sendable, Equatable {
    public let environmentEnabled: Bool
    public let savedMode: String
    public let effectiveMode: String
    public let tokenConfigured: Bool
    public let configurationGeneration: Int
    public let lastRunAt: String?
    public let lastOutcome: String?
    public let lastErrorCode: String?
    public let nextRunAt: String?

    public init(
        environmentEnabled: Bool,
        savedMode: String,
        effectiveMode: String,
        tokenConfigured: Bool,
        configurationGeneration: Int,
        lastRunAt: String? = nil,
        lastOutcome: String? = nil,
        lastErrorCode: String? = nil,
        nextRunAt: String? = nil
    ) {
        self.environmentEnabled = environmentEnabled
        self.savedMode = savedMode
        self.effectiveMode = effectiveMode
        self.tokenConfigured = tokenConfigured
        self.configurationGeneration = configurationGeneration
        self.lastRunAt = lastRunAt
        self.lastOutcome = lastOutcome
        self.lastErrorCode = lastErrorCode
        self.nextRunAt = nextRunAt
    }

    enum CodingKeys: String, CodingKey {
        case environmentEnabled = "environment_enabled"
        case savedMode = "saved_mode"
        case effectiveMode = "effective_mode"
        case tokenConfigured = "token_configured"
        case configurationGeneration = "configuration_generation"
        case lastRunAt = "last_run_at"
        case lastOutcome = "last_outcome"
        case lastErrorCode = "last_error_code"
        case nextRunAt = "next_run_at"
    }
}

public struct AgentTaskCandidate: Identifiable, Codable, Sendable, Equatable {
    public let taskRef: String
    public let entryRef: String
    public let version: Int
    public let title: String
    public let status: AgentTaskStatus
    public let project: String?
    public let requiredContexts: [String]
    public let tier: Int
    public let reason: String
    public let provenanceMarkers: [String]
    public let pinned: Bool

    public var id: String { taskRef }
    public var hasInferredProvenance: Bool { !provenanceMarkers.isEmpty }

    public init(
        taskRef: String,
        entryRef: String,
        version: Int,
        title: String,
        status: AgentTaskStatus = .open,
        project: String? = nil,
        requiredContexts: [String] = [],
        tier: Int,
        reason: String,
        provenanceMarkers: [String] = [],
        pinned: Bool = false
    ) {
        self.taskRef = taskRef
        self.entryRef = entryRef
        self.version = version
        self.title = title
        self.status = status
        self.project = project
        self.requiredContexts = requiredContexts
        self.tier = tier
        self.reason = reason
        self.provenanceMarkers = provenanceMarkers
        self.pinned = pinned
    }

    enum CodingKeys: String, CodingKey {
        case taskRef = "task_ref"
        case entryRef = "entry_ref"
        case version
        case title
        case status
        case project
        case requiredContexts = "required_contexts"
        case tier
        case reason
        case provenanceMarkers = "provenance_markers"
        case pinned
    }
}

public struct AgentTaskCandidatesData: Codable, Sendable, Equatable {
    public let view: AgentTaskView
    public let asOf: String
    public let contextsAvailable: [String]
    public let items: [AgentTaskCandidate]
    public let urgentTotal: Int
    public let nextRemaining: Int
    public let backlogTotal: Int
    public let nextCursor: String?

    public init(
        view: AgentTaskView,
        asOf: String,
        contextsAvailable: [String],
        items: [AgentTaskCandidate],
        urgentTotal: Int,
        nextRemaining: Int,
        backlogTotal: Int,
        nextCursor: String? = nil
    ) {
        self.view = view
        self.asOf = asOf
        self.contextsAvailable = contextsAvailable
        self.items = items
        self.urgentTotal = urgentTotal
        self.nextRemaining = nextRemaining
        self.backlogTotal = backlogTotal
        self.nextCursor = nextCursor
    }

    enum CodingKeys: String, CodingKey {
        case view
        case asOf = "as_of"
        case contextsAvailable = "contexts_available"
        case items
        case urgentTotal = "urgent_total"
        case nextRemaining = "next_remaining"
        case backlogTotal = "backlog_total"
        case nextCursor = "next_cursor"
    }
}

public struct AgentTaskSourcedValue<Value: Codable & Sendable & Equatable>: Codable, Sendable, Equatable {
    public let value: Value
    public let source: String
    public let setAt: String
    public let note: String?

    enum CodingKeys: String, CodingKey {
        case value
        case source
        case setAt = "set_at"
        case note
    }
}

public struct AgentTaskDocument: Codable, Sendable, Equatable {
    public let id: String
    public let title: String
    public let status: AgentTaskSourcedValue<String>?
    public let notes: AgentTaskSourcedValue<String>?
    public let project: AgentTaskSourcedValue<String>?
    public let readyAt: AgentTaskSourcedValue<String>?
    public let softDue: AgentTaskSourcedValue<String>?
    public let hardDue: AgentTaskSourcedValue<String>?
    public let requiredContexts: AgentTaskSourcedValue<[String]>?
    public let estimateMinutes: AgentTaskSourcedValue<Int>?
    public let todayPin: AgentTaskSourcedValue<String>?

    enum CodingKeys: String, CodingKey {
        case id
        case title
        case status
        case notes
        case project
        case readyAt = "ready_at"
        case softDue = "soft_due"
        case hardDue = "hard_due"
        case requiredContexts = "required_contexts"
        case estimateMinutes = "estimate_minutes"
        case todayPin = "today_pin"
    }
}

public struct AgentTaskDetail: Identifiable, Codable, Sendable, Equatable {
    public let taskRef: String
    public let entryRef: String
    public let version: Int
    public let title: String
    public let status: AgentTaskStatus
    public let task: AgentTaskDocument
    public let createdAt: String
    public let updatedAt: String

    public var id: String { taskRef }

    enum CodingKeys: String, CodingKey {
        case taskRef = "task_ref"
        case entryRef = "entry_ref"
        case version
        case title
        case status
        case task
        case createdAt = "created_at"
        case updatedAt = "updated_at"
    }
}

public struct AgentTaskDetailData: Codable, Sendable, Equatable {
    public let task: AgentTaskDetail
}

public struct AgentTaskUpdateData: Codable, Sendable, Equatable {
    public let task: AgentTaskDetail
    public let action: String
    public let correctionRef: String?
    public let doneTodayCount: Int?
    public let nextOccurrenceTaskRef: String?
    public let replayed: Bool

    enum CodingKeys: String, CodingKey {
        case task
        case action
        case correctionRef = "correction_ref"
        case doneTodayCount = "done_today_count"
        case nextOccurrenceTaskRef = "next_occurrence_task_ref"
        case replayed
    }
}

public struct AgentTaskDoneItem: Identifiable, Codable, Sendable, Equatable {
    public let taskRef: String
    public let entryRef: String
    public let version: Int
    public let title: String
    public let doneAt: String
    public let completedVia: String?

    public var id: String { taskRef }

    enum CodingKeys: String, CodingKey {
        case taskRef = "task_ref"
        case entryRef = "entry_ref"
        case version
        case title
        case doneAt = "done_at"
        case completedVia = "completed_via"
    }
}

public struct AgentTaskDoneSummaryData: Codable, Sendable, Equatable {
    public let from: String
    public let through: String
    public let timezone: String
    public let asOf: String
    public let count: Int
    public let doneTodayCount: Int
    public let items: [AgentTaskDoneItem]
    public let nextCursor: String?

    enum CodingKeys: String, CodingKey {
        case from
        case through
        case timezone
        case asOf = "as_of"
        case count
        case doneTodayCount = "done_today_count"
        case items
        case nextCursor = "next_cursor"
    }
}

public struct AgentTaskContext: Identifiable, Codable, Sendable, Equatable {
    public let slug: String
    public let displayName: String
    public let aliases: [String]
    public let description: String?
    public let archived: Bool
    public let createdBy: String
    public let version: Int
    public let activeTaskCount: Int

    public var id: String { slug }

    enum CodingKeys: String, CodingKey {
        case slug
        case displayName = "display_name"
        case aliases
        case description
        case archived
        case createdBy = "created_by"
        case version
        case activeTaskCount = "active_task_count"
    }
}

public struct AgentTaskSurfaceDefault: Codable, Sendable, Equatable {
    public let contextsAvailable: [String]
    public let version: Int

    enum CodingKeys: String, CodingKey {
        case contextsAvailable = "contexts_available"
        case version
    }
}

public struct AgentTaskContextListData: Codable, Sendable, Equatable {
    public let contexts: [AgentTaskContext]
    public let surfaceDefaults: [String: AgentTaskSurfaceDefault]

    enum CodingKeys: String, CodingKey {
        case contexts
        case surfaceDefaults = "surface_defaults"
    }
}

public struct AgentTaskProject: Identifiable, Codable, Sendable, Equatable {
    public let slug: String
    public let title: String
    public let interest: String
    public let lastActivityAt: String?
    public let openTaskCount: Int
    public let lastCheckpointAt: String?
    public let version: Int

    public var id: String { slug }

    enum CodingKeys: String, CodingKey {
        case slug
        case title
        case interest
        case lastActivityAt = "last_activity_at"
        case openTaskCount = "open_task_count"
        case lastCheckpointAt = "last_checkpoint_at"
        case version
    }
}

public struct AgentTaskProjectListData: Codable, Sendable, Equatable {
    public let projects: [AgentTaskProject]
    public let asOf: String

    enum CodingKeys: String, CodingKey {
        case projects
        case asOf = "as_of"
    }
}

public enum AgentTaskTextList: Codable, Sendable, Equatable {
    case text(String)
    case list([String])

    public init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if let text = try? container.decode(String.self) {
            self = .text(text)
        } else {
            self = .list(try container.decode([String].self))
        }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case let .text(text): try container.encode(text)
        case let .list(items): try container.encode(items)
        }
    }

    public var lines: [String] {
        switch self {
        case let .text(text): [text]
        case let .list(items): items
        }
    }
}

public struct AgentTaskCheckpointState: Codable, Sendable, Equatable {
    public let objective: String?
    public let currentState: AgentTaskTextList?
    public let nextActions: AgentTaskTextList?
    public let openQuestions: AgentTaskTextList?

    enum CodingKeys: String, CodingKey {
        case objective
        case currentState = "current_state"
        case nextActions = "next_actions"
        case openQuestions = "open_questions"
    }
}

public struct AgentTaskProjectCheckpoint: Codable, Sendable, Equatable {
    public let checkpointAt: String
    public let state: AgentTaskCheckpointState?

    enum CodingKeys: String, CodingKey {
        case checkpointAt = "checkpoint_at"
        case state
    }
}

public struct AgentTaskWaitingItem: Identifiable, Codable, Sendable, Equatable {
    public let taskRef: String
    public let title: String
    public let since: String
    public let ageDays: Int

    public var id: String { taskRef }

    enum CodingKeys: String, CodingKey {
        case taskRef = "task_ref"
        case title
        case since
        case ageDays = "age_days"
    }
}

public struct AgentTaskProjectStateData: Codable, Sendable, Equatable {
    public struct Project: Codable, Sendable, Equatable {
        public let slug: String
        public let title: String
        public let interest: String
        public let lastActivityAt: String?
        public let version: Int

        enum CodingKeys: String, CodingKey {
            case slug
            case title
            case interest
            case lastActivityAt = "last_activity_at"
            case version
        }
    }

    public let project: Project
    public let checkpoint: AgentTaskProjectCheckpoint?
    public let urgentCount: Int
    public let next: [AgentTaskCandidate]
    public let waiting: [AgentTaskWaitingItem]
    public let waitingTotal: Int
    public let waitingRemaining: Int
    public let parkedCount: Int
    public let asOf: String

    enum CodingKeys: String, CodingKey {
        case project
        case checkpoint
        case urgentCount = "urgent_count"
        case next
        case waiting
        case waitingTotal = "waiting_total"
        case waitingRemaining = "waiting_remaining"
        case parkedCount = "parked_count"
        case asOf = "as_of"
    }
}

public enum AgentTaskCorrectionValue: Sendable, Equatable, Encodable {
    case string(String)
    case strings([String])
    case integer(Int)
    case null

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case let .string(value): try container.encode(value)
        case let .strings(value): try container.encode(value)
        case let .integer(value): try container.encode(value)
        case .null: try container.encodeNil()
        }
    }
}

public enum AgentTaskUpdateOperation: Sendable, Equatable, Encodable {
    case complete
    case snooze(days: Int)
    case snoozeUntil(String)
    case waitOn(String)
    case pinToday
    case unpin
    case confirmHard
    case downgradeToSoft
    case correct(field: String, value: AgentTaskCorrectionValue, note: String?)

    private enum CodingKeys: String, CodingKey {
        case type
        case source
        case completedVia = "completed_via"
        case days
        case until
        case whoOrWhat = "who_or_what"
        case field
        case value
        case note
        case reason
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode("owner", forKey: .source)
        switch self {
        case .complete:
            try container.encode("complete", forKey: .type)
            try container.encode("ios", forKey: .completedVia)
        case let .snooze(days):
            try container.encode("snooze", forKey: .type)
            try container.encode(days, forKey: .days)
        case let .snoozeUntil(until):
            try container.encode("snooze", forKey: .type)
            try container.encode(until, forKey: .until)
        case let .waitOn(value):
            try container.encode("wait_on", forKey: .type)
            try container.encode(value, forKey: .whoOrWhat)
        case .pinToday:
            try container.encode("pin_today", forKey: .type)
        case .unpin:
            try container.encode("unpin", forKey: .type)
        case .confirmHard:
            try container.encode("confirm_hard", forKey: .type)
        case .downgradeToSoft:
            try container.encode("downgrade_to_soft", forKey: .type)
        case let .correct(field, value, note):
            try container.encode("correct", forKey: .type)
            try container.encode(field, forKey: .field)
            try container.encode(value, forKey: .value)
            try container.encodeIfPresent(note, forKey: .note)
            try container.encode("Corrected on iOS", forKey: .reason)
        }
    }
}

public struct AgentTaskUpdateRequest: Encodable, Sendable, Equatable {
    public let expectedVersion: Int
    public let idempotencyKey: String
    public let operation: AgentTaskUpdateOperation

    public init(
        expectedVersion: Int,
        idempotencyKey: String = "ios-task-\(UUID().uuidString.lowercased())",
        operation: AgentTaskUpdateOperation
    ) {
        self.expectedVersion = expectedVersion
        self.idempotencyKey = idempotencyKey
        self.operation = operation
    }

    enum CodingKeys: String, CodingKey {
        case expectedVersion = "expected_version"
        case idempotencyKey = "idempotency_key"
        case operation
    }
}

public struct WorkspaceDashboardData: Codable, Sendable, Equatable {
    public let generatedAt: String
    public let timezone: String
    public let workspaceGeneration: Int
    public let activityTrackingStartedAt: String?
    public let tracking: DashboardTrackingHealth?
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
        tracking: DashboardTrackingHealth? = nil,
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
        self.tracking = tracking
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
        case tracking
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
    public let count: Int?
    public let sizeBytes: Int64?
    public let semantics: String?
    public let status: String?
    public let observedAt: String?

    public init(
        count: Int?,
        sizeBytes: Int64?,
        semantics: String? = nil,
        status: String? = nil,
        observedAt: String? = nil
    ) {
        self.count = count
        self.sizeBytes = sizeBytes
        self.semantics = semantics
        self.status = status
        self.observedAt = observedAt
    }

    enum CodingKeys: String, CodingKey {
        case count
        case sizeBytes = "size_bytes"
        case semantics
        case status
        case observedAt = "observed_at"
    }
}

public struct DashboardTrackingHealth: Codable, Sendable, Equatable {
    public let status: String
    public let trackingStartedAt: String?
    public let dataThrough: String?
    public let lastFlushAt: String?
    public let droppedEvents: UInt64
    public let flushFailures: UInt64

    public init(
        status: String,
        trackingStartedAt: String? = nil,
        dataThrough: String? = nil,
        lastFlushAt: String? = nil,
        droppedEvents: UInt64 = 0,
        flushFailures: UInt64 = 0
    ) {
        self.status = status
        self.trackingStartedAt = trackingStartedAt
        self.dataThrough = dataThrough
        self.lastFlushAt = lastFlushAt
        self.droppedEvents = droppedEvents
        self.flushFailures = flushFailures
    }

    enum CodingKeys: String, CodingKey {
        case status
        case trackingStartedAt = "tracking_started_at"
        case dataThrough = "data_through"
        case lastFlushAt = "last_flush_at"
        case droppedEvents = "dropped_events"
        case flushFailures = "flush_failures"
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
    public let manageable: Bool
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
        manageable: Bool = true,
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
        self.manageable = manageable
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
        case manageable
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

public enum WorkspaceSearchSort: String, Codable, Sendable, CaseIterable, Identifiable {
    case bestMatch = "best_match"
    case lastModified = "last_modified"
    case title

    public var id: String { rawValue }
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
    public let score: Double?
    public let updatedAt: String?

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
        lanes: [String]? = nil,
        score: Double? = nil,
        updatedAt: String? = nil
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
        self.score = score
        self.updatedAt = updatedAt
    }

    enum CodingKeys: String, CodingKey {
        case reference
        case path
        case title
        case version
        case heading
        case excerpt
        case text
        case representation
        case lanes
        case score
        case updatedAt = "updated_at"
    }
}

public enum WorkspaceSearchOrdering {
    public static func sorted(
        _ candidates: [WorkspaceSearchCandidate],
        by sort: WorkspaceSearchSort
    ) -> [WorkspaceSearchCandidate] {
        let positioned = candidates.enumerated().map { index, candidate in
            PositionedCandidate(
                candidate: candidate,
                originalIndex: index,
                updatedAt: parseDate(candidate.updatedAt),
                foldedTitle: candidate.title.folding(
                    options: [.caseInsensitive, .diacriticInsensitive],
                    locale: Locale(identifier: "en_US_POSIX")
                )
            )
        }

        return positioned.sorted { left, right in
            switch sort {
            case .bestMatch:
                if let leftScore = left.candidate.score,
                   let rightScore = right.candidate.score
                {
                    if leftScore != rightScore { return leftScore > rightScore }
                    if let newer = newestFirst(left.updatedAt, right.updatedAt) {
                        return newer
                    }
                }
                // Older servers already return relevance order but do not
                // expose the score. Preserve that order instead of treating
                // every result as a relevance tie.
                return left.originalIndex < right.originalIndex
            case .lastModified:
                if let newer = newestFirst(left.updatedAt, right.updatedAt) {
                    return newer
                }
                return titleThenPath(left, right)
            case .title:
                if left.foldedTitle != right.foldedTitle {
                    return left.foldedTitle < right.foldedTitle
                }
                if let newer = newestFirst(left.updatedAt, right.updatedAt) {
                    return newer
                }
                return left.candidate.path < right.candidate.path
            }
        }.map(\.candidate)
    }

    private struct PositionedCandidate {
        let candidate: WorkspaceSearchCandidate
        let originalIndex: Int
        let updatedAt: Date?
        let foldedTitle: String
    }

    private static func newestFirst(_ left: Date?, _ right: Date?) -> Bool? {
        switch (left, right) {
        case let (left?, right?) where left != right:
            left > right
        case (_?, nil):
            true
        case (nil, _?):
            false
        default:
            nil
        }
    }

    private static func titleThenPath(
        _ left: PositionedCandidate,
        _ right: PositionedCandidate
    ) -> Bool {
        if left.foldedTitle != right.foldedTitle {
            return left.foldedTitle < right.foldedTitle
        }
        if left.candidate.path != right.candidate.path {
            return left.candidate.path < right.candidate.path
        }
        return left.originalIndex < right.originalIndex
    }

    private static func parseDate(_ value: String?) -> Date? {
        guard let value else { return nil }
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = formatter.date(from: value) { return date }
        formatter.formatOptions = [.withInternetDateTime]
        return formatter.date(from: value)
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
    public let sort: WorkspaceSearchSort

    public init(
        id: String = "mobile",
        query: String,
        goal: String = "Find source-backed durable context for the owner",
        limit: Int = 12,
        modes: [String] = ["exact", "lexical"],
        sort: WorkspaceSearchSort = .bestMatch
    ) {
        self.id = id
        self.query = query
        self.goal = goal
        self.limit = limit
        self.modes = modes
        self.sort = sort
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
    public let linkTarget: String?
    public let view: String
    public let maxChars: Int
    public let version: Int?

    enum CodingKeys: String, CodingKey {
        case reference = "ref"
        case path
        case linkTarget = "link_target"
        case view
        case maxChars = "max_chars"
        case version
    }

    public init(
        reference: String? = nil,
        path: String? = nil,
        linkTarget: String? = nil,
        view: String = "full",
        maxChars: Int = 80000,
        version: Int? = nil
    ) {
        self.reference = reference
        self.path = path
        self.linkTarget = linkTarget
        self.view = view
        self.maxChars = maxChars
        self.version = version
    }
}
