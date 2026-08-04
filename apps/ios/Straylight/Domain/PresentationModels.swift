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

public struct BriefingDisplaySectionPart: Identifiable, Sendable, Equatable {
    public let section: BriefingSection
    public let itemLabel: String

    public var id: String {
        section.topic
    }

    public init(section: BriefingSection, itemLabel: String) {
        self.section = section
        self.itemLabel = itemLabel
    }
}

public struct BriefingDisplaySection: Identifiable, Sendable, Equatable {
    public let id: String
    public let title: String
    public let parts: [BriefingDisplaySectionPart]

    public var itemCount: Int {
        parts.reduce(0) { $0 + $1.section.items.count }
    }

    public init(id: String, title: String, parts: [BriefingDisplaySectionPart]) {
        self.id = id
        self.title = title
        self.parts = parts
    }

    public static func grouped(_ sections: [BriefingSection]) -> [BriefingDisplaySection] {
        var groups: [BriefingDisplaySection] = []
        var groupIndexes: [String: Int] = [:]

        for section in sections {
            let title = displayTitle(for: section.title)
            let groupID = title.parent == nil ? "topic:\(section.topic)" : "parent:\(title.header)"
            let part = BriefingDisplaySectionPart(
                section: section,
                itemLabel: title.itemLabel
            )

            if let index = groupIndexes[groupID] {
                let group = groups[index]
                groups[index] = BriefingDisplaySection(
                    id: group.id,
                    title: group.title,
                    parts: group.parts + [part]
                )
            } else {
                groupIndexes[groupID] = groups.count
                groups.append(BriefingDisplaySection(
                    id: groupID,
                    title: title.header,
                    parts: [part]
                ))
            }
        }

        return groups
    }

    private static let groupedParentTitles = ["RTS LLC", "Hobby Projects"]

    private static func displayTitle(for title: String) -> (
        header: String,
        itemLabel: String,
        parent: String?
    ) {
        for parent in groupedParentTitles {
            let prefix = "\(parent) — "
            guard title.hasPrefix(prefix) else { continue }
            let child = String(title.dropFirst(prefix.count))
                .trimmingCharacters(in: .whitespacesAndNewlines)
            guard !child.isEmpty else { continue }
            return (parent, child, parent)
        }
        return (title, title, nil)
    }
}

public struct WorkspaceEntryLink: Hashable, Sendable {
    public let target: String
    public let label: String?
    public let isWikiLink: Bool
    public let reference: String?

    public init?(
        target rawTarget: String,
        label: String? = nil,
        isWikiLink: Bool = false
    ) {
        var target = rawTarget.trimmingCharacters(in: .whitespacesAndNewlines)
        if target.hasPrefix("<"), target.hasSuffix(">"), target.count > 2 {
            target.removeFirst()
            target.removeLast()
        }
        target = target.removingPercentEncoding ?? target

        let withoutAlias = target.split(separator: "|", maxSplits: 1).first.map(String.init)
            ?? target
        if let suffix = withoutAlias.firstIndex(where: { "#?^".contains($0) }) {
            target = String(withoutAlias[..<suffix])
        } else {
            target = withoutAlias
        }
        target = target.trimmingCharacters(in: .whitespacesAndNewlines)

        if let reference = Self.entryReference(from: target) {
            self.target = target
            self.label = label
            self.isWikiLink = isWikiLink
            self.reference = reference
            return
        }

        if let scheme = URL(string: target)?.scheme?.lowercased(), !scheme.isEmpty {
            // HTTP(S) is intentionally handled by the system. Every other
            // non-entry scheme fails closed instead of becoming a file or
            // application URL.
            return nil
        }

        guard !target.isEmpty || rawTarget.trimmingCharacters(in: .whitespaces).hasPrefix("#") else {
            return nil
        }

        self.target = target
        self.label = label
        self.isWikiLink = isWikiLink
        reference = nil
    }

    public func pathCandidates(relativeTo sourcePath: String) -> [String] {
        guard reference == nil else { return [] }
        if target.isEmpty { return [sourcePath] }

        let rooted = target.hasPrefix("/")
        let explicitRelative = target.hasPrefix("./") || target.hasPrefix("../")
        let unrooted = String(target.drop(while: { $0 == "/" }))
        let hasMarkdownSuffix = unrooted.lowercased().hasSuffix(".md")
            || unrooted.lowercased().hasSuffix(".markdown")
        let withSuffix = isWikiLink && !hasMarkdownSuffix ? "\(unrooted).md" : unrooted
        let sourceDirectory = sourcePath.split(separator: "/").dropLast().map(String.init)

        var candidates: [String] = []
        func append(_ raw: String) {
            guard let normalized = Self.normalizedPath(raw), !candidates.contains(normalized) else {
                return
            }
            candidates.append(normalized)
        }

        let sourceRelative = !rooted
            && (!isWikiLink || explicitRelative || !unrooted.contains("/"))
        if sourceRelative {
            append((sourceDirectory + [withSuffix]).joined(separator: "/"))
        }
        if isWikiLink, !unrooted.contains("/"), withSuffix != unrooted {
            append(unrooted)
        }
        append(withSuffix)

        // Former-vault paths retain their vault-relative names under
        // `sources/`. Treat that directory as the hosted vault root.
        if sourcePath.hasPrefix("sources/"), !withSuffix.hasPrefix("sources/") {
            if isWikiLink, !unrooted.contains("/"), withSuffix != unrooted {
                append("sources/\(unrooted)")
            }
            append("sources/\(withSuffix)")
        }

        return candidates
    }

    public var lookupTerm: String? {
        guard reference == nil, !target.isEmpty else { return nil }
        let name = target.split(separator: "/").last.map(String.init) ?? target
        if name.lowercased().hasSuffix(".markdown") {
            return String(name.dropLast(9))
        }
        if name.lowercased().hasSuffix(".md") {
            return String(name.dropLast(3))
        }
        return name
    }

    private static func entryReference(from target: String) -> String? {
        if target.hasPrefix("entry:"), target.count > "entry:".count {
            return target
        }
        guard let url = URL(string: target),
              url.scheme?.lowercased() == "straylight",
              url.host?.lowercased() == "entry"
        else { return nil }
        let raw = url.pathComponents.filter { $0 != "/" }.first ?? ""
        return raw.hasPrefix("entry:") ? raw : (raw.isEmpty ? nil : "entry:\(raw)")
    }

    private static func normalizedPath(_ raw: String) -> String? {
        var parts: [String] = []
        for part in raw.split(separator: "/", omittingEmptySubsequences: true) {
            switch part {
            case ".":
                continue
            case "..":
                guard !parts.isEmpty else { return nil }
                parts.removeLast()
            default:
                parts.append(String(part))
            }
        }
        return parts.isEmpty ? nil : parts.joined(separator: "/")
    }
}

public struct WorkspaceEntryRequest: Hashable, Sendable {
    public let reference: String?
    public let pathCandidates: [String]
    public let version: Int?
    public let title: String
    public let lookupTerm: String?

    public init(candidate: WorkspaceSearchCandidate) {
        reference = candidate.reference
        pathCandidates = [candidate.path]
        version = candidate.version
        title = candidate.title
        lookupTerm = nil
    }

    public init(link: WorkspaceEntryLink, sourcePath: String) {
        reference = link.reference
        pathCandidates = link.pathCandidates(relativeTo: sourcePath)
        version = nil
        title = link.label ?? link.lookupTerm ?? "Linked entry"
        lookupTerm = link.isWikiLink && !link.target.contains("/")
            ? link.lookupTerm
            : nil
    }
}
