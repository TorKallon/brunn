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
    private let configuration: StraylightAPIConfiguration
    private let session: URLSession
    private var bearerToken: String?

    public init(
        configuration: StraylightAPIConfiguration = .init(),
        bearerToken: String? = nil,
        session: URLSession? = nil
    ) {
        self.configuration = configuration
        self.bearerToken = bearerToken?.trimmingCharacters(in: .whitespacesAndNewlines)
        if let session {
            self.session = session
        } else {
            let sessionConfiguration = URLSessionConfiguration.ephemeral
            sessionConfiguration.waitsForConnectivity = true
            sessionConfiguration.timeoutIntervalForRequest = 30
            sessionConfiguration.timeoutIntervalForResource = 60
            sessionConfiguration.requestCachePolicy = .reloadIgnoringLocalCacheData
            sessionConfiguration.urlCache = nil
            sessionConfiguration.httpCookieStorage = nil
            sessionConfiguration.httpShouldSetCookies = false
            self.session = URLSession(configuration: sessionConfiguration)
        }
    }

    public func setBearerToken(_ token: String?) {
        bearerToken = token?.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    public func me() async throws -> MeData {
        try await get(path: "me")
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

    public func search(_ text: String) async throws -> WorkspaceEnvelope<WorkspaceSearchData> {
        let request = SearchRequest(queries: [SearchQuery(query: text)])
        return try await post(path: "workspace/search", body: request)
    }

    public func read(
        reference: String?,
        path: String?,
        version: Int? = nil
    ) async throws -> WorkspaceReadItem {
        let request = ReadRequest(
            requests: [ReadRequestItem(reference: reference, path: path, version: version)]
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

    private func request<Response: Decodable & Sendable>(
        path: String,
        queryItems: [URLQueryItem],
        method: String,
        body: Data?
    ) async throws -> Response {
        guard let bearerToken, !bearerToken.isEmpty else {
            throw StraylightAPIError.notConnected
        }
        let url = try makeURL(path: path, queryItems: queryItems)
        var request = URLRequest(url: url)
        request.httpMethod = method
        request.httpBody = body
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.setValue("Bearer \(bearerToken)", forHTTPHeaderField: "Authorization")
        if body != nil {
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
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
