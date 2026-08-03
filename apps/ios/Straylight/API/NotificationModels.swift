import Foundation

public enum StraylightNotificationKind: String, Codable, Sendable, Equatable, CaseIterable {
    case briefingReady = "briefing_ready"
    case newsAlert = "news_alert"
    case correction
    case operational

    public var label: String {
        switch self {
        case .briefingReady: "Briefing"
        case .newsAlert: "News"
        case .correction: "Correction"
        case .operational: "Operational"
        }
    }
}

public enum StraylightNotificationImportance: String, Codable, Sendable, Equatable {
    case normal
    case important
}

public struct StraylightNotificationSource: Codable, Sendable, Equatable {
    public let type: String
    public let reference: String
    public let versionRef: String?

    public init(type: String, reference: String, versionRef: String? = nil) {
        self.type = type
        self.reference = reference
        self.versionRef = versionRef
    }

    enum CodingKeys: String, CodingKey {
        case type
        case reference = "ref"
        case versionRef = "version_ref"
    }
}

public enum StraylightNotificationTargetType: String, Codable, Sendable, Equatable {
    case notification
    case today
    case briefing
    case entry
}

public struct StraylightNotificationTarget: Codable, Sendable, Equatable {
    public let type: StraylightNotificationTargetType
    public let date: String?
    public let edition: String?
    public let itemID: String?
    public let entryRef: String?

    public init(
        type: StraylightNotificationTargetType,
        date: String? = nil,
        edition: String? = nil,
        itemID: String? = nil,
        entryRef: String? = nil
    ) {
        self.type = type
        self.date = date
        self.edition = edition
        self.itemID = itemID
        self.entryRef = entryRef
    }

    enum CodingKeys: String, CodingKey {
        case type
        case date
        case edition
        case itemID = "item_id"
        case entryRef = "entry_ref"
    }
}

public enum StraylightNotificationDeliveryState: String, Codable, Sendable, Equatable {
    case suppressed
    case queued
    case running
    case acceptedByAPNs = "accepted_by_apns"
    case failed
    case expired
}

public struct StraylightNotificationDelivery: Codable, Sendable, Equatable, Identifiable {
    public let deliveryRef: String
    public let state: StraylightNotificationDeliveryState
    public let acceptedAt: String?
    public let failedAt: String?
    public let lastErrorCode: String?

    public var id: String { deliveryRef }

    public init(
        deliveryRef: String,
        state: StraylightNotificationDeliveryState,
        acceptedAt: String? = nil,
        failedAt: String? = nil,
        lastErrorCode: String? = nil
    ) {
        self.deliveryRef = deliveryRef
        self.state = state
        self.acceptedAt = acceptedAt
        self.failedAt = failedAt
        self.lastErrorCode = lastErrorCode
    }

    enum CodingKeys: String, CodingKey {
        case deliveryRef = "delivery_ref"
        case state
        case acceptedAt = "accepted_at"
        case failedAt = "failed_at"
        case lastErrorCode = "last_error_code"
    }
}

public struct StraylightNotification: Codable, Sendable, Equatable, Identifiable {
    public let notificationRef: String
    public let kind: StraylightNotificationKind
    public let importance: StraylightNotificationImportance
    public let title: String
    public let body: String
    public let source: StraylightNotificationSource?
    public let target: StraylightNotificationTarget
    public let occurredAt: String
    public let expiresAt: String?
    public let openedAt: String?
    public let acknowledgedAt: String?
    public let deliveries: [StraylightNotificationDelivery]

    public var id: String { notificationRef }
    public var isUnread: Bool { openedAt == nil }

    public init(
        notificationRef: String,
        kind: StraylightNotificationKind,
        importance: StraylightNotificationImportance,
        title: String,
        body: String,
        source: StraylightNotificationSource? = nil,
        target: StraylightNotificationTarget,
        occurredAt: String,
        expiresAt: String? = nil,
        openedAt: String? = nil,
        acknowledgedAt: String? = nil,
        deliveries: [StraylightNotificationDelivery] = []
    ) {
        self.notificationRef = notificationRef
        self.kind = kind
        self.importance = importance
        self.title = title
        self.body = body
        self.source = source
        self.target = target
        self.occurredAt = occurredAt
        self.expiresAt = expiresAt
        self.openedAt = openedAt
        self.acknowledgedAt = acknowledgedAt
        self.deliveries = deliveries
    }

