import Foundation

public enum TaskExecutionState: String, Codable, Sendable, CaseIterable {
    case open
    case active
    case waiting
    case completed
}

public struct TaskItem: Identifiable, Codable, Sendable, Equatable {
    public let id: String
    public let title: String
    public let note: String?
    public let state: TaskExecutionState
    public let context: String?
    public let estimatedMinutes: Int?
    public let reason: String?

    public init(
        id: String,
        title: String,
        note: String? = nil,
        state: TaskExecutionState,
        context: String? = nil,
        estimatedMinutes: Int? = nil,
        reason: String? = nil
    ) {
        self.id = id
        self.title = title
        self.note = note
        self.state = state
        self.context = context
        self.estimatedMinutes = estimatedMinutes
        self.reason = reason
    }
}

public enum AlertKind: String, Codable, Sendable {
    case new
    case update
    case correction
    case watching
}

public struct AlertItem: Identifiable, Codable, Sendable, Equatable {
    public let id: String
    public let topic: String
    public let headline: String
    public let detail: String
    public let kind: AlertKind
    public let deliveredAt: Date
    public let whatChanged: String?
    public let acknowledged: Bool

    public init(
        id: String,
        topic: String,
        headline: String,
        detail: String,
        kind: AlertKind,
        deliveredAt: Date,
        whatChanged: String? = nil,
        acknowledged: Bool = false
    ) {
        self.id = id
        self.topic = topic
        self.headline = headline
        self.detail = detail
        self.kind = kind
        self.deliveredAt = deliveredAt
        self.whatChanged = whatChanged
        self.acknowledged = acknowledged
    }
}

public enum NewsDeliveryKind: String, Sendable, Equatable {
    case new
    case update
    case correction
    case context

    public var label: String {
        switch self {
        case .new: "New"
        case .update: "Update"
        case .correction: "Correction"
        case .context: "Context"
        }
    }
}

public struct BriefingNewsItem: Identifiable, Sendable, Equatable {
    public let id: String
    public let editionRef: String
    public let date: String
    public let edition: String
    public let version: Int
    public let sectionTitle: String
    public let topicSlug: String
    public let deliveredAt: String
    public let item: BriefingItem

    public init(
        id: String? = nil,
        editionRef: String,
        date: String,
        edition: String,
        version: Int,
        sectionTitle: String,
        topicSlug: String,
        deliveredAt: String,
        item: BriefingItem
    ) {
        self.id = id ?? item.id
        self.editionRef = editionRef
        self.date = date
        self.edition = edition
        self.version = version
        self.sectionTitle = sectionTitle
        self.topicSlug = topicSlug
        self.deliveredAt = deliveredAt
        self.item = item
    }

    public var kind: NewsDeliveryKind {
        if item.kind.localizedCaseInsensitiveContains("correction") {
            return .correction
        }
        switch item.delta {
        case "new": return .new
        case "update": return .update
        default: return item.whatChanged == nil ? .context : .update
        }
    }

    public var isPriority: Bool {
        kind == .new || kind == .update || kind == .correction
    }
}
