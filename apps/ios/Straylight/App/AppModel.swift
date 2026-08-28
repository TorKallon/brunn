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
typealias NotificationListLoader = @Sendable (StraylightAPI, String?) async throws -> NotificationListResponse
typealias NotificationDetailLoader = @Sendable (StraylightAPI, String) async throws -> StraylightNotification
typealias NotificationReceiptWriter = @Sendable (
    StraylightAPI,
    String,
    NotificationReceiptKind,
    String?
) async throws -> NotificationReceiptResponse

@MainActor
private final class MessagingBearerState {
    var token: String?
}

enum AppPhase: Equatable {
    case launching
    case connectionRequired
    case ready
    case failed(String)
}

enum AppTab: Hashable {
    case dashboard
    case today
    case tasks
    case agents
    case alerts
    case archive
    case more
}

enum MessagingRefreshOutcome: Equatable {
    case newData
    case noData
    case failed
}

@MainActor
final class AppModel: ObservableObject {
    @Published private(set) var phase: AppPhase = .launching
    @Published private(set) var user: UserSummary?
    @Published private(set) var currentCredentialID: String?
    @Published private(set) var readOnlyCredential = false
    @Published private(set) var canManageNotifications = false
    @Published private(set) var canWriteTasks = false
    @Published private(set) var canWriteMessages = false
    @Published private(set) var messagingEnabled = false
    @Published private(set) var messagingMessage: String?
    @Published var focusedMessagingConversationID: String?
    @Published var focusedMessagingSequence: Int64?
    @Published private(set) var hasStoredDeviceTaskCredential = false
    @Published private(set) var deviceTaskAccessMessage: String?
    @Published private(set) var isConfiguringDeviceTaskAccess = false
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
    @Published private(set) var notifications: [StraylightNotification] = []
    @Published private(set) var notificationUnreadCount = 0
    @Published private(set) var notificationMessage: String?
    @Published private(set) var isRefreshingNotifications = false
    @Published private(set) var isLoadingMoreNotifications = false
    @Published private(set) var canLoadMoreNotifications = false
    @Published var presentedNotification: StraylightNotification?
    @Published private(set) var dashboard: WorkspaceDashboardData?
    @Published private(set) var dashboardMessage: String?
    @Published private(set) var isRefreshingDashboard = false
    @Published private(set) var searchResults: [WorkspaceSearchCandidate] = []
    @Published private(set) var searchEnvelopeStatus: String?
    @Published private(set) var searchMessage: String?
    @Published private(set) var isSearching = false
    @Published private(set) var tasks: [TaskItem] = []
    @Published private(set) var urgentTasks: [AgentTaskCandidate] = []
    @Published private(set) var nextTasks: [AgentTaskCandidate] = []
    @Published private(set) var doneToday: AgentTaskDoneSummaryData?
    @Published private(set) var taskContexts: [AgentTaskContext] = []
    @Published private(set) var selectedTaskContexts: Set<String> = []
    @Published private(set) var taskProjects: [AgentTaskProject] = []
    @Published private(set) var todoistStatus: AgentTaskTodoistStatus?
    @Published var selectedProjectState: AgentTaskProjectStateData?
    @Published private(set) var taskNextRemaining = 0
    @Published private(set) var taskBacklogTotal = 0
    @Published private(set) var taskMessage: String?
    @Published private(set) var isRefreshingTasks = false
    @Published private(set) var mutatingTaskRefs: Set<String> = []
    @Published var presentedTask: AgentTaskDetail?
    @Published private(set) var alerts: [AlertItem] = []
    @Published var selectedTab: AppTab = .dashboard
    @Published var focusedBriefingItemID: String?

    var newsItems: [BriefingNewsItem] {
        if !briefingActivity.isEmpty { return briefingActivity }
        guard let edition = latestBriefing else { return [] }
        return Self.projectNews(from: edition, uniqueIDs: false)
    }

    let api: StraylightAPI
    let messagingController: MessagingController?
    private let credentialStore: any CredentialStoring
    private let messagingBearerState: MessagingBearerState
    private let briefingCache: BriefingCache
    private let taskSurfaceCache: TaskSurfaceCache
    private let bootstrapValidationTimeout: Duration
    private let storedSessionChecker: StoredSessionChecker
    private let bootstrapIdentityLoader: BootstrapIdentityLoader
    private let loginLoader: LoginLoader
    private let dashboardLoader: DashboardLoader
    private let notificationListLoader: NotificationListLoader
    private let notificationDetailLoader: NotificationDetailLoader
    private let notificationReceiptWriter: NotificationReceiptWriter
    private var pendingRoute: AppRoute?
    private var nextBriefingHistoryPath: String?
    private var nextNotificationCursor: String?
    private var dashboardContextGeneration: UInt64 = 0
    private var searchContextGeneration: UInt64 = 0

    init(
        api: StraylightAPI = StraylightAPI(),
        credentialStore: any CredentialStoring = KeychainCredentialStore(),
        briefingCache: BriefingCache = BriefingCache(),
        taskSurfaceCache: TaskSurfaceCache = TaskSurfaceCache(),
        bootstrapValidationTimeout: Duration = .seconds(6),
        storedSessionChecker: @escaping StoredSessionChecker = { api in
            if api.hasAuthenticatedSession() { return true }
            // CFNetwork can rehydrate the persistent cookie store after this
            // actor's first synchronous read on a cold process launch. A
            // single session probe lets URLSession load and send that cookie
            // before treating the device as signed out.
            return (try? await api.authSession()) != nil
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
        },
        notificationListLoader: @escaping NotificationListLoader = { api, cursor in
            try await api.notifications(cursor: cursor)
        },
        notificationDetailLoader: @escaping NotificationDetailLoader = { api, reference in
            try await api.notification(reference: reference)
        },
        notificationReceiptWriter: @escaping NotificationReceiptWriter = {
            api, notificationRef, kind, deliveryRef in
            try await api.recordNotificationReceipt(
                notificationRef: notificationRef,
                kind: kind,
                deliveryRef: deliveryRef
            )
        },
        messagingController: MessagingController? = nil
    ) {
        let messagingBearerState = MessagingBearerState()
        self.api = api
        self.credentialStore = credentialStore
        self.messagingBearerState = messagingBearerState
        self.briefingCache = briefingCache
        self.taskSurfaceCache = taskSurfaceCache
        self.bootstrapValidationTimeout = bootstrapValidationTimeout
        self.storedSessionChecker = storedSessionChecker
        self.bootstrapIdentityLoader = bootstrapIdentityLoader
        self.loginLoader = loginLoader
        self.dashboardLoader = dashboardLoader
        self.notificationListLoader = notificationListLoader
        self.notificationDetailLoader = notificationDetailLoader
        self.notificationReceiptWriter = notificationReceiptWriter
        if let messagingController {
            self.messagingController = messagingController
        } else if let store = try? MessagingStore() {
            self.messagingController = MessagingController(
                store: store,
                transport: MessagingTransportOperations(
                    sync: { request in
                        try await api.messagingSync(request)
                    },
                    send: { conversationID, exactRequestData in
                        guard let bearerToken = await messagingBearerState.token,
                              !bearerToken.isEmpty
                        else { throw StraylightAPIError.notConnected }
                        return try await api.sendMessagingMessage(
                            conversationID: conversationID,
                            exactRequestData: exactRequestData,
                            bearerToken: bearerToken
                        )
                    },
                    createConversation: { request in
                        guard let bearerToken = await messagingBearerState.token,
                              !bearerToken.isEmpty
                        else { throw StraylightAPIError.notConnected }
                        return try await api.createMessagingConversation(
                            request,
                            bearerToken: bearerToken
                        )
                    },
                    markRead: { conversationID, request in
                        guard let bearerToken = await messagingBearerState.token,
                              !bearerToken.isEmpty
                        else { throw StraylightAPIError.notConnected }
                        return try await api.markMessagingRead(
                            conversationID: conversationID,
                            lastReadSeq: request.lastReadSeq,
                            bearerToken: bearerToken
                        )
                    }
                )
            )
        } else {
            self.messagingController = nil
        }
    }

