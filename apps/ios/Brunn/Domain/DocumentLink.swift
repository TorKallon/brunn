import Foundation

/// The one native published-document contract, shared by routing, API requests and sharing.
/// Raw workspace references and titles are deliberately not convertible to document links.
public struct DocumentLink: Hashable, Sendable {
    public let slug: String
    public let version: Int64?

    public init?(slug: String, version: Int64? = nil) {
        let bytes = Array(slug.utf8)
        func alphanumeric(_ byte: UInt8) -> Bool {
            (97 ... 122).contains(byte) || (48 ... 57).contains(byte)
        }
        guard (2 ... 80).contains(bytes.count),
              let first = bytes.first, let last = bytes.last,
              alphanumeric(first), alphanumeric(last),
              bytes.allSatisfy({ alphanumeric($0) || $0 == 45 }),
              version.map({ $0 > 0 }) ?? true
        else { return nil }
        self.slug = slug
        self.version = version
    }

    public init?(url: URL) {
        guard url.absoluteString.lowercased().hasPrefix("brunn://document/"),
              let parts = URLComponents(url: url, resolvingAgainstBaseURL: false),
              parts.scheme?.lowercased() == "brunn",
              parts.percentEncodedHost?.lowercased() == "document",
              parts.user == nil, parts.password == nil, parts.port == nil,
              parts.fragment == nil, parts.percentEncodedPath.hasPrefix("/")
        else { return nil }
        // Validate the wire path, not URL.pathComponents (which decodes/normalizes it).
        // No escaping is needed for a slug, so encoded separators/traversal never qualify.
        let slug = String(parts.percentEncodedPath.dropFirst())
        let version: Int64?
        if let query = parts.percentEncodedQuery {
            guard query.hasPrefix("version="),
                  let parsed = Int64(query.dropFirst("version=".count)), parsed > 0,
                  query == "version=\(parsed)"
            else { return nil }
            version = parsed
        } else {
            version = nil
        }
        self.init(slug: slug, version: version)
    }

    public var url: URL {
        var parts = URLComponents()
        parts.scheme = "brunn"
        parts.host = "document"
        parts.path = "/\(slug)"
        parts.queryItems = queryItems.isEmpty ? nil : queryItems
        // All components were validated at construction.
        return parts.url!
    }

    public var latest: DocumentLink { DocumentLink(slug: slug)! }

    public var queryItems: [URLQueryItem] {
        version.map { [URLQueryItem(name: "version", value: String($0))] } ?? []
    }
}
