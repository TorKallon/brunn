import Foundation

private enum BootstrapValidationError: LocalizedError {
    case timedOut

    var errorDescription: String? {
        "The saved connection could not be verified quickly enough."
    }
}

typealias BootstrapIdentityLoader = @Sendable (StraylightAPI) async throws -> MeData
typealias StoredSessionChecker = @Sendable (StraylightAPI) async -> Bool
typealias LoginLoader = @Sendable (StraylightAPI, String, String) async throws -> MeData
typealias DashboardLoader = @Sendable (StraylightAPI, String) async throws -> WorkspaceDashboardData

enum AppPhase: Equatable {
    case launching
    case connectionRequired
    case ready
    case failed(String)
}

enum AppTab: Hashable {
    case dashboard
    case today
    case news
    case archive
    case more
}

@MainActor
final class AppModel: ObservableObject {
    @Published private(set) var phase: AppPhase = .launching
    @Published private(set) var user: UserSummary?
    @Published private(set) var currentCredentialID: String?
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
    @Published private(set) var dashboard: WorkspaceDashboardData?
    @Published private(set) var dashboardMessage: String?
    @Published private(set) var isRefreshingDashboard = false
    @Published private(set) var searchResults: [WorkspaceSearchCandidate] = []
    @Published private(set) var searchEnvelopeStatus: String?
    @Published private(set) var searchMessage: String?
    @Published private(set) var isSearching = false
    @Published private(set) var tasks: [TaskItem] = []
    @Published private(set) var alerts: [AlertItem] = []
    @Published var selectedTab: AppTab = .dashboard
    @Published var focusedBriefingItemID: String?

    var newsItems: [BriefingNewsItem] {
        if !briefingActivity.isEmpty { return briefingActivity }
        guard let edition = latestBriefing else { return [] }
        return Self.projectNews(from: edition, uniqueIDs: false)
    }

    let api: StraylightAPI
    private let credentialStore: any CredentialStoring
    private let briefingCache: BriefingCache
    private let bootstrapValidationTimeout: Duration
    private let storedSessionChecker: StoredSessionChecker
    private let bootstrapIdentityLoader: BootstrapIdentityLoader
    private let loginLoader: LoginLoader
    private let dashboardLoader: DashboardLoader
    private var pendingRoute: AppRoute?
    private var nextBriefingHistoryPath: String?
    private var dashboardContextGeneration: UInt64 = 0

    init(
        api: StraylightAPI = StraylightAPI(),
        credentialStore: any CredentialStoring = KeychainCredentialStore(),
        briefingCache: BriefingCache = BriefingCache(),
        bootstrapValidationTimeout: Duration = .seconds(6),
        storedSessionChecker: @escaping StoredSessionChecker = { api in
            api.hasAuthenticatedSession()
        },
        bootstrapIdentityLoader: @escaping BootstrapIdentityLoader = { api in
            _ = try await api.authSession()
            return try await api.me()
        },
        loginLoader: @escaping LoginLoader = { api, email, password in
            _ = try await api.login(email: email, password: password)
            return try await api.me()
        },
        dashboardLoader: @escaping DashboardLoader = { api, timezone in
            try await api.dashboard(timezone: timezone).data
        }
    ) {
        self.api = api
        self.credentialStore = credentialStore
        self.briefingCache = briefingCache
        self.bootstrapValidationTimeout = bootstrapValidationTimeout
        self.storedSessionChecker = storedSessionChecker
        self.bootstrapIdentityLoader = bootstrapIdentityLoader
        self.loginLoader = loginLoader
        self.dashboardLoader = dashboardLoader
    }

    func bootstrap() async {
        if ProcessInfo.processInfo.arguments.contains("--demo") {
            enterDemo()
            return
        }

        if ProcessInfo.processInfo.arguments.contains("--ui-test-connection-required") {
            phase = .connectionRequired
            return
        }

        invalidateDashboardContext()
        try? credentialStore.delete()
        guard await storedSessionChecker(api) else {
            phase = .connectionRequired
            return
        }

        await loadCachedBriefing()
        if latestBriefing != nil {
            user = UserSummary(id: "cached", displayName: "Owner")
            connectionValidated = false
            phase = .ready
            connectionMessage = "Checking Straylight while the last protected briefing remains available."
            applyPendingRouteLocally()
        }

        do {
            let identity = try await loadBootstrapIdentity()
            accept(identity)
            Task { await refreshDashboard() }
            await refreshBriefing()
            await resumePendingRoute()
        } catch is BootstrapValidationError {
            connectionValidated = false
            phase = .connectionRequired
            connectionMessage = "The saved sign-in is taking too long to verify. Sign in again, or retry when connectivity returns."
        } catch let error as StraylightAPIError where error.isUnauthorized {
            await api.clearAuthenticatedSession()
            do {
                try await briefingCache.clear()
                cacheSavedAt = nil
                cachedAt = nil
                latestBriefing = nil
            } catch {
                phase = .failed("The expired sign-in was removed, but the protected cache could not be cleared. \(error.localizedDescription)")
                return
            }
            connectionValidated = false
            phase = .connectionRequired
            connectionMessage = "Your session expired. Sign in again."
        } catch {
            if latestBriefing != nil {
                user = UserSummary(id: "cached", displayName: "Owner")
                connectionValidated = false
                phase = .ready
                connectionMessage = "Showing the last protected briefing because Straylight could not be reached."
                applyPendingRouteLocally()
            } else {
                phase = .failed(error.localizedDescription)
            }
        }
    }

