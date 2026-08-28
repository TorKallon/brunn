import CryptoKit
import Foundation

public struct StraylightAPIConfiguration: Sendable, Equatable {
    public let baseURL: URL

    public init(baseURL: URL? = nil) {
#if DEBUG
        let testOverride = ProcessInfo.processInfo.environment["STRAYLIGHT_API_BASE_URL"]
            .flatMap(URL.init(string:))
#else
        let testOverride: URL? = nil
#endif
        self.baseURL = baseURL
            ?? testOverride
            ?? URL(string: "https://straylight.rourkem.com/api/v1")!
    }
}

public enum StraylightAPIError: Error, Sendable, Equatable, LocalizedError {
    case invalidConfiguration
    case notConnected
    case invalidResponse
    case server(status: Int, code: String, message: String)
    case decoding(String)

    public var errorDescription: String? {
        switch self {
        case .invalidConfiguration:
            "The Straylight server address is invalid."
        case .notConnected:
            "Connect this device to Straylight first."
        case .invalidResponse:
            "Straylight returned an invalid response."
        case let .server(_, _, message):
            message
        case let .decoding(message):
            "Straylight returned data this app does not understand: \(message)"
        }
    }

    public var isUnauthorized: Bool {
        if case let .server(status, _, _) = self {
            return status == 401
        }
        return false
    }
}

