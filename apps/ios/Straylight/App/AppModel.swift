import Foundation

enum AppPhase: Equatable {
    case launching
    case connectionRequired
    case ready
    case failed(String)
}

enum AppTab: Hashable {
    case today
    case news
    case archive
    case more
}

@MainActor
final class AppModel: ObservableObject {
    @Published private(set) var phase: AppPhase = .launching
    @Published private(set) var user: UserSummary?
    @Published private(set) var readOnlyCredential = false
    @Published private(set) var isDemo = false
    @Published private(set) var latestBriefing: BriefingEditionData?
    @Published private(set) var cachedAt: Date?
    @Published private(set) var cacheSavedAt: Date?
    @Published private(set) var connectionValidated = false
    @Published private(set) var connectionMessage: String?
    @Published private(set) var privacyMessage: String?
    @Published private(set) var isRefreshingBriefing = false
    @Published private(set) var briefingHistory: [BriefingListRow] = []
    @Published private(set) var canLoadMoreBriefings = false
    @Published private(set) var isLoadingMoreBriefings = false
    @Published private(set) var topicsSnapshot: BriefingTopicsSnapshot?
    @Published private(set) var deliveryMessage: String?
    @Published private(set) var readNewsItemIDs: Set<String> = []
    @Published private(set) var briefingActivity: [BriefingNewsItem] = []
    @Published private(set) var searchResults: [WorkspaceSearchCandidate] = []
    @Published private(set) var searchEnvelopeStatus: String?
    @Published private(set) var searchMessage: String?
    @Published private(set) var isSearching = false
    @Published private(set) var tasks: [TaskItem] = []
    @Published private(set) var alerts: [AlertItem] = []
    @Published var selectedTab: AppTab = .today
    @Published var focusedBriefingItemID: String?

    var newsItems: [BriefingNewsItem] {
        if !briefingActivity.isEmpty { return briefingActivity }
        guard let edition = latestBriefing else { return [] }
        return Self.projectNews(from: edition, uniqueIDs: false)
    }

    let api: StraylightAPI
    private let credentialStore: KeychainCredentialStore
    private let briefingCache: BriefingCache
    private var pendingRoute: AppRoute?
    private var nextBriefingHistoryPath: String?

    init(
        api: StraylightAPI = StraylightAPI(),
        credentialStore: KeychainCredentialStore = KeychainCredentialStore(),
        briefingCache: BriefingCache = BriefingCache()
    ) {
        self.api = api
        self.credentialStore = credentialStore
        self.briefingCache = briefingCache
    }

    func bootstrap() async {
        if ProcessInfo.processInfo.arguments.contains("--demo") {
            enterDemo()
            return
        }

        do {
            guard let token = try credentialStore.load() else {
                phase = .connectionRequired
                return
            }
            await api.setBearerToken(token)
            await loadCachedBriefing()
            if latestBriefing != nil {
                user = UserSummary(id: "cached", displayName: "Owner")
                readOnlyCredential = true
                connectionValidated = false
                phase = .ready
                connectionMessage = "Checking Straylight while the last protected briefing remains available."
                applyPendingRouteLocally()
            }
            do {
                let identity = try await api.me()
                guard Self.isAllowedDeviceCredential(identity) else {
                    do {
                        try credentialStore.delete()
                        try await briefingCache.clear()
                        cacheSavedAt = nil
                        cachedAt = nil
                        latestBriefing = nil
                    } catch {
                        await api.setBearerToken(nil)
                        phase = .failed(Self.credentialRemovalFailure(error))
                        return
                    }
                    await api.setBearerToken(nil)
                    phase = .connectionRequired
                    connectionMessage = Self.credentialScopeMessage
                    return
                }
                accept(identity)
                await refreshBriefing()
                await resumePendingRoute()
            } catch let error as StraylightAPIError where error.isUnauthorized {
                do {
                    try credentialStore.delete()
                    try await briefingCache.clear()
                    cacheSavedAt = nil
                    cachedAt = nil
                    latestBriefing = nil
                } catch {
                    await api.setBearerToken(nil)
                    phase = .failed(Self.credentialRemovalFailure(error))
                    return
                }
                await api.setBearerToken(nil)
                connectionValidated = false
                phase = .connectionRequired
                connectionMessage = "This device credential is no longer accepted."
            } catch {
                if latestBriefing != nil {
                    user = UserSummary(id: "cached", displayName: "Owner")
                    readOnlyCredential = true
                    connectionValidated = false
                    phase = .ready
                    connectionMessage = "Showing the last protected briefing because Straylight could not be reached."
                    applyPendingRouteLocally()
                } else {
                    phase = .failed(error.localizedDescription)
                }
            }
        } catch {
            phase = .failed(error.localizedDescription)
        }
    }