    func connect(email: String, password: String) async {
        let email = email.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !email.isEmpty, !password.isEmpty else {
            connectionMessage = "Enter your email and password."
            return
        }
        invalidateDashboardContext()
        phase = .launching
        connectionMessage = nil
        await api.clearAuthenticatedSession()
        do {
            let identity = try await loginLoader(api, email, password)
            accept(identity)
            Task { await refreshDashboard() }
            await loadCachedBriefing()
            await refreshBriefing()
            await resumePendingRoute()
        } catch {
            await api.clearAuthenticatedSession()
            phase = .connectionRequired
            if let error = error as? StraylightAPIError, error.isUnauthorized {
                connectionMessage = "The email or password is incorrect."
            } else {
                connectionMessage = error.localizedDescription
            }
        }
    }

    func enterDemo() {
        invalidateDashboardContext()
        isDemo = true
        user = UserSummary(id: "user:demo", displayName: "Rourke")
        currentCredentialID = "credential:demo-iphone"
        dashboard = SampleData.dashboard
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
        try? await api.logout()
        await api.clearAuthenticatedSession()
        do {
            try credentialStore.delete()
            try await briefingCache.clear()
        } catch {
            privacyMessage = "Disconnect is incomplete: private local data could not be removed. \(error.localizedDescription) Retry before considering this iPhone disconnected."
            return
        }
        invalidateDashboardContext()
        user = nil
        currentCredentialID = nil
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
        selectedTab = .dashboard
        phase = .connectionRequired
    }

    func retryBootstrap() async {
        phase = .launching
        await bootstrap()
    }

    private func loadBootstrapIdentity() async throws -> MeData {
        let api = api
        let identityLoader = bootstrapIdentityLoader
        let timeout = bootstrapValidationTimeout

        return try await withThrowingTaskGroup(of: MeData.self) { group in
            group.addTask {
                try await identityLoader(api)
            }
            group.addTask {
                try await Task.sleep(for: timeout)
                throw BootstrapValidationError.timedOut
            }

            defer { group.cancelAll() }
            guard let identity = try await group.next() else {
                throw BootstrapValidationError.timedOut
            }
            return identity
        }
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
        guard !isDemo,
              phase == .ready,
              connectionValidated,
              !isRefreshingBriefing
        else { return }
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

    func refreshDashboard(timezone: TimeZone = .current) async {
        guard !isDemo,
              phase == .ready,
              connectionValidated,
              !isRefreshingDashboard
        else { return }
        let contextGeneration = dashboardContextGeneration
        isRefreshingDashboard = true
        defer {
            if contextGeneration == dashboardContextGeneration {
                isRefreshingDashboard = false
            }
        }
        do {
            let value = try await dashboardLoader(api, timezone.identifier)
            guard contextGeneration == dashboardContextGeneration,
                  connectionValidated,
                  phase == .ready
            else { return }
            dashboard = value
            dashboardMessage = nil
        } catch {
            guard contextGeneration == dashboardContextGeneration,
                  connectionValidated,
                  phase == .ready
            else { return }
            dashboardMessage = "Usage and access details could not be refreshed."
        }
    }

    func refreshDashboardIfNeeded(
        now: Date = .now,
        timezone: TimeZone = .current
    ) async {
        guard dashboard.map({
            Self.dashboardNeedsRefresh($0, now: now, timezone: timezone)
        }) ?? true else { return }
        await refreshDashboard(timezone: timezone)
    }

    nonisolated static func dashboardNeedsRefresh(
        _ dashboard: WorkspaceDashboardData,
        now: Date,
        timezone: TimeZone,
        maximumAge: TimeInterval = 5 * 60
    ) -> Bool {
        guard let generatedAt = parseDashboardTimestamp(dashboard.generatedAt) else {
            return true
        }
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = timezone
        guard calendar.isDate(generatedAt, inSameDayAs: now) else { return true }
        let age = now.timeIntervalSince(generatedAt)
        return age < -60 || age >= maximumAge
    }

    private nonisolated static func parseDashboardTimestamp(_ value: String) -> Date? {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = formatter.date(from: value) { return date }
        formatter.formatOptions = [.withInternetDateTime]
        return formatter.date(from: value)
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

    private func accept(_ identity: MeData) {
        invalidateDashboardContext()
        isDemo = false
        user = identity.user
        currentCredentialID = identity.credentialID
        readOnlyCredential = identity.readOnly
        connectionValidated = true
        phase = .ready
        tasks = []
        alerts = []
        connectionMessage = nil
    }

    private func invalidateDashboardContext() {
        dashboardContextGeneration &+= 1
        dashboard = nil
        dashboardMessage = nil
        isRefreshingDashboard = false
        connectionValidated = false
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