public actor StraylightAPI {
    private static let sessionCookieNames = [
        "__Host-straylight_session",
        "straylight_session",
    ]
    private static let csrfCookieNames = [
        "__Host-straylight_csrf",
        "straylight_csrf",
    ]

    private let configuration: StraylightAPIConfiguration
    private let session: URLSession
    private let cookieStorage: HTTPCookieStorage
    private let usesBackgroundMessagingTransport: Bool
    private let sessionFingerprintURL: URL?
    private var responseCSRFToken: String?
    private var responseSessionFingerprint: String?
    private var persistedSessionFingerprint: String?

    public init(
        configuration: StraylightAPIConfiguration = .init(),
        session: URLSession? = nil,
        cookieStorage: HTTPCookieStorage? = nil,
        sessionFingerprintURL: URL? = nil
    ) {
        self.configuration = configuration
        let resolvedCookieStorage = cookieStorage
            ?? session?.configuration.httpCookieStorage
            ?? HTTPCookieStorage.shared
        self.cookieStorage = resolvedCookieStorage
        let resolvedFingerprintURL = sessionFingerprintURL
            ?? (session == nil && cookieStorage == nil
                ? Self.defaultSessionFingerprintURL()
                : nil)
        self.sessionFingerprintURL = resolvedFingerprintURL
        responseCSRFToken = nil
        responseSessionFingerprint = nil
        persistedSessionFingerprint = resolvedFingerprintURL
            .flatMap(Self.loadPersistedSessionFingerprint)
        if let session {
            self.session = session
            usesBackgroundMessagingTransport = false
        } else {
            let sessionConfiguration = URLSessionConfiguration.default
            sessionConfiguration.waitsForConnectivity = true
            sessionConfiguration.timeoutIntervalForRequest = 30
            sessionConfiguration.timeoutIntervalForResource = 60
            sessionConfiguration.requestCachePolicy = .reloadIgnoringLocalCacheData
            sessionConfiguration.urlCache = nil
            sessionConfiguration.httpCookieStorage = resolvedCookieStorage
            sessionConfiguration.httpShouldSetCookies = true
            self.session = URLSession(configuration: sessionConfiguration)
            usesBackgroundMessagingTransport = true
        }
    }

    public func hasAuthenticatedSession() -> Bool {
        authenticatedSessionFingerprint() != nil
    }

    public func authenticatedSessionFingerprint() -> String? {
        let liveFingerprint = sessionFingerprint(from: cookiesForServer())
        if let responseSessionFingerprint {
            if liveFingerprint == responseSessionFingerprint {
                self.responseSessionFingerprint = nil
            }
            return responseSessionFingerprint
        }
        if let liveFingerprint {
            if liveFingerprint != persistedSessionFingerprint {
                persistSessionFingerprint(liveFingerprint)
            }
            return liveFingerprint
        }
        return persistedSessionFingerprint
    }

    private func sessionFingerprint(from cookies: [HTTPCookie]) -> String? {
        let sessionCookies = cookies
            .filter { Self.sessionCookieNames.contains($0.name) }
            .filter { !$0.value.isEmpty }
            .sorted { $0.name < $1.name }
        guard !sessionCookies.isEmpty else { return nil }
        let material = sessionCookies.map {
            [$0.name, $0.value].joined(separator: "\u{1f}")
        }.joined(separator: "\u{1e}")
        let digest = SHA256.hash(data: Data(material.utf8))
        return "sha256:" + digest.map { String(format: "%02x", $0) }.joined()
    }

    @discardableResult
    public func clearAuthenticatedSession() async -> Bool {
        responseCSRFToken = nil
        responseSessionFingerprint = nil
        persistSessionFingerprint(nil)
        for cookie in cookiesForServer()
            where Self.sessionCookieNames.contains(cookie.name)
                || Self.csrfCookieNames.contains(cookie.name)
        {
            cookieStorage.deleteCookie(cookie)
        }
        if usesBackgroundMessagingTransport {
            return await MessagingBackgroundTransport.clearAuthenticatedSessionArtifacts()
        }
        return true
    }

    public func login(email: String, password: String) async throws -> AuthSessionData {
        struct LoginRequest: Encodable, Sendable {
            let email: String
            let password: String
        }

        // A new interactive sign-in replaces any prior cookie pair. Clearing
        // first also guarantees the response's session and CSRF cookies are
        // accepted as one pair rather than racing stale automatic handling.
        guard await clearAuthenticatedSession() else {
            throw MessagingBackgroundTransportError.artifactPurgeFailed
        }
        let response: WorkspaceEnvelope<AuthSessionData> = try await post(
            path: "auth/login",
            body: LoginRequest(email: email, password: password)
        )
        return response.data
    }

    public func authSession() async throws -> AuthSessionData {
        let response: WorkspaceEnvelope<AuthSessionData> = try await get(path: "auth/session")
        _ = authenticatedSessionFingerprint()
        return response.data
    }

    public func logout() async throws {
        let response: WorkspaceEnvelope<AuthCompletionData> = try await request(
            path: "auth/logout",
            queryItems: [],
            method: "POST",
            body: nil
        )
        _ = response
        guard await clearAuthenticatedSession() else {
            throw MessagingBackgroundTransportError.artifactPurgeFailed
        }
    }

    public func me() async throws -> MeData {
        try await get(path: "me")
    }

    public func deviceCredentialIdentity(bearerToken: String) async throws -> MeData {
        try await get(
            path: "me",
            bearerToken: bearerToken,
            sendCookies: false
        )
    }

    public func bootstrapDeviceTaskCredential() async throws -> DeviceTaskCredentialBootstrapResponse {
        struct Request: Encodable, Sendable {
            let name = "iOS task access"
            let access = "ios_tasks"
        }
        return try await post(path: "credentials", body: Request())
    }

    public func revokeCredential(reference: String) async throws -> CredentialRevocationResponse {
        try await delete(path: "credentials/\(reference)")
    }

    public func dashboard(
        timezone: String = TimeZone.current.identifier
    ) async throws -> WorkspaceEnvelope<WorkspaceDashboardData> {
        try await get(
            path: "workspace/dashboard",
            queryItems: [URLQueryItem(name: "timezone", value: timezone)]
        )
    }

    public func latestBriefing() async throws -> BriefingEditionData? {
        let list = try await briefings(limit: 1)
        guard let edition = list.data.editions.first else { return nil }
        return try await briefing(date: edition.date, edition: edition.edition)
    }

    public func briefings(
        limit: Int = 30,
        afterPath: String? = nil
    ) async throws -> WorkspaceEnvelope<BriefingListData> {
        var queryItems = [
            URLQueryItem(name: "limit", value: String(min(max(limit, 1), 60))),
        ]
        if let afterPath, !afterPath.isEmpty {
            queryItems.append(URLQueryItem(name: "after_path", value: afterPath))
        }
        return try await get(path: "workspace/briefings", queryItems: queryItems)
    }

    public func briefing(
        date: String,
        edition: String,
        version: Int? = nil
    ) async throws -> BriefingEditionData {
        let path = Self.briefingPath(date: date, edition: edition)
        let queryItems = version.map { [URLQueryItem(name: "version", value: String($0))] } ?? []
        let response: WorkspaceEnvelope<BriefingEditionData> = try await get(
            path: path,
            queryItems: queryItems
        )
        return response.data
    }

    public func briefingTopics() async throws -> WorkspaceEnvelope<BriefingTopicsSnapshot> {
        try await get(path: "workspace/briefings/topics")
    }

    public func briefingItemAction(
        _ action: BriefingItemActionRequest
    ) async throws -> WorkspaceEnvelope<BriefingItemActionData> {
        try await post(path: "workspace/briefings/items/action", body: action)
    }

    public func notifications(
        limit: Int = 50,
        cursor: String? = nil,
        unread: Bool? = nil,
        importance: StraylightNotificationImportance? = nil
    ) async throws -> NotificationListResponse {
        var queryItems = [
            URLQueryItem(name: "limit", value: String(min(max(limit, 1), 100))),
        ]
        if let cursor, !cursor.isEmpty {
            queryItems.append(URLQueryItem(name: "cursor", value: cursor))
        }
        if let unread {
            queryItems.append(URLQueryItem(name: "unread", value: String(unread)))
        }
        if let importance {
            queryItems.append(URLQueryItem(name: "importance", value: importance.rawValue))
        }
        return try await get(path: "workspace/notifications", queryItems: queryItems)
    }

    public func notification(reference: String) async throws -> StraylightNotification {
        let response: NotificationDetailResponse = try await get(
            path: Self.notificationPath(reference: reference)
        )
        return response.notification
    }

    public func taskCandidates(
        view: AgentTaskView,
        limit: Int? = nil,
        contextsAvailable: [String] = []
    ) async throws -> WorkspaceEnvelope<AgentTaskCandidatesData> {
        var queryItems = [URLQueryItem(name: "view", value: view.rawValue)]
        if let limit {
            queryItems.append(URLQueryItem(name: "limit", value: String(min(max(limit, 1), 25))))
        }
        queryItems.append(contentsOf: contextsAvailable.map {
            URLQueryItem(name: "contexts_available", value: $0)
        })
        return try await get(path: "workspace/tasks/candidates", queryItems: queryItems)
    }

    public func task(reference: String) async throws -> AgentTaskDetail {
        let response: WorkspaceEnvelope<AgentTaskDetailData> = try await get(
            path: Self.taskPath(reference: reference)
        )
        return response.data.task
    }

    public func updateTask(
        reference: String,
        request: AgentTaskUpdateRequest,
        bearerToken: String
    ) async throws -> AgentTaskUpdateData {
        let response: WorkspaceEnvelope<AgentTaskUpdateData> = try await patch(
            path: Self.taskPath(reference: reference),
            body: request,
            bearerToken: bearerToken,
            sendCookies: false
        )
        return response.data
    }

    public func taskDoneSummary(
        limit: Int = 25
    ) async throws -> WorkspaceEnvelope<AgentTaskDoneSummaryData> {
        try await get(
            path: "workspace/tasks/done-summary",
            queryItems: [URLQueryItem(name: "limit", value: String(min(max(limit, 1), 25)))]
        )
    }

    public func taskContexts() async throws -> WorkspaceEnvelope<AgentTaskContextListData> {
        try await get(
            path: "workspace/contexts",
            queryItems: [
                URLQueryItem(name: "include_archived", value: "false"),
                URLQueryItem(name: "limit", value: "100"),
            ]
        )
    }

    public func taskProjects() async throws -> WorkspaceEnvelope<AgentTaskProjectListData> {
        try await get(
            path: "workspace/projects",
            queryItems: [URLQueryItem(name: "limit", value: "100")]
        )
    }

    public func taskTodoistStatus() async throws -> WorkspaceEnvelope<AgentTaskTodoistStatus> {
        try await get(path: "workspace/integrations/todoist/status")
    }

    public func taskProjectState(slug: String) async throws -> AgentTaskProjectStateData {
        let response: WorkspaceEnvelope<AgentTaskProjectStateData> = try await get(
            path: "workspace/projects/\(slug)/state"
        )
        return response.data
    }

    func messagingStatus() async throws -> MessagingRuntimeStatus {
        try await get(path: "status")
    }

    func messagingSync(_ request: MessagingSyncRequest) async throws -> MessagingSyncResponse {
        var queryItems = [
            URLQueryItem(name: "cursor", value: String(request.cursor)),
            URLQueryItem(name: "wait", value: String(request.waitSeconds)),
            URLQueryItem(name: "limit", value: String(request.limit)),
        ]
        if let conversationID = request.conversationID {
            guard ConversationReference.canonical(conversationID) != nil else {
                throw StraylightAPIError.invalidConfiguration
            }
            queryItems.append(URLQueryItem(name: "conversation_id", value: conversationID))
        }
        if let afterSequence = request.afterSequence {
            queryItems.append(URLQueryItem(name: "after_seq", value: String(afterSequence)))
        }
        let response: WorkspaceEnvelope<MessagingSyncResponse> = try await get(
            path: "workspace/messaging/sync",
            queryItems: queryItems
        )
        return response.data
    }

    func messagingAgents() async throws -> MessagingAgentListResponse {
        let response: WorkspaceEnvelope<MessagingAgentListResponse> = try await get(
            path: "workspace/messaging/agents"
        )
        return response.data
    }

    func bindMessagingCredential(
        agentID: String,
        credentialReference: String
    ) async throws -> MessagingCredentialBindingResponse {
        guard Self.isCanonicalMessagingAgentID(agentID),
              credentialReference.hasPrefix("credential:")
        else {
            throw StraylightAPIError.invalidConfiguration
        }
        let credentialID = String(credentialReference.dropFirst("credential:".count))
        guard let identifier = UUID(uuidString: credentialID),
              credentialID == identifier.uuidString.lowercased()
        else {
            throw StraylightAPIError.invalidConfiguration
        }
        let response: WorkspaceEnvelope<MessagingCredentialBindingResponse> = try await put(
            path: "workspace/messaging/agents/\(agentID)/credential",
            body: MessagingCredentialBindingRequest(credentialID: credentialID)
        )
        return response.data
    }

    func createMessagingConversation(
        _ request: MessagingCreateConversationRequest,
        bearerToken: String
    ) async throws -> MessagingCreateConversationResponse {
        let response: WorkspaceEnvelope<MessagingCreateConversationResponse> = try await post(
            path: "workspace/messaging/conversations",
            body: request,
            bearerToken: bearerToken,
            sendCookies: false
        )
        return response.data
    }

    func sendMessagingMessage(
        conversationID: String,
        exactRequestData: Data,
        bearerToken: String
    ) async throws -> MessagingSendResponse {
        guard ConversationReference.canonical(conversationID) != nil,
              !exactRequestData.isEmpty
        else {
            throw StraylightAPIError.invalidConfiguration
        }
        if usesBackgroundMessagingTransport {
            let url = try makeURL(
                path: "workspace/messaging/conversations/\(conversationID)/messages",
                queryItems: []
            )
            var request = URLRequest(url: url)
            request.httpMethod = "POST"
            request.httpShouldHandleCookies = false
            request.setValue("application/json", forHTTPHeaderField: "Accept")
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.setValue("Bearer \(bearerToken)", forHTTPHeaderField: "Authorization")
            let upload = try await MessagingBackgroundTransport.shared.upload(
                request: request,
                exactRequestData: exactRequestData
            )
            let response: WorkspaceEnvelope<MessagingSendResponse> = try decodeResponse(
                data: upload.data,
                response: upload.response
            )
            return response.data
        }
        let response: WorkspaceEnvelope<MessagingSendResponse> = try await request(
            path: "workspace/messaging/conversations/\(conversationID)/messages",
            queryItems: [],
            method: "POST",
            body: exactRequestData,
            bearerToken: bearerToken,
            sendCookies: false
        )
        return response.data
    }

    func markMessagingRead(
        conversationID: String,
        lastReadSeq: Int64,
        bearerToken: String
    ) async throws -> MessagingReadResponse {
        guard ConversationReference.canonical(conversationID) != nil else {
            throw StraylightAPIError.invalidConfiguration
        }
        let response: WorkspaceEnvelope<MessagingReadResponse> = try await post(
            path: "workspace/messaging/conversations/\(conversationID)/read",
            body: MessagingReadRequest(lastReadSeq: lastReadSeq),
            bearerToken: bearerToken,
            sendCookies: false
        )
        return response.data
    }

    public func setTaskProjectInterest(
        slug: String,
        interest: String,
        expectedVersion: Int,
        bearerToken: String
    ) async throws {
        struct Request: Encodable, Sendable {
            let interest: String
            let expectedVersion: Int
            let source: String
            let idempotencyKey: String

            enum CodingKeys: String, CodingKey {
                case interest
                case expectedVersion = "expected_version"
                case source
                case idempotencyKey = "idempotency_key"
            }
        }
        struct IgnoredResponse: Decodable, Sendable {}

        let request = Request(
            interest: interest,
            expectedVersion: expectedVersion,
            source: "owner",
            idempotencyKey: "ios-project-interest-\(UUID().uuidString.lowercased())"
        )
        let _: WorkspaceEnvelope<IgnoredResponse> = try await put(
            path: "workspace/projects/\(slug)/interest",
            body: request,
            bearerToken: bearerToken,
            sendCookies: false
        )
    }

    public func recordNotificationReceipt(
        notificationRef: String,
        kind: NotificationReceiptKind,
        deliveryRef: String? = nil
    ) async throws -> NotificationReceiptResponse {
        try await post(
            path: "\(Self.notificationPath(reference: notificationRef))/receipts",
            body: NotificationReceiptRequest(kind: kind, deliveryRef: deliveryRef)
        )
    }

    public func upsertNotificationInstallation(
        installationID: UUID,
        request: NotificationInstallationRequest,
        bearerToken: String
    ) async throws -> NotificationInstallationResponse {
        try await put(
            path: Self.notificationInstallationPath(installationID: installationID),
            body: request,
            bearerToken: bearerToken,
            sendCookies: false
        )
    }

    public func revokeNotificationInstallation(
        installationID: UUID,
        bearerToken: String
    ) async throws -> NotificationInstallationResponse {
        try await delete(
            path: Self.notificationInstallationPath(installationID: installationID),
            bearerToken: bearerToken,
            sendCookies: false
        )
    }

    public func search(
        _ text: String,
        sort: WorkspaceSearchSort = .bestMatch
    ) async throws -> WorkspaceEnvelope<WorkspaceSearchData> {
        let request = SearchRequest(queries: [SearchQuery(query: text, sort: sort)])
        return try await post(path: "workspace/search", body: request)
    }

    public func read(
        reference: String?,
        path: String?,
        linkTarget: String? = nil,
        version: Int? = nil
    ) async throws -> WorkspaceReadItem {
        let request = ReadRequest(
            requests: [ReadRequestItem(
                reference: reference,
                path: path,
                linkTarget: linkTarget,
                version: version
            )]
        )
        let response: WorkspaceEnvelope<WorkspaceReadData> = try await post(
            path: "workspace/read",
            body: request
        )
        guard let item = response.data.items.first else {
            throw StraylightAPIError.invalidResponse
        }
        if item.status == "not_found" || item.text == nil {
            throw StraylightAPIError.server(
                status: 404,
                code: item.error?.code ?? "source_not_found",
                message: item.error?.message ?? "The matched source is no longer available."
            )
        }
        return item
    }

    public nonisolated static func briefingPath(date: String, edition: String) -> String {
        "workspace/briefings/\(date)/\(edition)"
    }

    public nonisolated static func notificationPath(reference: String) -> String {
        "workspace/notifications/\(reference)"
    }

    public nonisolated static func notificationInstallationPath(installationID: UUID) -> String {
        "workspace/notification-installations/\(installationID.uuidString.lowercased())"
    }

    public nonisolated static func taskPath(reference: String) -> String {
        "workspace/tasks/\(reference)"
    }

    private nonisolated static func isCanonicalMessagingAgentID(_ value: String) -> Bool {
        let bytes = Array(value.utf8)
        guard (1 ... 80).contains(bytes.count) else { return false }

        func isLetterOrNumber(_ byte: UInt8) -> Bool {
            (byte >= Character("a").asciiValue! && byte <= Character("z").asciiValue!)
                || (byte >= Character("0").asciiValue! && byte <= Character("9").asciiValue!)
        }

        guard let first = bytes.first,
              let last = bytes.last,
              isLetterOrNumber(first),
              isLetterOrNumber(last)
        else { return false }

        var previousWasSeparator = false
        for byte in bytes {
            if isLetterOrNumber(byte) {
                previousWasSeparator = false
            } else if byte == Character(".").asciiValue
                || byte == Character("_").asciiValue
                || byte == Character("-").asciiValue
            {
                guard !previousWasSeparator else { return false }
                previousWasSeparator = true
            } else {
                return false
            }
        }
        return true
    }

    private func get<Response: Decodable & Sendable>(
        path: String,
        queryItems: [URLQueryItem] = [],
        bearerToken: String? = nil,
        sendCookies: Bool = true
    ) async throws -> Response {
        try await request(
            path: path,
            queryItems: queryItems,
            method: "GET",
            body: nil,
            bearerToken: bearerToken,
            sendCookies: sendCookies
        )
    }

    private func post<Response: Decodable & Sendable>(
        path: String,
        body: some Encodable & Sendable,
        bearerToken: String? = nil,
        sendCookies: Bool = true
    ) async throws -> Response {
        let encoder = JSONEncoder()
        return try await request(
            path: path,
            queryItems: [],
            method: "POST",
            body: encoder.encode(body),
            bearerToken: bearerToken,
            sendCookies: sendCookies
        )
    }

    private func put<Response: Decodable & Sendable>(
        path: String,
        body: some Encodable & Sendable,
        bearerToken: String? = nil,
        sendCookies: Bool = true
    ) async throws -> Response {
        let encoder = JSONEncoder()
        return try await request(
            path: path,
            queryItems: [],
            method: "PUT",
            body: encoder.encode(body),
            bearerToken: bearerToken,
            sendCookies: sendCookies
        )
    }

    private func patch<Response: Decodable & Sendable>(
        path: String,
        body: some Encodable & Sendable,
        bearerToken: String? = nil,
        sendCookies: Bool = true
    ) async throws -> Response {
        let encoder = JSONEncoder()
        return try await request(
            path: path,
            queryItems: [],
            method: "PATCH",
            body: encoder.encode(body),
            bearerToken: bearerToken,
            sendCookies: sendCookies
        )
    }

    private func delete<Response: Decodable & Sendable>(
        path: String,
        bearerToken: String? = nil,
        sendCookies: Bool = true
    ) async throws -> Response {
        try await request(
            path: path,
            queryItems: [],
            method: "DELETE",
            body: nil,
            bearerToken: bearerToken,
            sendCookies: sendCookies
        )
    }

    private func request<Response: Decodable & Sendable>(
        path: String,
        queryItems: [URLQueryItem],
        method: String,
        body: Data?,
        bearerToken: String? = nil,
        sendCookies: Bool = true
    ) async throws -> Response {
        let url = try makeURL(path: path, queryItems: queryItems)
        var request = URLRequest(url: url)
        request.httpMethod = method
        request.httpBody = body
        request.httpShouldHandleCookies = sendCookies
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        let unsafeMethod = !["GET", "HEAD", "OPTIONS"].contains(method.uppercased())
        let csrfToken = sendCookies
            ? responseCSRFToken ?? cookiesForServer().first {
                Self.csrfCookieNames.contains($0.name)
            }?.value
            : nil
        if unsafeMethod, let csrfToken {
            // URLSession owns the Cookie header and receives/stores cookies
            // through this actor's configured HTTPCookieStorage. The explicit
            // header is retained from that same authenticated response even
            // if CFNetwork has not yet exposed its cookie-store update.
            request.setValue(csrfToken, forHTTPHeaderField: "X-CSRF-Token")
        }
        if let bearerToken {
            request.setValue("Bearer \(bearerToken)", forHTTPHeaderField: "Authorization")
        }
        if body != nil {
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }

        let (data, response) = try await session.data(for: request)
        if sendCookies {
            captureResponseCookies(response, for: url)
        }
        return try decodeResponse(data: data, response: response)
    }

    private func captureResponseCookies(_ response: URLResponse, for url: URL) {
        guard let response = response as? HTTPURLResponse else { return }
        let responseURL = response.url ?? url
        var headerFields: [String: String] = [:]
        for (rawName, rawValue) in response.allHeaderFields {
            guard let name = rawName as? String else { continue }
            if let values = rawValue as? [String] {
                headerFields[name] = values.joined(separator: ", ")
            } else {
                headerFields[name] = String(describing: rawValue)
            }
        }
        let cookies = HTTPCookie.cookies(
            withResponseHeaderFields: headerFields,
            for: responseURL
        )
        guard !cookies.isEmpty else { return }
        cookieStorage.setCookies(cookies, for: responseURL, mainDocumentURL: nil)
        let sessionCookies = cookies.filter {
            Self.sessionCookieNames.contains($0.name)
        }
        if !sessionCookies.isEmpty {
            // HTTPCookieStorage can lag its own successful setCookies call.
            // Retain only the derived fingerprint needed for immediate,
            // account-scoped cache binding; never retain the raw session value.
            let fingerprint = sessionFingerprint(from: sessionCookies)
            responseSessionFingerprint = fingerprint
            persistSessionFingerprint(fingerprint)
        }
        if let csrfCookie = cookies.first(where: {
            Self.csrfCookieNames.contains($0.name)
        }) {
            responseCSRFToken = csrfCookie.value.isEmpty ? nil : csrfCookie.value
        }
    }

    private func decodeResponse<Response: Decodable & Sendable>(
        data: Data,
        response: URLResponse
    ) throws -> Response {
        guard let response = response as? HTTPURLResponse else {
            throw StraylightAPIError.invalidResponse
        }
        guard (200 ..< 300).contains(response.statusCode) else {
            throw decodeServerError(data: data, status: response.statusCode)
        }

        do {
            return try JSONDecoder().decode(Response.self, from: data)
        } catch {
            throw StraylightAPIError.decoding(error.localizedDescription)
        }
    }

    private func cookiesForServer() -> [HTTPCookie] {
        cookieStorage.cookies(for: configuration.baseURL) ?? []
    }

    private func persistSessionFingerprint(_ fingerprint: String?) {
        persistedSessionFingerprint = fingerprint
        guard let sessionFingerprintURL else { return }
        if let fingerprint, Self.isValidSessionFingerprint(fingerprint) {
            do {
                let directory = sessionFingerprintURL.deletingLastPathComponent()
                try FileManager.default.createDirectory(
                    at: directory,
                    withIntermediateDirectories: true,
                    attributes: [.protectionKey: FileProtectionType.complete]
                )
                try FileManager.default.setAttributes(
                    [.protectionKey: FileProtectionType.complete],
                    ofItemAtPath: directory.path
                )
                var directoryValues = URLResourceValues()
                directoryValues.isExcludedFromBackup = true
                var mutableDirectory = directory
                try mutableDirectory.setResourceValues(directoryValues)
                try Data(fingerprint.utf8).write(
                    to: sessionFingerprintURL,
                    options: [.atomic, .completeFileProtection]
                )
                var fileValues = URLResourceValues()
                fileValues.isExcludedFromBackup = true
                var mutableFile = sessionFingerprintURL
                try mutableFile.setResourceValues(fileValues)
            } catch {
                // Cookie auth remains authoritative. Failure to persist this
                // non-secret digest only disables protected cold-cache paint.
            }
        } else if FileManager.default.fileExists(atPath: sessionFingerprintURL.path) {
            try? FileManager.default.removeItem(at: sessionFingerprintURL)
        }
    }

    private static func defaultSessionFingerprintURL() -> URL? {
        let root = FileManager.default.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        )[0]
        var directory = root.appendingPathComponent("Straylight", isDirectory: true)
#if DEBUG
        if let namespace = ProcessInfo.processInfo.environment["STRAYLIGHT_CREDENTIAL_NAMESPACE"],
           !namespace.isEmpty
        {
            let allowed = CharacterSet(
                charactersIn: "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_"
            )
            guard (1 ... 64).contains(namespace.utf8.count),
                  namespace.unicodeScalars.allSatisfy(allowed.contains)
            else { return nil }
            directory.appendPathComponent(namespace, isDirectory: true)
        }
#endif
        return directory.appendingPathComponent(
            "web-session-fingerprint-v1",
            isDirectory: false
        )
    }

    private static func loadPersistedSessionFingerprint(from url: URL) -> String? {
        guard let data = try? Data(contentsOf: url),
              let fingerprint = String(data: data, encoding: .utf8),
              isValidSessionFingerprint(fingerprint)
        else { return nil }
        return fingerprint
    }

    private static func isValidSessionFingerprint(_ value: String) -> Bool {
        guard value.hasPrefix("sha256:"), value.utf8.count == 71 else { return false }
        return value.utf8.dropFirst(7).allSatisfy { byte in
            (byte >= 48 && byte <= 57) || (byte >= 97 && byte <= 102)
        }
    }

    private func makeURL(path: String, queryItems: [URLQueryItem]) throws -> URL {
        var url = configuration.baseURL
        for component in path.split(separator: "/") {
            url.appendPathComponent(String(component))
        }
        guard !queryItems.isEmpty else { return url }
        guard var components = URLComponents(url: url, resolvingAgainstBaseURL: false) else {
            throw StraylightAPIError.invalidConfiguration
        }
        components.queryItems = queryItems
        guard let result = components.url else {
            throw StraylightAPIError.invalidConfiguration
        }
        return result
    }

    private func decodeServerError(data: Data, status: Int) -> StraylightAPIError {
        struct ErrorResponse: Decodable {
            struct ServerError: Decodable {
                let code: String?
                let message: String?
            }

            let error: ServerError?
            let code: String?
            let message: String?
        }

        let decoded = try? JSONDecoder().decode(ErrorResponse.self, from: data)
        let code = decoded?.error?.code ?? decoded?.code ?? "http_\(status)"
        let message = decoded?.error?.message
            ?? decoded?.message
            ?? HTTPURLResponse.localizedString(forStatusCode: status)
        return .server(status: status, code: code, message: message)
    }
}