    func connect(with token: String) async {
        let token = token.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !token.isEmpty else {
            connectionMessage = "Paste the dedicated device credential."
            return
        }
        phase = .launching
        connectionMessage = nil
        await api.setBearerToken(token)
        do {
            let identity = try await api.me()
            guard Self.isAllowedDeviceCredential(identity) else {
                await api.setBearerToken(nil)
                phase = .connectionRequired
                connectionMessage = Self.credentialScopeMessage
                return
            }
            try credentialStore.save(token)
            accept(identity)
            await loadCachedBriefing()
            await refreshBriefing()
            await resumePendingRoute()
        } catch {
            await api.setBearerToken(nil)
            phase = .connectionRequired
            connectionMessage = error.localizedDescription
        }
    }

    func enterDemo() {
        isDemo = true
        user = UserSummary(id: "user:demo", displayName: "Rourke")
        latestBriefing = SampleData.briefing
        tasks = SampleData.tasks
        alerts = SampleData.alerts
        briefingHistory = SampleData.briefingHistory
        canLoadMoreBriefings = false
        nextBriefingHistoryPath = nil
        topicsSnapshot = SampleData.topicsSnapshot
        readNewsItemIDs = SampleData.initiallyReadNewsItemIDs
        briefingActivity = Self.projectNews(from: SampleData.briefing, uniqueIDs: false)
        cachedAt = nil
        cacheSavedAt = nil
        connectionValidated = false
        connectionMessage = "Demo content · nothing here is written to Straylight"
        privacyMessage = nil
        phase = .ready
        if let pendingRoute {
            self.pendingRoute = nil
            applyLocalRoute(pendingRoute)
        }
    }

    func disconnect() async {
        do {
            try credentialStore.delete()
            try await briefingCache.clear()
        } catch {
            privacyMessage = "Disconnect is incomplete: private local data could not be removed. \(error.localizedDescription) Retry before considering this iPhone disconnected."
            return
        }
        await api.setBearerToken(nil)
        user = nil
        latestBriefing = nil
        briefingHistory = []
        canLoadMoreBriefings = false
        nextBriefingHistoryPath = nil
        topicsSnapshot = nil
        readNewsItemIDs = []
        briefingActivity = []
        tasks = []
        alerts = []
        searchResults = []
        cachedAt = nil
        cacheSavedAt = nil
        connectionValidated = false
        connectionMessage = nil
        privacyMessage = nil
        isDemo = false
        phase = .connectionRequired
    }

    func retryBootstrap() async {
        phase = .launching
        await bootstrap()
    }

    func clearBriefingCache() async {
        do {
            try await briefingCache.clear()
            cachedAt = nil
            cacheSavedAt = nil
            privacyMessage = nil
        } catch {
            privacyMessage = "The protected briefing cache could not be removed. \(error.localizedDescription)"
        }
    }

