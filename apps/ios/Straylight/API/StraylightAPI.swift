import Foundation

public struct StraylightAPIConfiguration: Sendable, Equatable {
    public let baseURL: URL

    public init(baseURL: URL = URL(string: "https://straylight.rourkem.com/api/v1")!) {
        self.baseURL = baseURL
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

    public init(
        configuration: StraylightAPIConfiguration = .init(),
        session: URLSession? = nil,
        cookieStorage: HTTPCookieStorage? = nil
    ) {
        self.configuration = configuration
        let resolvedCookieStorage = cookieStorage
            ?? session?.configuration.httpCookieStorage
            ?? HTTPCookieStorage.shared
        self.cookieStorage = resolvedCookieStorage
        if let session {
            self.session = session
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
        }
    }

    public func hasAuthenticatedSession() -> Bool {
        cookiesForServer().contains { Self.sessionCookieNames.contains($0.name) }
    }

    public func clearAuthenticatedSession() {
        for cookie in cookiesForServer()
            where Self.sessionCookieNames.contains(cookie.name)
                || Self.csrfCookieNames.contains(cookie.name)
        {
            cookieStorage.deleteCookie(cookie)
        }
    }

    public func login(email: String, password: String) async throws -> AuthSessionData {
        struct LoginRequest: Encodable, Sendable {
            let email: String
            let password: String
        }

        let response: WorkspaceEnvelope<AuthSessionData> = try await post(
            path: "auth/login",
            body: LoginRequest(email: email, password: password)
        )
        return response.data
    }

    public func authSession() async throws -> AuthSessionData {
        let response: WorkspaceEnvelope<AuthSessionData> = try await get(path: "auth/session")
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
        clearAuthenticatedSession()
    }

    public func me() async throws -> MeData {
        try await get(path: "me")
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
        request: NotificationInstallationRequest
    ) async throws -> NotificationInstallationResponse {
        try await put(
            path: Self.notificationInstallationPath(installationID: installationID),
            body: request
        )
    }

    public func revokeNotificationInstallation(
        installationID: UUID
    ) async throws -> NotificationInstallationResponse {
        try await delete(path: Self.notificationInstallationPath(installationID: installationID))
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

    private func get<Response: Decodable & Sendable>(
        path: String,
        queryItems: [URLQueryItem] = []
    ) async throws -> Response {
        try await request(path: path, queryItems: queryItems, method: "GET", body: nil)
    }

    private func post<Response: Decodable & Sendable>(
        path: String,
        body: some Encodable & Sendable
    ) async throws -> Response {
        let encoder = JSONEncoder()
        return try await request(
            path: path,
            queryItems: [],
            method: "POST",
            body: encoder.encode(body)
        )
    }

    private func put<Response: Decodable & Sendable>(
        path: String,
        body: some Encodable & Sendable
    ) async throws -> Response {
        let encoder = JSONEncoder()
        return try await request(
            path: path,
            queryItems: [],
            method: "PUT",
            body: encoder.encode(body)
        )
    }

    private func delete<Response: Decodable & Sendable>(
        path: String
    ) async throws -> Response {
        try await request(path: path, queryItems: [], method: "DELETE", body: nil)
    }

    private func request<Response: Decodable & Sendable>(
        path: String,
        queryItems: [URLQueryItem],
        method: String,
        body: Data?
    ) async throws -> Response {
        let url = try makeURL(path: path, queryItems: queryItems)
        var request = URLRequest(url: url)
        request.httpMethod = method
        request.httpBody = body
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        if let cookieHeader = HTTPCookie.requestHeaderFields(
            with: cookiesForServer()
        )["Cookie"] {
            request.setValue(cookieHeader, forHTTPHeaderField: "Cookie")
        }
        if body != nil {
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        if !["GET", "HEAD", "OPTIONS"].contains(method.uppercased()),
           let csrfToken = csrfToken()
        {
            request.setValue(csrfToken, forHTTPHeaderField: "X-CSRF-Token")
        }

        let (data, response) = try await session.data(for: request)
        guard let response = response as? HTTPURLResponse else {
            throw StraylightAPIError.invalidResponse
        }
        guard (200 ..< 300).contains(response.statusCode) else {
            throw decodeServerError(data: data, status: response.statusCode)
        }

        let decoder = JSONDecoder()
        do {
            return try decoder.decode(Response.self, from: data)
        } catch {
            throw StraylightAPIError.decoding(error.localizedDescription)
        }
    }

    private func cookiesForServer() -> [HTTPCookie] {
        cookieStorage.cookies(for: configuration.baseURL) ?? []
    }

    private func csrfToken() -> String? {
        cookiesForServer().first { Self.csrfCookieNames.contains($0.name) }?.value
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
