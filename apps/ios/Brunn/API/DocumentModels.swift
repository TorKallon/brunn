import Foundation

public struct PublishedDocument: Codable, Equatable, Sendable {
    public let slug: String
    public let title: String
    public let summary: String?
    public let bodyMD: String
    public let sources: [PublishedDocumentSource]
    public let version: Int64
    public let currentVersion: Int64
    // Build-served documents (for example `agent-orientation`) publish with
    // no timestamps and no version history.
    public let publishedAt: String?
    public let updatedAt: String?
    public let versions: [PublishedDocumentVersion]
    public let url: String
    public let versionURL: String
    // Additive: older hosted deployments can still supply authenticated document content.
    public let appURL: String?
    public let appVersionURL: String?

    public func matches(_ link: DocumentLink) -> Bool {
        slug == link.slug && version > 0 && currentVersion >= version
            && version == (link.version ?? currentVersion)
            && (versions.contains { $0.version == version }
                || (versions.isEmpty && version == currentVersion))
    }

    enum CodingKeys: String, CodingKey {
        case slug, title, summary, sources, version, versions, url
        case bodyMD = "body_md"
        case currentVersion = "current_version"
        case publishedAt = "published_at"
        case updatedAt = "updated_at"
        case versionURL = "version_url"
        case appURL = "app_url"
        case appVersionURL = "app_version_url"
    }
}

public struct PublishedDocumentSource: Codable, Equatable, Sendable {
    public let label: String
    public let entryRef: String?
    public let url: String?

    enum CodingKeys: String, CodingKey {
        case label, url
        case entryRef = "entry_ref"
    }
}

public struct PublishedDocumentVersion: Codable, Equatable, Sendable {
    public let version: Int64
    public let createdAt: String
    public let versionURL: String
    public let appVersionURL: String?

    enum CodingKeys: String, CodingKey {
        case version
        case createdAt = "created_at"
        case versionURL = "version_url"
        case appVersionURL = "app_version_url"
    }
}