    func refreshBriefing() async {
        guard !isDemo, phase == .ready, !isRefreshingBriefing else { return }
        isRefreshingBriefing = true
        defer { isRefreshingBriefing = false }
        do {
            let fetchedEdition = try await api.latestBriefing()
            connectionValidated = true
            guard let edition = fetchedEdition else {
                latestBriefing = nil
                cachedAt = nil
                connectionMessage = "No published briefing is available yet."
                await resumePendingRoute()
                return
            }
            latestBriefing = edition
            cachedAt = nil
            connectionMessage = nil
            let savedAt = Date()
            do {
                try await briefingCache.save(edition, at: savedAt)
                cacheSavedAt = savedAt
                privacyMessage = nil
            } catch {
                cacheSavedAt = nil
                privacyMessage = "The latest briefing is visible, but its protected offline copy could not be saved."
            }
            alerts = projectedAlerts(from: edition)
            briefingActivity = Self.projectNews(from: edition, uniqueIDs: false)
            await refreshBriefingIndexAndTopics()
            await resumePendingRoute()
        } catch {
            connectionMessage = "Refresh failed. The last available briefing remains visible."
        }
    }

    func loadBriefing(
        date: String,
        edition: String,
        version: Int? = nil
    ) async throws -> BriefingEditionData {
        if isDemo {
            return SampleData.briefing(date: date, edition: edition, version: version)
        }
        return try await api.briefing(date: date, edition: edition, version: version)
    }

    func markNewsItemRead(_ id: String) {
        readNewsItemIDs.insert(id)
    }

    func isNewsItemRead(_ id: String) -> Bool {
        readNewsItemIDs.contains(id)
    }

    func refreshBriefingIndexAndTopics() async {
        guard !isDemo, phase == .ready else { return }
        deliveryMessage = nil

        do {
            let response = try await api.briefings(limit: 30)
            briefingHistory = response.data.editions
            nextBriefingHistoryPath = response.data.next?.afterPath
            canLoadMoreBriefings = response.data.truncated && nextBriefingHistoryPath != nil
        } catch {
            deliveryMessage = "Briefing history could not be refreshed."
        }

        do {
            topicsSnapshot = try await api.briefingTopics().data
        } catch {
            if deliveryMessage == nil {
                deliveryMessage = "Tracked-topic details could not be refreshed."
            }
        }

        await refreshNewsActivity()
    }

    func loadMoreBriefings() async {
        guard !isDemo,
              phase == .ready,
              canLoadMoreBriefings,
              !isLoadingMoreBriefings,
              let nextBriefingHistoryPath
        else { return }

        isLoadingMoreBriefings = true
        defer { isLoadingMoreBriefings = false }
        do {
            let response = try await api.briefings(
                limit: 30,
                afterPath: nextBriefingHistoryPath
            )
            let existing = Set(briefingHistory.map(\.entryRef))
            briefingHistory.append(contentsOf: response.data.editions.filter {
                !existing.contains($0.entryRef)
            })
            self.nextBriefingHistoryPath = response.data.next?.afterPath
            canLoadMoreBriefings = response.data.truncated && self.nextBriefingHistoryPath != nil
            deliveryMessage = nil
        } catch {
            deliveryMessage = "Older briefings could not be loaded."
        }
    }

    func performSearch(_ query: String) async {
        let query = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard query.count >= 2 else {
            searchMessage = "Enter at least two characters."
            return
        }
        isSearching = true
        searchMessage = nil
        defer { isSearching = false }

        if isDemo {
            searchResults = SampleData.searchResults.filter {
                $0.title.localizedCaseInsensitiveContains(query)
                    || $0.previewText.localizedCaseInsensitiveContains(query)
                    || query.localizedCaseInsensitiveContains("straylight")
            }
            if searchResults.isEmpty {
                searchResults = SampleData.searchResults
            }
            searchEnvelopeStatus = "complete"
            return
        }

        do {
            let response = try await api.search(query)
            searchEnvelopeStatus = response.status
            searchResults = response.data.results.flatMap(\.candidates)
            let incomplete = response.status != "complete"
                || response.data.responseTruncated == true
                || response.data.results.contains(where: { $0.queryStatus == "partial" })
            if searchResults.isEmpty {
                searchMessage = incomplete
                    ? "No result was returned, but retrieval was not complete."
                    : "No matching sources were returned."
            } else if incomplete {
                searchMessage = "Retrieval was partial or budget-truncated; these source matches are useful but incomplete."
            }
        } catch {
            searchResults = []
            searchEnvelopeStatus = nil
            searchMessage = error.localizedDescription
        }
    }