    enum CodingKeys: String, CodingKey {
        case notificationRef = "notification_ref"
        case kind
        case importance
        case title
        case body
        case source
        case target
        case occurredAt = "occurred_at"
        case expiresAt = "expires_at"
        case openedAt = "opened_at"
        case acknowledgedAt = "acknowledged_at"
        case deliveries
    }
}

public struct NotificationListResponse: Codable, Sendable, Equatable {
    public let items: [StraylightNotification]
    public let nextCursor: String?
    public let unreadCount: Int

    public init(items: [StraylightNotification], nextCursor: String? = nil, unreadCount: Int) {
        self.items = items
        self.nextCursor = nextCursor
        self.unreadCount = unreadCount
    }

    enum CodingKeys: String, CodingKey {
        case items
        case nextCursor = "next_cursor"
        case unreadCount = "unread_count"
    }
}

public struct NotificationDetailResponse: Codable, Sendable, Equatable {
    public let notification: StraylightNotification

    public init(notification: StraylightNotification) {
        self.notification = notification
    }
}

public enum NotificationReceiptKind: String, Codable, Sendable, Equatable {
    case opened
    case acknowledged
}

public struct NotificationReceiptRequest: Codable, Sendable, Equatable {
    public let kind: NotificationReceiptKind
    public let deliveryRef: String?

    public init(kind: NotificationReceiptKind, deliveryRef: String? = nil) {
        self.kind = kind
        self.deliveryRef = deliveryRef
    }

    enum CodingKeys: String, CodingKey {
        case kind
        case deliveryRef = "delivery_ref"
    }
}

public struct NotificationReceiptResponse: Codable, Sendable, Equatable {
    public let notificationRef: String
    public let kind: NotificationReceiptKind
    public let deliveryRef: String?
    public let recordedAt: String
    public let replayed: Bool
    public let openedAt: String?
    public let acknowledgedAt: String?

    public init(
        notificationRef: String,
        kind: NotificationReceiptKind,
        deliveryRef: String? = nil,
        recordedAt: String,
        replayed: Bool,
        openedAt: String? = nil,
        acknowledgedAt: String? = nil
    ) {
        self.notificationRef = notificationRef
        self.kind = kind
        self.deliveryRef = deliveryRef
        self.recordedAt = recordedAt
        self.replayed = replayed
        self.openedAt = openedAt
        self.acknowledgedAt = acknowledgedAt
    }

    enum CodingKeys: String, CodingKey {
        case notificationRef = "notification_ref"
        case kind
        case deliveryRef = "delivery_ref"
        case recordedAt = "recorded_at"
        case replayed
        case openedAt = "opened_at"
        case acknowledgedAt = "acknowledged_at"
    }
}

public struct NotificationInstallationRequest: Codable, Sendable, Equatable {
    public let platform: String
    public let environment: String
    public let appID: String
    public let deviceToken: String
    public let preview: String
    public let enabled: Bool

    public init(
        environment: String,
        appID: String,
        deviceToken: String,
        enabled: Bool = true
    ) {
        platform = "ios"
        self.environment = environment
        self.appID = appID
        self.deviceToken = deviceToken
        preview = "generic"
        self.enabled = enabled
    }

    enum CodingKeys: String, CodingKey {
        case platform
        case environment
        case appID = "app_id"
        case deviceToken = "device_token"
        case preview
        case enabled
    }
}

public struct NotificationInstallationResponse: Codable, Sendable, Equatable {
    public let installationRef: String
    public let status: String
    public let updatedAt: String

    enum CodingKeys: String, CodingKey {
        case installationRef = "installation_ref"
        case status
        case updatedAt = "updated_at"
    }
}