    func bootstrap() async {
        messagingBearerState.token = nil
        if ProcessInfo.processInfo.arguments.contains("--ui-test-reset-task-contexts") {
            UserDefaults.standard.removeObject(forKey: Self.taskContextsDefaultsKey)
        }

        if ProcessInfo.processInfo.arguments.contains("--demo") {
            enterDemo()
            return
        }

        if ProcessInfo.processInfo.arguments.contains("--ui-test-connection-required") {
            phase = .connectionRequired
            return
        }

        invalidateDashboardContext()
        guard await storedSessionChecker(api) else {
            phase = .connectionRequired
            return
        }

        let cachedTaskUserID = await loadLocallyBoundTaskSurface()
        await loadCachedBriefing()
        let hasImmediatelyAvailableProtectedSurface = latestBriefing != nil
            || cachedTaskUserID != nil
        if hasImmediatelyAvailableProtectedSurface {
            user = UserSummary(
                id: cachedTaskUserID ?? "cached",
                displayName: "Owner"
            )
            connectionValidated = false
            phase = .ready
            connectionMessage = "Checking Straylight while the last protected Today view remains available."
            applyPendingRouteLocally()
        }

        // Restoring the SwiftData messaging cache can take longer than the
        // bounded Today cache read. Keep the existing instant-paint contract
        // by revealing an already verified Today surface before this work.
        let cachedMessagingUserID = await loadLocallyBoundMessagingSurface()
        let hasProtectedOfflineSurface = latestBriefing != nil
            || cachedTaskUserID != nil
            || cachedMessagingUserID != nil
        if hasProtectedOfflineSurface, !hasImmediatelyAvailableProtectedSurface {
            user = UserSummary(
                id: cachedTaskUserID ?? cachedMessagingUserID ?? "cached",
                displayName: "Owner"
            )
            connectionValidated = false
            phase = .ready
            connectionMessage = "Checking Straylight while the last protected Today view remains available."
            applyPendingRouteLocally()
        }

        do {
            let identity = try await loadBootstrapIdentity()
            accept(identity)
            if await loadCachedTaskSurface(for: identity.user.id) {
                await bindTaskSurfaceCache(to: identity.user.id)
            }
            await bindMessagingSurface(to: identity.user.id)
            await validateStoredDeviceTaskCredential()
            Task { await refreshDashboard() }
            Task { await refreshNotifications() }
            Task { await refreshTaskSurface() }
            Task { await refreshMessaging(.launch) }
            await resumePendingRoute()
            await refreshBriefing()
        } catch is BootstrapValidationError {
            connectionValidated = false
            if hasProtectedOfflineSurface {
                phase = .ready
                connectionMessage = "Showing the last protected Today view because the saved sign-in is taking too long to verify."
                applyPendingRouteLocally()
            } else {
                phase = .connectionRequired
                connectionMessage = "The saved sign-in is taking too long to verify. Sign in again, or retry when connectivity returns."
            }
        } catch let error as StraylightAPIError where error.isUnauthorized {
            await api.clearAuthenticatedSession()
            do {
                try await briefingCache.clear()
                try await taskSurfaceCache.clear()
                try messagingController?.clearActiveAccount()
                messagingEnabled = false
                messagingMessage = nil
                cacheSavedAt = nil
                cachedAt = nil
                latestBriefing = nil
                urgentTasks = []
                nextTasks = []
                doneToday = nil
                taskProjects = []
                todoistStatus = nil
                taskContexts = []
                selectedTaskContexts = []
                taskNextRemaining = 0
                taskBacklogTotal = 0
            } catch {
                phase = .failed("The expired sign-in was removed, but the protected cache could not be cleared. \(error.localizedDescription)")
                return
            }
            connectionValidated = false
            phase = .connectionRequired
            connectionMessage = "Your session expired. Sign in again."
        } catch {
            if hasProtectedOfflineSurface {
                if user == nil {
                    user = UserSummary(
                        id: cachedTaskUserID ?? cachedMessagingUserID ?? "cached",
                        displayName: "Owner"
                    )
                }
                connectionValidated = false
                phase = .ready
                connectionMessage = "Showing the last protected Today view because Straylight could not be reached."
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
        messagingBearerState.token = nil
        messagingController?.deactivate()
        messagingEnabled = false
        guard await api.clearAuthenticatedSession() else {
            phase = .connectionRequired
            connectionMessage = "The previous protected message upload could not be removed. Unlock this iPhone and retry before signing in."
            return
        }
        do {
            let identity = try await loginLoader(api, email, password)
            accept(identity)
            if await loadCachedTaskSurface(for: identity.user.id) {
                await bindTaskSurfaceCache(to: identity.user.id)
            }
            await bindMessagingSurface(to: identity.user.id)
            await validateStoredDeviceTaskCredential()
            Task { await refreshDashboard() }
            Task { await refreshNotifications() }
            await loadCachedBriefing()
            Task { await refreshTaskSurface() }
            Task { await refreshMessaging(.launch) }
            await resumePendingRoute()
            await refreshBriefing()
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
        messagingBearerState.token = nil
        messagingController?.deactivate()
        isDemo = true
        user = UserSummary(id: "user:demo", displayName: "Rourke")
        currentCredentialID = "credential:demo-iphone"
        canManageNotifications = true
        canWriteTasks = !ProcessInfo.processInfo.arguments.contains("--ui-test-task-read-only")
        canWriteMessages = false
        messagingEnabled = false
        messagingMessage = nil
        focusedMessagingConversationID = nil
        focusedMessagingSequence = nil
        hasStoredDeviceTaskCredential = false
        dashboard = SampleData.dashboard
        latestBriefing = SampleData.briefing
        tasks = SampleData.tasks
        urgentTasks = ProcessInfo.processInfo.arguments.contains("--ui-test-task-empty-urgent")
            ? []
            : SampleData.agentUrgentTasks
        nextTasks = SampleData.agentNextTasks
        doneToday = SampleData.agentDoneToday
        taskContexts = ProcessInfo.processInfo.arguments.contains("--ui-test-task-crowded-contexts")
            ? SampleData.agentTaskCrowdedContexts
            : SampleData.agentTaskContexts
        selectedTaskContexts = ["phone", "online"]
        taskProjects = SampleData.agentTaskProjects
        if ProcessInfo.processInfo.arguments.contains("--ui-test-todoist-error") {
            todoistStatus = AgentTaskTodoistStatus(
                environmentEnabled: true,
                savedMode: "pull",
                effectiveMode: "pull",
                tokenConfigured: true,
                configurationGeneration: 2,
                lastOutcome: "error",
                lastErrorCode: "todoist_apply_rejected"
            )
        } else {
            todoistStatus = AgentTaskTodoistStatus(
                environmentEnabled: false,
                savedMode: "off",
                effectiveMode: "off",
                tokenConfigured: false,
                configurationGeneration: 1
            )
        }
        taskNextRemaining = 7
        taskBacklogTotal = 18
        taskMessage = nil
        alerts = SampleData.alerts
        briefingHistory = SampleData.briefingHistory
        canLoadMoreBriefings = false
        nextBriefingHistoryPath = nil
        topicsSnapshot = SampleData.topicsSnapshot
        notifications = SampleData.notifications
        notificationUnreadCount = notifications.filter(\.isUnread).count
        nextNotificationCursor = nil
        canLoadMoreNotifications = false
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
            if case let .task(reference) = pendingRoute {
                presentedTask = SampleData.agentTaskDetail(reference: reference)
            }
        }
    }

    func disconnect() async {
        if !isDemo {
            let pendingCredentialRef = Self.pendingDeviceCredentialRef()
            let credential: DeviceTaskCredential?
            do {
                credential = try credentialStore.load()
            } catch {
                guard pendingCredentialRef != nil else {
                    canWriteTasks = false
                    canManageNotifications = false
                    canWriteMessages = false
                    messagingBearerState.token = nil
                    hasStoredDeviceTaskCredential = true
                    privacyMessage = "Disconnect stopped because the protected device credential could not be read for server revocation. Retry after unlocking this iPhone."
                    return
                }
                credential = nil
            }
            let references = Set([credential?.credentialRef, pendingCredentialRef].compactMap { $0 })
            for reference in references {
                do {
                    _ = try await api.revokeCredential(reference: reference)
                } catch {
                    canWriteTasks = false
                    canManageNotifications = false
                    canWriteMessages = false
                    messagingBearerState.token = nil
                    hasStoredDeviceTaskCredential = true
                    privacyMessage = "Disconnect stopped because device task access could not be revoked on Straylight. The protected Keychain credential remains locally disabled; retry while online."
                    return
                }
            }
        }
        try? await api.logout()
        guard await api.clearAuthenticatedSession() else {
            privacyMessage = "Disconnect is incomplete: protected queued message data could not be removed. Unlock this iPhone and retry before considering it disconnected."
            return
        }
        do {
            try credentialStore.delete()
            Self.clearPendingDeviceCredentialRef()
            try await briefingCache.clear()
            try await taskSurfaceCache.clear()
            try messagingController?.clearActiveAccount()
        } catch {
            privacyMessage = "Disconnect is incomplete: private local data could not be removed. \(error.localizedDescription) Retry before considering this iPhone disconnected."
            return
        }
        invalidateDashboardContext()
        user = nil
        currentCredentialID = nil
        canManageNotifications = false
        canWriteTasks = false
        canWriteMessages = false
        messagingBearerState.token = nil
        messagingEnabled = false
        messagingMessage = nil
        focusedMessagingConversationID = nil
        focusedMessagingSequence = nil
        hasStoredDeviceTaskCredential = false
        deviceTaskAccessMessage = nil
        latestBriefing = nil
        briefingHistory = []
        canLoadMoreBriefings = false
        nextBriefingHistoryPath = nil
        topicsSnapshot = nil
        readNewsItemIDs = []
        briefingActivity = []
        notifications = []
        notificationUnreadCount = 0
        nextNotificationCursor = nil
        canLoadMoreNotifications = false
        notificationMessage = nil
        presentedNotification = nil
        tasks = []
        urgentTasks = []
        nextTasks = []
        doneToday = nil
        taskContexts = []
        selectedTaskContexts = []
        taskProjects = []
        todoistStatus = nil
        selectedProjectState = nil
        taskNextRemaining = 0
        taskBacklogTotal = 0
        taskMessage = nil
        mutatingTaskRefs = []
        presentedTask = nil
        alerts = []
        searchResults = []
        searchContextGeneration &+= 1
        searchEnvelopeStatus = nil
        searchMessage = nil
        isSearching = false
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

    private func loadLocallyBoundMessagingSurface() async -> String? {
        guard let messagingController,
              let sessionFingerprint = await api.authenticatedSessionFingerprint()
        else {
            messagingEnabled = false
            return nil
        }
        do {
            guard try messagingController.activateCachedSession(
                sessionFingerprint: sessionFingerprint
            ) else {
                messagingEnabled = false
                return nil
            }
            messagingEnabled = messagingController.messagingEnabled
            messagingMessage = nil
            if messagingEnabled, let activeAccountID = messagingController.activeAccountID {
                do {
                    if let credential = try credentialStore.load(),
                       credential.userID == activeAccountID,
                       let capabilities = credential.capabilities,
                       Set(capabilities) == Self.messagingDeviceCapabilities,
                       Self.hasApprovedDeviceCapabilities(capabilities),
                       !credential.token.isEmpty
                    {
                        applyDeviceCapabilities(capabilities, bearerToken: credential.token)
                    }
                } catch {
                    canWriteMessages = false
                    messagingBearerState.token = nil
                    messagingMessage = "The protected device credential could not be read; Agents remains view only."
                }
            }
            return messagingController.activeAccountID
        } catch {
            messagingController.deactivate()
            messagingEnabled = false
            messagingMessage = "The protected Agents cache could not be verified and remains hidden."
            return nil
        }
    }

    private func bindMessagingSurface(to userID: String) async {
        let sessionFingerprint = await api.authenticatedSessionFingerprint()
        guard let messagingController,
              let sessionFingerprint
        else {
            messagingEnabled = false
            return
        }
        do {
            try messagingController.bindValidatedAccount(
                accountID: userID,
                sessionFingerprint: sessionFingerprint
            )
            let status = try await api.messagingStatus()
            let enabled = status.featureFlags?.messagingEnabled == true
            try messagingController.setMessagingEnabled(enabled)
            messagingEnabled = enabled
            if !enabled {
                try messagingController.selectConversation(nil)
                focusedMessagingConversationID = nil
                focusedMessagingSequence = nil
                if selectedTab == .agents { selectedTab = .dashboard }
            }
            messagingMessage = nil
        } catch {
            // If the authenticated identity is valid but status is temporarily
            // unreachable, retain only an exact same-session cached gate.
            messagingEnabled = messagingController.messagingEnabled
            messagingMessage = messagingEnabled
                ? "Showing the protected Agents cache while messaging reconnects."
                : nil
        }
    }

    @discardableResult
    func refreshMessaging(
        _ trigger: MessagingSyncTrigger = .pullToRefresh
    ) async -> MessagingRefreshOutcome {
        if trigger == .foreground {
            await refreshMessagingRuntimeGate()
        }
        guard messagingEnabled, let messagingController else { return .noData }
        do {
            if canWriteMessages {
                try await messagingController.flushOutbox()
            }
            let response = try await messagingController.refresh(trigger)
            messagingMessage = nil
            return response.messages.isEmpty && response.conversations.isEmpty
                ? .noData
                : .newData
        } catch {
            messagingMessage = "Agents could not refresh. The protected local copy remains available."
            return .failed
        }
    }

    private func refreshMessagingRuntimeGate() async {
        guard connectionValidated, let messagingController else { return }
        do {
            let status = try await api.messagingStatus()
            let enabled = status.featureFlags?.messagingEnabled == true
            try messagingController.setMessagingEnabled(enabled)
            messagingEnabled = enabled
            if !enabled {
                try messagingController.selectConversation(nil)
                focusedMessagingConversationID = nil
                focusedMessagingSequence = nil
                if selectedTab == .agents { selectedTab = .dashboard }
            }
        } catch {
            // A status transport failure is not evidence that the server gate
            // changed. Retain only the account/session-bound cached decision.
            messagingEnabled = messagingController.messagingEnabled
        }
    }

    @discardableResult
    func createMessagingConversation(
        participants: [String],
        subject: String?
    ) async throws -> String {
        guard messagingEnabled, canWriteMessages, let messagingController else {
            throw StraylightAPIError.notConnected
        }
        let conversation = try await messagingController.createConversation(
            participants: participants,
            subject: subject
        )
        focusedMessagingConversationID = conversation.conversationID
        focusedMessagingSequence = nil
        return conversation.conversationID
    }

    func focusMessagingConversation(_ conversationID: String, sequence: Int64? = nil) {
        guard messagingEnabled, let messagingController else { return }
        selectedTab = .agents
        focusedMessagingConversationID = conversationID
        focusedMessagingSequence = sequence
        try? messagingController.selectConversation(conversationID)
    }

    func bootstrapDeviceTaskAccess() async {
        guard !isConfiguringDeviceTaskAccess else { return }
        guard !isDemo else {
            deviceTaskAccessMessage = "Task actions can be enabled after connecting this iPhone to Straylight."
            return
        }
        guard connectionValidated else {
            deviceTaskAccessMessage = "Reconnect to Straylight before enabling task actions."
            return
        }
        isConfiguringDeviceTaskAccess = true
        defer { isConfiguringDeviceTaskAccess = false }
        var createdCredentialRef: String?
        do {
            if hasStoredDeviceTaskCredential {
                await validateStoredDeviceTaskCredential()
                if canWriteTasks { return }
                guard !hasStoredDeviceTaskCredential else { return }
            }
            if Self.pendingDeviceCredentialRef() != nil {
                hasStoredDeviceTaskCredential = true
                deviceTaskAccessMessage = "Pending device access must be revoked before a replacement can be created."
                return
            }
            let existingCredential: DeviceTaskCredential?
            do {
                existingCredential = try credentialStore.load()
            } catch {
                hasStoredDeviceTaskCredential = true
                deviceTaskAccessMessage = "The protected device credential could not be read. Unlock this iPhone and retry revocation before creating replacement access."
                return
            }
            if existingCredential != nil {
                hasStoredDeviceTaskCredential = true
                deviceTaskAccessMessage = "Existing device access must be revoked before a replacement can be created."
                return
            }
            hasStoredDeviceTaskCredential = false
            var oneTimeResponse: DeviceTaskCredentialBootstrapResponse? =
                try await api.bootstrapDeviceTaskCredential()
            createdCredentialRef = oneTimeResponse?.id
            guard oneTimeResponse?.access == "ios_tasks",
                  Self.hasApprovedDeviceCapabilities(oneTimeResponse?.capabilities ?? []),
                  oneTimeResponse?.id.hasPrefix("credential:") == true,
                  oneTimeResponse?.token.isEmpty == false,
                  let credentialRef = oneTimeResponse?.id
            else {
                throw StraylightAPIError.invalidResponse
            }
            var oneTimeToken = oneTimeResponse?.token ?? ""
            let issuedCapabilities = oneTimeResponse?.capabilities ?? []
            oneTimeResponse = nil
            defer { oneTimeToken.removeAll(keepingCapacity: false) }
            if Set(issuedCapabilities) == Self.messagingDeviceCapabilities {
                let owners = try await api.messagingAgents().agents.filter {
                    !$0.archived && $0.principalKind == "owner"
                }
                guard owners.count == 1 else {
                    throw StraylightAPIError.invalidResponse
                }
                _ = try await api.bindMessagingCredential(
                    agentID: owners[0].agentID,
                    credentialReference: credentialRef
                )
            }
            let identity = try await api.deviceCredentialIdentity(
                bearerToken: oneTimeToken
            )
            guard identity.user.id == user?.id,
                  identity.credentialID == credentialRef,
                  Self.hasApprovedDeviceCapabilities(identity.capabilities)
            else {
                throw StraylightAPIError.invalidResponse
            }
            try credentialStore.save(DeviceTaskCredential(
                credentialRef: credentialRef,
                token: oneTimeToken,
                userID: identity.user.id,
                capabilities: identity.capabilities.sorted()
            ))
            hasStoredDeviceTaskCredential = true
            applyDeviceCapabilities(identity.capabilities, bearerToken: oneTimeToken)
            deviceTaskAccessMessage = canWriteMessages
                ? "Device task and messaging access is ready."
                : "Device task access is ready."
        } catch {
            var revocationUnconfirmed = false
            if let createdCredentialRef {
                do {
                    _ = try await api.revokeCredential(reference: createdCredentialRef)
                    try credentialStore.delete()
                    Self.clearPendingDeviceCredentialRef()
                    hasStoredDeviceTaskCredential = false
                } catch {
                    revocationUnconfirmed = true
                    Self.retainPendingDeviceCredentialRef(createdCredentialRef)
                    do {
                        try credentialStore.save(DeviceTaskCredential(
                            credentialRef: createdCredentialRef,
                            token: ""
                        ))
                        hasStoredDeviceTaskCredential = true
                    } catch {
                        hasStoredDeviceTaskCredential = true
                    }
                }
            }
            canWriteTasks = false
            canManageNotifications = false
            canWriteMessages = false
            messagingBearerState.token = nil
            deviceTaskAccessMessage = revocationUnconfirmed
                ? "Device task access failed after issuance, and automatic revocation could not be confirmed. Revoke “iOS task access” from the web before retrying."
                : "Device task access could not be created. \(error.localizedDescription)"
        }
    }

    func revokeDeviceTaskAccess() async -> Bool {
        guard !isDemo else { return true }
        do {
            let credential = try credentialStore.load()
            let references = Set([
                credential?.credentialRef,
                Self.pendingDeviceCredentialRef(),
            ].compactMap { $0 })
            for reference in references {
                _ = try await api.revokeCredential(reference: reference)
            }
            try credentialStore.delete()
            Self.clearPendingDeviceCredentialRef()
            hasStoredDeviceTaskCredential = false
            canWriteTasks = false
            canManageNotifications = false
            canWriteMessages = false
            messagingBearerState.token = nil
            deviceTaskAccessMessage = "Device task access was revoked."
            return true
        } catch {
            canWriteTasks = false
            canManageNotifications = false
            canWriteMessages = false
            messagingBearerState.token = nil
            hasStoredDeviceTaskCredential = true
            deviceTaskAccessMessage = "Revocation could not be confirmed. The local credential remains protected and disabled."
            return false
        }
    }

    func deviceTaskBearer() -> String? {
        guard canWriteTasks || canManageNotifications || canWriteMessages else { return nil }
        guard let token = try? credentialStore.load()?.token, !token.isEmpty else { return nil }
        return token
    }

    private func validateStoredDeviceTaskCredential() async {
        canWriteTasks = false
        canManageNotifications = false
        canWriteMessages = false
        messagingBearerState.token = nil
        if let pendingCredentialRef = Self.pendingDeviceCredentialRef() {
            hasStoredDeviceTaskCredential = true
            do {
                _ = try await api.revokeCredential(reference: pendingCredentialRef)
                try credentialStore.delete()
                Self.clearPendingDeviceCredentialRef()
                hasStoredDeviceTaskCredential = false
                deviceTaskAccessMessage = "Pending device access was revoked and removed."
            } catch {
                deviceTaskAccessMessage = "Pending device access remains disabled because server revocation could not be confirmed."
            }
            return
        }
        let credential: DeviceTaskCredential?
        do {
            credential = try credentialStore.load()
        } catch {
            hasStoredDeviceTaskCredential = true
            deviceTaskAccessMessage = "The protected device credential could not be read. Unlock this iPhone and retry before creating replacement access."
            return
        }
        guard let credential else {
            hasStoredDeviceTaskCredential = false
            return
        }
        hasStoredDeviceTaskCredential = true
        if credential.token.isEmpty {
            do {
                _ = try await api.revokeCredential(reference: credential.credentialRef)
                try credentialStore.delete()
                hasStoredDeviceTaskCredential = false
                deviceTaskAccessMessage = "Pending device access was revoked and removed."
            } catch {
                deviceTaskAccessMessage = "Pending device access remains disabled because server revocation could not be confirmed."
            }
            return
        }
        do {
            let identity = try await api.deviceCredentialIdentity(
                bearerToken: credential.token
            )
            guard identity.user.id == user?.id,
                  identity.credentialID == credential.credentialRef,
                  Self.hasApprovedDeviceCapabilities(identity.capabilities)
            else {
                do {
                    _ = try await api.revokeCredential(reference: credential.credentialRef)
                    try credentialStore.delete()
                    hasStoredDeviceTaskCredential = false
                    deviceTaskAccessMessage = "The saved device credential did not match this account or was not least-privilege, so it was revoked and removed."
                } catch {
                    deviceTaskAccessMessage = "The saved device credential did not match this account or was not least-privilege. It remains protected but disabled because server revocation could not be confirmed."
                }
                return
            }
            try credentialStore.save(DeviceTaskCredential(
                credentialRef: credential.credentialRef,
                token: credential.token,
                userID: identity.user.id,
                capabilities: identity.capabilities.sorted()
            ))
            applyDeviceCapabilities(identity.capabilities, bearerToken: credential.token)
            deviceTaskAccessMessage = nil
        } catch let error as StraylightAPIError where error.isUnauthorized {
            do {
                _ = try await api.revokeCredential(reference: credential.credentialRef)
                try credentialStore.delete()
                hasStoredDeviceTaskCredential = false
                deviceTaskAccessMessage = "Device task access expired and was removed. Set it up again."
            } catch {
                deviceTaskAccessMessage = "Device task access is invalid and disabled; server revocation could not be confirmed."
            }
        } catch {
            deviceTaskAccessMessage = "Device task access could not be verified; mutations remain disabled."
        }
    }

    private func loadLocallyBoundTaskSurface() async -> String? {
        do {
            guard let sessionFingerprint = await api.authenticatedSessionFingerprint(),
                  let userID = try await taskSurfaceCache.boundUserID(
                      matching: sessionFingerprint
                  )
            else { return nil }
            guard await loadCachedTaskSurface(for: userID) else {
                try? await taskSurfaceCache.clear()
                return nil
            }
            return userID
        } catch {
            clearTaskSurfacePresentation()
            try? await taskSurfaceCache.clear()
            taskMessage = "The protected task cache could not be verified and was removed."
            return nil
        }
    }

    @discardableResult
    private func loadCachedTaskSurface(for userID: String) async -> Bool {
        do {
            guard let cached = try await taskSurfaceCache.load() else { return false }
            guard cached.userID == userID else {
                clearTaskSurfacePresentation()
                try await taskSurfaceCache.clear()
                taskMessage = "A protected Tasks cache for another account was removed."
                return false
            }
            urgentTasks = cached.urgent
            nextTasks = cached.next
            doneToday = cached.doneToday
            taskProjects = cached.projects
            taskContexts = cached.contexts
            selectedTaskContexts = Set(cached.selectedContexts)
            taskNextRemaining = cached.nextRemaining
            taskBacklogTotal = cached.backlogTotal
            return true
        } catch {
            clearTaskSurfacePresentation()
            try? await taskSurfaceCache.clear()
            taskMessage = "The protected task cache could not be verified and was removed."
            return false
        }
    }

    private func bindTaskSurfaceCache(to userID: String) async {
        do {
            guard let sessionFingerprint = await api.authenticatedSessionFingerprint() else {
                throw StraylightAPIError.notConnected
            }
            try await taskSurfaceCache.bind(
                to: userID,
                sessionFingerprint: sessionFingerprint
            )
        } catch {
            taskMessage = taskMessage
                ?? "Tasks are available, but their protected account binding could not be saved for offline launch."
        }
    }

    func refreshTaskSurface() async {
        guard !isDemo,
              phase == .ready,
              connectionValidated,
              !isRefreshingTasks
        else { return }
        isRefreshingTasks = true
        defer { isRefreshingTasks = false }

        do {
            let contextResponse = try await api.taskContexts()
            taskContexts = contextResponse.data.contexts.filter { !$0.archived }
            let validSlugs = Set(taskContexts.map(\.slug))
            let defaults = contextResponse.data.surfaceDefaults["ios"]?.contextsAvailable
                ?? ["phone", "online"]
            selectedTaskContexts = Set(defaults.filter(validSlugs.contains))
            UserDefaults.standard.removeObject(forKey: Self.taskContextsDefaultsKey)

            let selected = selectedTaskContexts.sorted()
            async let urgentResponse = api.taskCandidates(
                view: .urgent,
                contextsAvailable: selected
            )
            async let nextResponse = api.taskCandidates(
                view: .next,
                limit: 17,
                contextsAvailable: selected
            )
            async let doneResponse = api.taskDoneSummary(limit: 25)
            async let projectResponse = api.taskProjects()
            async let todoistStatusResponse: WorkspaceEnvelope<AgentTaskTodoistStatus>? =
                try? api.taskTodoistStatus()

            let (urgent, next, done, projects) = try await (
                urgentResponse,
                nextResponse,
                doneResponse,
                projectResponse
            )
            urgentTasks = urgent.data.items
            nextTasks = next.data.items
            doneToday = done.data
            taskProjects = projects.data.projects
            todoistStatus = await todoistStatusResponse?.data
            taskNextRemaining = next.data.nextRemaining
            taskBacklogTotal = next.data.backlogTotal
            taskMessage = nil
            try await saveTaskSurfaceCache()
        } catch {
            taskMessage = "Tasks could not refresh. The last protected task set remains visible."
        }
    }

    @discardableResult
    func performTaskAction(
        _ candidate: AgentTaskCandidate,
        operation: AgentTaskUpdateOperation
    ) async -> Bool {
        guard canWriteTasks else {
            taskMessage = "View only — this credential does not have task.write."
            return false
        }
        guard isDemo || connectionValidated else {
            taskMessage = "Reconnect before changing a task. Offline changes are not queued."
            return false
        }
        guard !mutatingTaskRefs.contains(candidate.taskRef) else { return false }

        let oldUrgent = urgentTasks
        let oldNext = nextTasks
        let removesFromToday: Bool
        switch operation {
        case .complete, .snooze, .snoozeUntil, .waitOn:
            removesFromToday = true
        default:
            removesFromToday = false
        }
        if removesFromToday {
            urgentTasks.removeAll { $0.taskRef == candidate.taskRef }
            nextTasks.removeAll { $0.taskRef == candidate.taskRef }
        }
        mutatingTaskRefs.insert(candidate.taskRef)
        defer { mutatingTaskRefs.remove(candidate.taskRef) }

        if isDemo {
            applyDemoTaskAction(candidate, operation: operation)
            taskMessage = nil
            return true
        }

        do {
            guard let bearerToken = deviceTaskBearer() else {
                throw StraylightAPIError.notConnected
            }
            let response = try await api.updateTask(
                reference: candidate.taskRef,
                request: AgentTaskUpdateRequest(
                    expectedVersion: candidate.version,
                    operation: operation
                ),
                bearerToken: bearerToken
            )
            if presentedTask?.taskRef == candidate.taskRef {
                presentedTask = response.task
            }
            if let doneTodayCount = response.doneTodayCount,
               let current = doneToday
            {
                doneToday = AgentTaskDoneSummaryData(
                    from: current.from,
                    through: current.through,
                    timezone: current.timezone,
                    asOf: current.asOf,
                    count: doneTodayCount,
                    doneTodayCount: doneTodayCount,
                    items: current.items,
                    nextCursor: current.nextCursor
                )
            }
            taskMessage = nil
            await refreshTaskSurface()
            return true
        } catch let error as StraylightAPIError {
            urgentTasks = oldUrgent
            nextTasks = oldNext
            if error.isUnauthorized {
                canWriteTasks = false
                canManageNotifications = false
                await validateStoredDeviceTaskCredential()
                taskMessage = "Device task access is no longer valid. Set it up again before changing tasks."
            } else if case let .server(status, _, _) = error, status == 409 {
                presentedTask = try? await api.task(reference: candidate.taskRef)
                taskMessage = "This task changed elsewhere. Its current version has been reloaded."
            } else {
                taskMessage = error.localizedDescription
            }
            return false
        } catch {
            urgentTasks = oldUrgent
            nextTasks = oldNext
            taskMessage = error.localizedDescription
            return false
        }
    }

    func openTask(reference: String) async {
        guard let canonical = TaskReference.canonical(reference) else {
            taskMessage = "The task link was invalid."
            return
        }
        selectedTab = .tasks
        if isDemo {
            presentedTask = SampleData.agentTaskDetail(reference: canonical)
            return
        }
        guard connectionValidated else {
            pendingRoute = .task(reference: canonical)
            return
        }
        do {
            presentedTask = try await api.task(reference: canonical)
            taskMessage = nil
        } catch {
            taskMessage = "The linked task could not be loaded. \(error.localizedDescription)"
        }
    }

    func loadProject(_ project: AgentTaskProject) async {
        if isDemo {
            selectedProjectState = SampleData.agentProjectState(project)
            return
        }
        do {
            selectedProjectState = try await api.taskProjectState(slug: project.slug)
            taskMessage = nil
        } catch {
            taskMessage = "Project state could not be loaded."
        }
    }

    func setProjectInterest(_ interest: String) async {
        guard canWriteTasks else {
            taskMessage = "View only — task.write is required to change project interest."
            return
        }
        guard let state = selectedProjectState else { return }
        if isDemo {
            taskMessage = nil
            return
        }
        do {
            guard let bearerToken = deviceTaskBearer() else {
                throw StraylightAPIError.notConnected
            }
            try await api.setTaskProjectInterest(
                slug: state.project.slug,
                interest: interest,
                expectedVersion: state.project.version,
                bearerToken: bearerToken
            )
            if let project = taskProjects.first(where: { $0.slug == state.project.slug }) {
                await loadProject(project)
            }
            await refreshTaskSurface()
        } catch let error as StraylightAPIError where error.isUnauthorized {
            canWriteTasks = false
            canManageNotifications = false
            await validateStoredDeviceTaskCredential()
            taskMessage = "Device task access is no longer valid. Set it up again before changing project interest."
        } catch {
            taskMessage = "Project interest changed elsewhere. Refresh and try again."
            await refreshTaskSurface()
        }
    }

    private func applyDemoTaskAction(
        _ candidate: AgentTaskCandidate,
        operation: AgentTaskUpdateOperation
    ) {
        switch operation {
        case .complete:
            let now = ISO8601DateFormatter().string(from: .now)
            let item = AgentTaskDoneItem(
                taskRef: candidate.taskRef,
                entryRef: candidate.entryRef,
                version: candidate.version + 1,
                title: candidate.title,
                doneAt: now,
                completedVia: "ios"
            )
            let current = doneToday ?? SampleData.agentDoneToday
            doneToday = AgentTaskDoneSummaryData(
                from: current.from,
                through: current.through,
                timezone: current.timezone,
                asOf: now,
                count: current.count + 1,
                doneTodayCount: current.doneTodayCount + 1,
                items: [item] + current.items,
                nextCursor: nil
            )
        case .pinToday, .unpin:
            let pinned = operation == .pinToday
            let updated = AgentTaskCandidate(
                taskRef: candidate.taskRef,
                entryRef: candidate.entryRef,
                version: candidate.version + 1,
                title: candidate.title,
                status: candidate.status,
                project: candidate.project,
                requiredContexts: candidate.requiredContexts,
                tier: candidate.tier,
                reason: candidate.reason,
                provenanceMarkers: candidate.provenanceMarkers,
                pinned: pinned
            )
            urgentTasks = urgentTasks.map { $0.taskRef == candidate.taskRef ? updated : $0 }
            nextTasks = nextTasks.map { $0.taskRef == candidate.taskRef ? updated : $0 }
        default:
            break
        }
    }

    private func saveTaskSurfaceCache() async throws {
        guard let userID = user?.id,
              let sessionFingerprint = await api.authenticatedSessionFingerprint()
        else { throw StraylightAPIError.notConnected }
        try await taskSurfaceCache.save(CachedTaskSurface(
            userID: userID,
            savedAt: .now,
            urgent: urgentTasks,
            next: nextTasks,
            doneToday: doneToday,
            projects: taskProjects,
            contexts: taskContexts,
            selectedContexts: selectedTaskContexts.sorted(),
            nextRemaining: taskNextRemaining,
            backlogTotal: taskBacklogTotal
        ), sessionFingerprint: sessionFingerprint)
    }

    private static let taskContextsDefaultsKey = "straylight.task-contexts.ios"
    private static var pendingDeviceCredentialRefKey: String {
        let namespace = ProcessInfo.processInfo.environment["STRAYLIGHT_CREDENTIAL_NAMESPACE"]
            .flatMap { $0.isEmpty ? nil : $0 }
        let base = "straylight.pending-device-task-credential-ref.v1"
        return namespace.map { "\(base).\($0)" } ?? base
    }
    private static let legacyDeviceTaskCapabilities: Set<String> = [
        "task.write",
        "notification:manage",
    ]
    private static let messagingDeviceCapabilities: Set<String> = [
        "task.write",
        "message.write",
        "notification:manage",
    ]

    private static func hasApprovedDeviceCapabilities(_ capabilities: [String]) -> Bool {
        let values = Set(capabilities)
        return capabilities.count == values.count
            && (values == legacyDeviceTaskCapabilities || values == messagingDeviceCapabilities)
    }

    private func applyDeviceCapabilities(_ capabilities: [String], bearerToken: String) {
        let values = Set(capabilities)
        canWriteTasks = values.contains("task.write")
        canManageNotifications = values.contains("notification:manage")
        canWriteMessages = values == Self.messagingDeviceCapabilities
        messagingBearerState.token = canWriteMessages ? bearerToken : nil
    }

    private static func pendingDeviceCredentialRef() -> String? {
        UserDefaults.standard.string(forKey: pendingDeviceCredentialRefKey)
    }

    private static func retainPendingDeviceCredentialRef(_ reference: String) {
        UserDefaults.standard.set(reference, forKey: pendingDeviceCredentialRefKey)
    }

    private static func clearPendingDeviceCredentialRef() {
        UserDefaults.standard.removeObject(forKey: pendingDeviceCredentialRefKey)
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

    func refreshNotifications() async {
        guard !isDemo,
              phase == .ready,
              connectionValidated,
              !isRefreshingNotifications
        else { return }

        isRefreshingNotifications = true
        defer { isRefreshingNotifications = false }
        do {
            let response = try await notificationListLoader(api, nil)
            notifications = response.items
            notificationUnreadCount = response.unreadCount
            nextNotificationCursor = response.nextCursor
            canLoadMoreNotifications = response.nextCursor != nil
            notificationMessage = nil
        } catch {
            notificationMessage = "Alerts could not be refreshed."
        }
    }

    func loadMoreNotifications() async {
        guard !isDemo,
              phase == .ready,
              connectionValidated,
              canLoadMoreNotifications,
              !isLoadingMoreNotifications,
              let nextNotificationCursor
        else { return }

        isLoadingMoreNotifications = true
        defer { isLoadingMoreNotifications = false }
        do {
            let response = try await notificationListLoader(api, nextNotificationCursor)
            let existing = Set(notifications.map(\.notificationRef))
            notifications.append(contentsOf: response.items.filter {
                !existing.contains($0.notificationRef)
            })
            notificationUnreadCount = response.unreadCount
            self.nextNotificationCursor = response.nextCursor
            canLoadMoreNotifications = response.nextCursor != nil
            notificationMessage = nil
        } catch {
            notificationMessage = "Older alerts could not be loaded."
        }
    }

    func openNotification(
        reference: String,
        deliveryRef: String? = nil
    ) async {
        selectedTab = .alerts
        notificationMessage = nil
        guard PushReference.isNotification(reference),
              deliveryRef.map(PushReference.isDelivery) ?? true
        else {
            notificationMessage = "The alert link was invalid."
            return
        }

        do {
            let notification: StraylightNotification
            if isDemo {
                guard let demo = SampleData.notifications.first(where: {
                    $0.notificationRef == reference
                }) else {
                    notificationMessage = "This alert is not available in the demo."
                    return
                }
                notification = demo
            } else {
                guard connectionValidated else {
                    pendingRoute = .notification(
                        notificationRef: reference,
                        deliveryRef: deliveryRef
                    )
                    return
                }
                notification = try await notificationDetailLoader(api, reference)
            }

            upsertNotification(notification)
            presentedNotification = notification

            // A push tap carries delivery-specific evidence even when the
            // user-level inbox item was opened earlier. Only list/detail opens
            // without a delivery reference may skip the idempotent receipt.
            guard notification.openedAt == nil || deliveryRef != nil else { return }
            if isDemo {
                applyLocalReceipt(
                    notificationRef: reference,
                    openedAt: ISO8601DateFormatter().string(from: .now),
                    acknowledgedAt: notification.acknowledgedAt
                )
                return
            }

            do {
                let receipt = try await notificationReceiptWriter(
                    api,
                    reference,
                    .opened,
                    deliveryRef
                )
                applyLocalReceipt(
                    notificationRef: reference,
                    openedAt: receipt.openedAt,
                    acknowledgedAt: receipt.acknowledgedAt
                )
            } catch {
                notificationMessage = "The alert opened, but its read receipt could not be saved."
            }
        } catch {
            notificationMessage = "The linked alert could not be loaded. \(error.localizedDescription)"
        }
    }

    func acknowledgeNotification(_ notificationRef: String) async {
        guard let notification = notifications.first(where: {
            $0.notificationRef == notificationRef
        }) ?? (presentedNotification?.notificationRef == notificationRef
            ? presentedNotification
            : nil)
        else { return }

        if isDemo {
            applyLocalReceipt(
                notificationRef: notificationRef,
                openedAt: notification.openedAt ?? ISO8601DateFormatter().string(from: .now),
                acknowledgedAt: ISO8601DateFormatter().string(from: .now)
            )
            return
        }

        do {
            let receipt = try await notificationReceiptWriter(
                api,
                notificationRef,
                .acknowledged,
                nil
            )
            applyLocalReceipt(
                notificationRef: notificationRef,
                openedAt: receipt.openedAt,
                acknowledgedAt: receipt.acknowledgedAt
            )
            notificationMessage = nil
        } catch {
            notificationMessage = "This alert could not be acknowledged."
        }
    }

    func openNotificationTarget(_ notification: StraylightNotification) async {
        switch notification.target.type {
        case .notification:
            return
        case .today:
            presentedNotification = nil
            selectedTab = .today
        case .briefing:
            guard let date = notification.target.date,
                  let edition = notification.target.edition
            else {
                notificationMessage = "This alert has an incomplete briefing target."
                return
            }
            do {
                latestBriefing = try await loadBriefing(
                    date: date,
                    edition: edition
                )
                focusedBriefingItemID = notification.target.itemID
                presentedNotification = nil
                selectedTab = .today
            } catch {
                notificationMessage = "The linked briefing could not be loaded."
            }
        case .entry:
            return
        case .task:
            guard let taskRef = notification.target.taskRef,
                  TaskReference.canonical(taskRef) != nil
            else {
                notificationMessage = "This alert has an invalid task target."
                return
            }
            presentedNotification = nil
            await openTask(reference: taskRef)
        case .conversation:
            guard let route = notification.target.appRoute else {
                notificationMessage = "This alert has an invalid conversation target."
                return
            }
            presentedNotification = nil
            await handle(route)
        }
    }

    func readNotificationEntry(_ notification: StraylightNotification) async throws -> WorkspaceReadItem {
        guard notification.target.type == .entry,
              let entryRef = notification.target.entryRef ?? notification.source?.reference
        else {
            throw StraylightAPIError.invalidResponse
        }
        let exactReference = notification.source?.versionRef ?? entryRef
        if isDemo {
            return WorkspaceReadItem(
                reference: exactReference,
                path: "Notification source",
                title: notification.title,
                text: notification.body
            )
        }
        return try await api.read(
            reference: exactReference,
            path: nil,
            version: nil
        )
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

    func performSearch(
        _ query: String,
        sort: WorkspaceSearchSort = .bestMatch
    ) async {
        let query = query.trimmingCharacters(in: .whitespacesAndNewlines)
        searchContextGeneration &+= 1
        let context = searchContextGeneration
        searchResults = []
        searchEnvelopeStatus = nil
        guard query.count >= 2 else {
            isSearching = false
            searchMessage = "Enter at least two characters."
            return
        }
        isSearching = true
        searchMessage = nil
        defer {
            if searchContextGeneration == context {
                isSearching = false
            }
        }

        if isDemo {
            let matches = SampleData.searchResults.filter {
                $0.title.localizedCaseInsensitiveContains(query)
                    || $0.previewText.localizedCaseInsensitiveContains(query)
                    || query.localizedCaseInsensitiveContains("straylight")
            }
            searchResults = WorkspaceSearchOrdering.sorted(
                matches.isEmpty ? SampleData.searchResults : matches,
                by: sort
            )
            searchEnvelopeStatus = "complete"
            return
        }

        do {
            let response = try await api.search(query, sort: sort)
            guard searchContextGeneration == context else { return }
            searchEnvelopeStatus = response.status
            // The server owns the wire-order contract, including its exact
            // scalar title ordering and relevance tie-breakers. Local sorting
            // is reserved for deterministic demo/legacy fixtures.
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
            guard searchContextGeneration == context else { return }
            searchResults = []
            searchEnvelopeStatus = nil
            searchMessage = error.localizedDescription
        }
    }

    func clearSearch() {
        searchContextGeneration &+= 1
        searchResults = []
        searchEnvelopeStatus = nil
        searchMessage = nil
        isSearching = false
    }

    func read(_ candidate: WorkspaceSearchCandidate) async throws -> WorkspaceReadItem {
        try await read(WorkspaceEntryRequest(candidate: candidate))
    }

    func read(_ request: WorkspaceEntryRequest) async throws -> WorkspaceReadItem {
        if isDemo {
            return try demoRead(request)
        }

        if let reference = request.reference {
            return try await api.read(
                reference: reference,
                path: nil,
                version: request.version
            )
        }

        var lastNotFound: Error?
        for path in request.pathCandidates {
            do {
                return try await api.read(
                    reference: nil,
                    path: path,
                    version: request.version
                )
            } catch let error as StraylightAPIError {
                guard case let .server(status, _, _) = error, status == 404 else {
                    throw error
                }
                lastNotFound = error
            }
        }

        if let lookupTerm = request.lookupTerm {
            return try await api.read(
                reference: nil,
                path: nil,
                linkTarget: lookupTerm
            )
        }

        throw lastNotFound ?? StraylightAPIError.server(
            status: 404,
            code: "entry_link_not_found",
            message: "The linked entry could not be found exactly. Use its full path to avoid an ambiguous link."
        )
    }

    private func demoRead(_ request: WorkspaceEntryRequest) throws -> WorkspaceReadItem {
        let candidates = SampleData.searchResults.filter { candidate in
            candidate.reference == request.reference
                || request.pathCandidates.contains(candidate.path)
                || request.lookupTerm.map {
                    Self.linkLookupKey(candidate.path) == Self.linkLookupKey($0)
                } == true
        }
        guard candidates.count <= 1 else {
            throw StraylightAPIError.server(
                status: 409,
                code: "entry_link_ambiguous",
                message: "More than one entry matches this link. Search for its full path instead."
            )
        }
        let candidate = candidates.first ?? WorkspaceSearchCandidate(
            reference: request.reference,
            path: request.pathCandidates.first ?? "Demo linked entry.md",
            title: request.title,
            version: request.version,
            excerpt: "Deterministic demo entry."
        )
        let related = SampleData.searchResults.first(where: { $0.id != candidate.id })
        let relatedMarkdown = related.map {
            "\n\n## Related\n\n[[\($0.path)|\($0.title)]]"
        } ?? ""
        return WorkspaceReadItem(
            reference: candidate.reference,
            path: candidate.path,
            title: candidate.title,
            version: candidate.version,
            text: """
            # \(candidate.title)

            **Source-backed entry.** \(candidate.previewText)

            - Search results open this exact entry version.
            - Markdown formatting can be turned off at any time.

            This is deterministic demo content. A connected app reads the exact source from hosted Straylight.\(relatedMarkdown)
            """,
            updatedAt: candidate.updatedAt
        )
    }

    private static func linkLookupKey(_ value: String) -> String {
        var name = value.split(separator: "/").last.map(String.init) ?? value
        if name.lowercased().hasSuffix(".markdown") {
            name.removeLast(9)
        } else if name.lowercased().hasSuffix(".md") {
            name.removeLast(3)
        }
        return name.precomposedStringWithCanonicalMapping.lowercased()
    }

    func handle(_ route: AppRoute) async {
        guard phase == .ready else {
            pendingRoute = route
            return
        }

        applyLocalRoute(route)
        switch route {
        case .today:
            return
        case let .task(reference):
            await openTask(reference: reference)
        case let .notification(notificationRef, deliveryRef):
            await openNotification(reference: notificationRef, deliveryRef: deliveryRef)
        case let .conversation(conversationID, sequence):
            guard messagingEnabled, let messagingController else { return }
            focusMessagingConversation(conversationID, sequence: sequence)
            guard connectionValidated else { return }
            do {
                _ = try await messagingController.refreshThread(
                    conversationID: conversationID
                )
                messagingMessage = nil
            } catch {
                messagingMessage = "The linked conversation could not refresh. The protected local copy remains visible."
            }
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
        let previousUserID = user?.id
        invalidateDashboardContext()
        isDemo = false
        user = identity.user
        currentCredentialID = identity.credentialID
        readOnlyCredential = identity.readOnly
        // Cookie sessions are the read/login channel only. Device mutations
        // remain disabled until the separate least-privilege bearer validates.
        canWriteTasks = false
        canManageNotifications = false
        canWriteMessages = false
        messagingBearerState.token = nil
        connectionValidated = true
        phase = .ready
        if let previousUserID, previousUserID != identity.user.id {
            clearTaskSurfacePresentation()
        }
        tasks = []
        alerts = []
        connectionMessage = nil
    }

    private func clearTaskSurfacePresentation() {
        tasks = []
        urgentTasks = []
        nextTasks = []
        doneToday = nil
        taskContexts = []
        selectedTaskContexts = []
        taskProjects = []
        todoistStatus = nil
        selectedProjectState = nil
        taskNextRemaining = 0
        taskBacklogTotal = 0
        taskMessage = nil
        mutatingTaskRefs = []
        presentedTask = nil
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
        case .notification:
            selectedTab = .alerts
        case .task:
            selectedTab = .tasks
        case let .conversation(conversationID, sequence):
            guard messagingEnabled else { return }
            selectedTab = .agents
            focusedMessagingConversationID = conversationID
            focusedMessagingSequence = sequence
            try? messagingController?.selectConversation(conversationID)
        }
    }

    private func upsertNotification(_ notification: StraylightNotification) {
        if let index = notifications.firstIndex(where: {
            $0.notificationRef == notification.notificationRef
        }) {
            notifications[index] = notification
        } else {
            notifications.insert(notification, at: 0)
        }
    }

    private func applyLocalReceipt(
        notificationRef: String,
        openedAt: String?,
        acknowledgedAt: String?
    ) {
        guard let current = notifications.first(where: {
            $0.notificationRef == notificationRef
        }) ?? (presentedNotification?.notificationRef == notificationRef
            ? presentedNotification
            : nil)
        else { return }

        let transitionsToOpened = current.openedAt == nil && openedAt != nil

        let updated = StraylightNotification(
            notificationRef: current.notificationRef,
            kind: current.kind,
            importance: current.importance,
            title: current.title,
            body: current.body,
            source: current.source,
            target: current.target,
            occurredAt: current.occurredAt,
            expiresAt: current.expiresAt,
            openedAt: openedAt ?? current.openedAt,
            acknowledgedAt: acknowledgedAt ?? current.acknowledgedAt,
            deliveries: current.deliveries
        )
        upsertNotification(updated)
        if transitionsToOpened {
            notificationUnreadCount = max(0, notificationUnreadCount - 1)
        }
        if presentedNotification?.notificationRef == notificationRef {
            presentedNotification = updated
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