    func read(_ candidate: WorkspaceSearchCandidate) async throws -> WorkspaceReadItem {
        if isDemo {
            return WorkspaceReadItem(
                reference: candidate.reference,
                path: candidate.path,
                title: candidate.title,
                version: candidate.version,
                text: "# \(candidate.title)\n\n\(candidate.previewText)\n\nThis is deterministic demo content. A connected app requests the exact current source from hosted Straylight."
            )
        }
        return try await api.read(
            reference: candidate.reference,
            path: candidate.path,
            version: candidate.version
        )
    }

    func handle(_ route: AppRoute) async {
        guard phase == .ready else {
            pendingRoute = route
            return
        }

        applyLocalRoute(route)
        switch route {
        case .today, .alert, .task:
            return
        case let .briefing(date, edition, _):
            guard !isDemo else { return }
            guard connectionValidated else {
                pendingRoute = route
                return
            }
            do {
                let briefing = try await api.briefing(date: date, edition: edition)
                latestBriefing = briefing
            } catch {
                connectionMessage = "The linked briefing could not be loaded."
            }
        }
    }

    nonisolated static func isAllowedDeviceCredential(_ identity: MeData) -> Bool {
        let readOnlyCapabilities: Set = [
            "open", "query", "read", "compute", "verify", "status",
        ]
        let capabilities = Set(identity.capabilities)
        let requiredCapabilities: Set = ["query", "read", "status"]
        return identity.readOnly
            && capabilities.isSubset(of: readOnlyCapabilities)
            && requiredCapabilities.isSubset(of: capabilities)
    }

    private static let credentialScopeMessage =
        "This alpha accepts only a dedicated read_only credential. Broader read_write and owner credentials are intentionally rejected."

    private static func credentialRemovalFailure(_ error: Error) -> String {
        "The rejected credential could not be removed from Keychain. \(error.localizedDescription)"
    }

    private func accept(_ identity: MeData) {
        isDemo = false
        user = identity.user
        readOnlyCredential = identity.readOnly
        connectionValidated = true
        phase = .ready
        tasks = []
        alerts = []
        connectionMessage = nil
    }

    private func loadCachedBriefing() async {
        do {
            guard let cached = try await briefingCache.load() else { return }
        latestBriefing = cached.edition
            cachedAt = cached.savedAt
            cacheSavedAt = cached.savedAt
            alerts = projectedAlerts(from: cached.edition)
            briefingHistory = [Self.listRow(from: cached.edition)]
            briefingActivity = Self.projectNews(from: cached.edition, uniqueIDs: false)
            canLoadMoreBriefings = false
            nextBriefingHistoryPath = nil
        } catch {
            privacyMessage = "The protected briefing cache could not be read. Clear it from More before relying on offline access."
        }
    }

    private func resumePendingRoute() async {
        guard let pendingRoute else { return }
        self.pendingRoute = nil
        await handle(pendingRoute)
    }

    private func applyPendingRouteLocally() {
        guard let pendingRoute else { return }
        applyLocalRoute(pendingRoute)
    }

    private func applyLocalRoute(_ route: AppRoute) {
        switch route {
        case .today:
            selectedTab = .today
        case let .briefing(_, _, itemID):
            selectedTab = .today
            focusedBriefingItemID = itemID
        case .alert:
            selectedTab = .news
        case .task:
            selectedTab = .today
        }
    }

    private static func listRow(from edition: BriefingEditionData) -> BriefingListRow {
        BriefingListRow(
            date: edition.date,
            edition: edition.edition,
            path: edition.path,
            entryRef: edition.entryRef,
            version: edition.currentVersion,
            generatedAt: edition.briefing?.generatedAt,
            summaryMD: edition.briefing?.summaryMD ?? [],
            sectionTitles: edition.briefing?.sections?.map(\.title) ?? [],
            itemCount: edition.briefing?.sections?.reduce(0) { $0 + $1.items.count } ?? 0
        )
    }

    private func refreshNewsActivity() async {
        guard !isDemo, !briefingHistory.isEmpty else { return }
        var activity: [BriefingNewsItem] = []

        for row in briefingHistory.prefix(7) {
            let current: BriefingEditionData
            do {
                if let latestBriefing,
                   latestBriefing.entryRef == row.entryRef,
                   latestBriefing.version == row.version
                {
                    current = latestBriefing
                } else {
                    current = try await api.briefing(date: row.date, edition: row.edition)
                }
            } catch {
                continue
            }

            let versions = current.versions.suffix(5)
            for descriptor in versions {
                let edition: BriefingEditionData
                do {
                    edition = descriptor.version == current.version
                        ? current
                        : try await api.briefing(
                            date: row.date,
                            edition: row.edition,
                            version: descriptor.version
                        )
                } catch {
                    continue
                }

                let delta = edition.briefing?.delta
                let relevantIDs: Set<String>? = descriptor.version == 1
                    ? nil
                    : Set((delta?.added ?? []) + (delta?.changed ?? []))
                activity.append(contentsOf: Self.projectNews(
                    from: edition,
                    uniqueIDs: true,
                    deliveredAt: descriptor.createdAt,
                    relevantIDs: relevantIDs
                ))

                for removedID in delta?.removed ?? [] {
                    let removed = BriefingItem(
                        id: removedID,
                        kind: "correction",
                        headlineMD: "**A previously published item was removed from this briefing.**",
                        bodyMD: "The item identifier was `\(removedID)`.",
                        whyItMatters: "Removal is preserved as a visible revision event instead of silently disappearing.",
                        whatChanged: "Removed in version \(descriptor.version).",
                        delta: "correction"
                    )
                    activity.append(BriefingNewsItem(
                        id: "\(edition.entryRef):v\(descriptor.version):removed:\(removedID)",
                        editionRef: edition.entryRef,
                        date: edition.date,
                        edition: edition.edition,
                        version: descriptor.version,
                        sectionTitle: "Briefing corrections",
                        topicSlug: "corrections",
                        deliveredAt: descriptor.createdAt,
                        item: removed
                    ))
                }
            }
        }

        if !activity.isEmpty {
            briefingActivity = activity.sorted {
                Self.activityDate($0.deliveredAt) > Self.activityDate($1.deliveredAt)
            }
        }
    }

    private static func projectNews(
        from edition: BriefingEditionData,
        uniqueIDs: Bool,
        deliveredAt: String? = nil,
        relevantIDs: Set<String>? = nil
    ) -> [BriefingNewsItem] {
        (edition.briefing?.sections ?? []).flatMap { section in
            section.items.compactMap { item in
                if let relevantIDs, !relevantIDs.contains(item.id) { return nil }
                return BriefingNewsItem(
                    id: uniqueIDs ? "\(edition.entryRef):v\(edition.version):\(item.id)" : item.id,
                    editionRef: edition.entryRef,
                    date: edition.date,
                    edition: edition.edition,
                    version: edition.version,
                    sectionTitle: section.title,
                    topicSlug: section.topic,
                    deliveredAt: deliveredAt
                        ?? item.times?.publishedAt
                        ?? item.times?.firstSeenAt
                        ?? edition.createdAt,
                    item: item
                )
            }
        }
    }

    private static func activityDate(_ raw: String) -> Date {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = formatter.date(from: raw) { return date }
        formatter.formatOptions = [.withInternetDateTime]
        return formatter.date(from: raw) ?? .distantPast
    }

    private func projectedAlerts(from edition: BriefingEditionData) -> [AlertItem] {
        let deliveredAt = ISO8601DateFormatter().date(from: edition.createdAt) ?? .now
        return (edition.briefing?.sections ?? [])
            .flatMap(\.items)
            .filter { $0.delta == "update" || $0.whatChanged != nil }
            .map { item in
                AlertItem(
                    id: "\(edition.entryRef):\(item.id)",
                    topic: "\(item.kind.uppercased()) · BRIEFING UPDATE",
                    headline: item.headlineMD,
                    detail: item.bodyMD ?? "Open the briefing for the source-backed update.",
                    kind: .update,
                    deliveredAt: deliveredAt,
                    whatChanged: item.whatChanged
                )
            }
    }
}
